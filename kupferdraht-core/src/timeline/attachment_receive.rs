// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `AttachmentSummary` — the public description of an inbound (or
//! self-sent) attachment, plus the `download` / `save_to` methods
//! consumers use to materialise the plaintext. Step 11g.
//!
//! `AttachmentSummary` holds what a UI renders (mime, filename, size,
//! sender) plus the crate-private state `download` / `save_to` need:
//! the server's locator (id, token, domain), `sha256`, the wrapped
//! content key, the cipher and a weak `Client` back-reference. The
//! `Weak<Client>` lets a UI consumer hold the summary across re-syncs
//! without keeping the Client alive; if the Client has been dropped,
//! download returns `InvalidInput`.
//!
//! ## Decrypt pipeline
//!
//! ```text
//! download():
//!   1. AES_GCM gate (warn + InvalidInput per ADR-0011)
//!   2. resolve cache_path OR download into the account's attachment cache,
//!      stopping past the declared size + 32 bytes (IntegrityCheck, Step 160)
//!   3. SHA-256 verify over the ciphertext
//!   4. unwrap the content key (60B packed = nonce ‖ tag ‖ ct,
//!      32B = legacy plaintext)
//!   5. attachment_crypto::decrypt_to_memory (capped at 64 MB)
//! save_to(path) is the same pipeline but uses decrypt_to_path
//! to stream directly to the caller-supplied destination.
//! ```

#![cfg_attr(not(test), allow(dead_code))]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};

use crate::client::Client;
#[cfg(not(target_arch = "wasm32"))]
use crate::encryption::attachment_crypto::decrypt_to_memory;
use crate::encryption::attachment_crypto::{decrypt_to_path, max_ciphertext_len};
use crate::error::KupferdrahtError;
use crate::storage::AttachmentRecord;
use crate::wire::http::assets::AssetDownload;

/// Size of the raw AES-256 content key.
const RAW_KEY_LEN: usize = 32;

/// The description of an attachment. Returned to consumers via
/// `MessageBody::Attachment` (event handler callback) and
/// `TimelineItemKind::Attachment` (timeline read API); the plaintext
/// comes from `download()` or `save_to()`. The server's locator,
/// token and cipher stay inside: a consumer has no use for them, and
/// the token grants the download.
#[derive(Clone)]
pub struct AttachmentSummary {
    /// The id of the message that carries the attachment.
    pub message_id: String,
    /// The attachment's mime type.
    pub mime_type: String,
    /// The display filename, when the sender named one.
    pub filename: Option<String>,
    /// The plaintext size.
    pub size_bytes: u64,
    /// The sender's user id (`<uuid>@<domain>`).
    pub sender_user_id: String,
    /// The room the attachment was sent in.
    pub room_id: String,
    /// Absolute path to the on-disk encrypted-cache blob, when
    /// populated. `download()` lazily populates this on first
    /// call.
    pub cache_path: Option<String>,

    // Crate-private state: the server's locator and the security
    // material, kept off the public surface so consumers can't
    // accidentally log the token, the integrity hash or the wrapped key.
    /// The server's id of the encrypted blob; with `attachment_domain`
    /// it forms the download URL.
    pub(crate) attachment_id: String,
    /// The token a private upload is fetched with; `None` for a public
    /// one (Kupferdraht uploads only private ones).
    pub(crate) attachment_token: Option<String>,
    /// The domain hosting the encrypted blob.
    pub(crate) attachment_domain: String,
    /// `"AES_CBC"` (what Kupferdraht and most clients send) or
    /// `"AES_GCM"` (decoded but unconsumable — ADR-0011 §AES-GCM
    /// toleration); `download()` rejects `AES_GCM`.
    pub(crate) cipher: String,
    pub(crate) sha256: Vec<u8>,
    pub(crate) key_ct: Vec<u8>,
    pub(crate) client: Weak<Client>,
}

impl std::fmt::Debug for AttachmentSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Step 101: secrets redacted (`crate::redact`).
        f.debug_struct("AttachmentSummary")
            .field("message_id", &self.message_id)
            .field("mime_type", &self.mime_type)
            .field("filename", &self.filename)
            .field("size_bytes", &self.size_bytes)
            .field("sender_user_id", &self.sender_user_id)
            .field("room_id", &self.room_id)
            .field("attachment_id", &self.attachment_id)
            .field(
                "attachment_token",
                &crate::redact::hidden_option(self.attachment_token.as_ref()),
            )
            .field("attachment_domain", &self.attachment_domain)
            .field("cipher", &self.cipher)
            .field("cache_path", &self.cache_path)
            .field("sha256", &self.sha256)
            .field("key_ct", &crate::redact::byte_count(&self.key_ct))
            .finish_non_exhaustive()
    }
}

impl crate::timeline::Timeline {
    /// Look up the persisted attachment for `message_id` in this
    /// timeline's room. Returns `None` when the message is not in
    /// the local store or carries no attachment (text / reaction /
    /// delete rows have no `attachments` entry). Step 11h.
    ///
    /// Reach for `summary.download()` / `summary.save_to(path)` on
    /// the returned value to materialise the plaintext.
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` on `SQLite` read failure.
    pub async fn attachment(
        &self,
        message_id: &str,
    ) -> Result<Option<AttachmentSummary>, KupferdrahtError> {
        let client_arc = self.client_handle();
        let Some(store) = client_arc.store_handle() else {
            return Ok(None);
        };
        let Some(attachment) = store.get_attachment(message_id, self.room_id()).await? else {
            return Ok(None);
        };
        // Sender lookup is best-effort — if the timeline_events
        // row is gone (orphan attachment, defensive degraded
        // state) we fall back to "unknown".
        let sender_user_id = client_arc
            .lookup_timeline_event(self.room_id(), message_id)
            .await?
            .map(|t| t.sender_user_id)
            .unwrap_or_default();
        Ok(Some(AttachmentSummary::from_attachment(
            attachment,
            sender_user_id,
            std::sync::Arc::downgrade(client_arc),
        )))
    }
}

/// Manual `PartialEq` — ignores the `Weak<Client>` back-reference
/// so the enclosing `MessageBody::Attachment` / `TimelineItemKind::Attachment`
/// variants can derive `PartialEq` for free. Two summaries are
/// equal iff their public metadata + attachment locator + integrity
/// material match.
impl PartialEq for AttachmentSummary {
    fn eq(&self, other: &Self) -> bool {
        self.message_id == other.message_id
            && self.mime_type == other.mime_type
            && self.filename == other.filename
            && self.size_bytes == other.size_bytes
            && self.sender_user_id == other.sender_user_id
            && self.room_id == other.room_id
            && self.attachment_id == other.attachment_id
            && self.attachment_token == other.attachment_token
            && self.attachment_domain == other.attachment_domain
            && self.sha256 == other.sha256
            && self.key_ct == other.key_ct
            && self.cipher == other.cipher
            && self.cache_path == other.cache_path
    }
}

impl Eq for AttachmentSummary {}

impl AttachmentSummary {
    /// Build an `AttachmentSummary` from an `attachments` row plus the
    /// sender metadata. Crate-private — consumers reach
    /// `AttachmentSummary` via `MessageBody::Attachment(_)` or
    /// `TimelineItemKind::Attachment(_)`.
    pub(crate) fn from_attachment(
        attachment: AttachmentRecord,
        sender_user_id: String,
        client: Weak<Client>,
    ) -> Self {
        Self {
            message_id: attachment.message_id,
            mime_type: attachment.mime_type,
            filename: attachment.filename,
            size_bytes: attachment.size_bytes,
            sender_user_id,
            room_id: attachment.room_id,
            attachment_id: attachment.attachment_id,
            attachment_token: attachment.attachment_token,
            attachment_domain: attachment.attachment_domain,
            sha256: attachment.sha256,
            key_ct: attachment.key_ct,
            cipher: attachment.cipher,
            cache_path: attachment.cache_path,
            client,
        }
    }

    /// Download (or load from cache), verify SHA-256, decrypt, and
    /// return the plaintext bytes. Capped at 64 MB per
    /// [`crate::encryption::attachment_crypto::decrypt_to_memory`].
    /// For larger attachments use [`Self::save_to`].
    ///
    /// # Errors
    /// * `InvalidInput` — owning Client dropped, its message was deleted
    ///   for everyone since this summary was taken (Step 84), attachment uses
    ///   `AES_GCM` (warn-and-skip per ADR-0011), or no `SqliteStore`
    ///   attached (no cache directory derivable).
    /// * `IntegrityCheck` — SHA-256 mismatch over the downloaded
    ///   ciphertext, or a download longer than the declared size
    ///   + 32 bytes (Step 160; no partial file stays in the cache).
    /// * `Network` / `BackendRejected` — `/v14/assets/...` failed.
    /// * `Storage` — filesystem error or DEK envelope unpack
    ///   failure.
    /// * `Encoding` — AES-CBC unpadding rejected (wrong key /
    ///   corrupt ciphertext).
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn download(&self) -> Result<Vec<u8>, KupferdrahtError> {
        let cipher_path = self.ensure_cache_populated().await?;
        let key = self.unpack_key()?;
        let sha = sha_array(&self.sha256)?;
        decrypt_to_memory(&cipher_path, &key, &sha)
    }

    /// wasm (W7): no filesystem cache — fetch the ciphertext into memory,
    /// verify SHA-256, and decrypt it in that buffer. The fetch stops past
    /// the declared size + 32 bytes, and an attachment declared over 100 MiB is
    /// refused before any request (Step 160).
    ///
    /// # Errors
    /// As the native variant, plus `Network`/`BackendRejected` from the fetch
    /// and `InvalidInput` for an attachment over the in-memory ceiling.
    #[cfg(target_arch = "wasm32")]
    pub async fn download(&self) -> Result<Vec<u8>, KupferdrahtError> {
        if self.cipher != "AES_CBC" {
            return Err(KupferdrahtError::InvalidInput(format!(
                "attachment download: unsupported cipher {}",
                self.cipher
            )));
        }
        let client = self.client.upgrade().ok_or_else(|| {
            KupferdrahtError::InvalidInput(
                "attachment download: owning Client has been dropped".into(),
            )
        })?;
        self.ensure_not_deleted(&client).await?;
        let key = self.unpack_key()?;
        let sha = sha_array(&self.sha256)?;
        let access_token = client.access_token().ok_or_else(|| {
            KupferdrahtError::InvalidInput(
                "attachment download: no access token (locked client?)".into(),
            )
        })?;
        let ciphertext = client
            .http_download_attachment_bytes(&access_token, self.bounded_download())
            .await?;
        crate::encryption::attachment_crypto::decrypt_bytes(ciphertext, &key, &sha)
    }

    /// Same pipeline as [`Self::download`] but streams the
    /// plaintext to `dest_path` — suitable for large attachments that
    /// shouldn't sit in memory.
    ///
    /// # Errors
    /// Same as [`Self::download`].
    pub async fn save_to(&self, dest_path: &Path) -> Result<(), KupferdrahtError> {
        let cipher_path = self.ensure_cache_populated().await?;
        let key = self.unpack_key()?;
        let sha = sha_array(&self.sha256)?;
        decrypt_to_path(&cipher_path, &key, &sha, dest_path).map(|_| ())
    }

    /// If `cache_path` is set and the file exists, return it.
    /// Otherwise, gate `AES_GCM`, download to the per-account
    /// cache directory, update the attachment row's `cache_path`,
    /// and return the new path.
    async fn ensure_cache_populated(&self) -> Result<PathBuf, KupferdrahtError> {
        let client = self.client.upgrade().ok_or_else(|| {
            KupferdrahtError::InvalidInput(
                "AttachmentSummary: Client dropped before download".into(),
            )
        })?;
        self.ensure_not_deleted(&client).await?;
        if let Some(p) = &self.cache_path {
            let path = PathBuf::from(p);
            if path.exists() {
                return Ok(path);
            }
            // cache_path was recorded but the file is gone (manual
            // user cleanup, OS cache eviction). Fall through and
            // re-download.
        }
        if self.cipher == "AES_GCM" {
            return Err(KupferdrahtError::InvalidInput(format!(
                "AttachmentSummary: cipher=AES_GCM unsupported (ADR-0011 §AES-GCM toleration); \
                 attachment {} skipped",
                self.attachment_id
            )));
        }
        self.download_to_cache(&client).await
    }

    /// Refuse a file whose message was deleted for everyone since this
    /// summary was taken: the delete took its row, and the summary's own copy
    /// of the key and locator must not bring the file back (Step 84). A
    /// client without a store has no rows to ask.
    async fn ensure_not_deleted(&self, client: &Client) -> Result<(), KupferdrahtError> {
        let Some(store) = client.store_handle() else {
            return Ok(());
        };
        match store
            .get_attachment(&self.message_id, &self.room_id)
            .await?
        {
            Some(_) => Ok(()),
            None => Err(KupferdrahtError::InvalidInput(format!(
                "attachment {}: its message was deleted",
                self.message_id
            ))),
        }
    }

    /// Download the ciphertext into the per-account cache, record its path
    /// on the attachment row, and return it.
    async fn download_to_cache(&self, client: &Arc<Client>) -> Result<PathBuf, KupferdrahtError> {
        let cache_path = self.compute_cache_path(client)?;
        let access_token = client.access_token().ok_or_else(|| {
            KupferdrahtError::InvalidInput("AttachmentSummary::download: no access token".into())
        })?;
        if let Some(parent) = cache_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                KupferdrahtError::Storage(format!("mkdir {}: {e}", parent.display()))
            })?;
        }
        client
            .http_download_attachment(&access_token, self.bounded_download(), &cache_path)
            .await?;
        // Persist the cache_path back to the attachments row so the
        // NEXT call hits the local file. Silent no-op when no
        // SqliteStore is attached (test / in-memory client).
        if let Some(store) = client.store_handle()
            && let Some(p) = cache_path.to_str()
        {
            store
                .update_attachment_cache_path(&self.message_id, &self.room_id, Some(p))
                .await?;
        }
        Ok(cache_path)
    }

    /// The download of this attachment: the sender's locator, bounded at the
    /// longest ciphertext its declared size can make (Step 160).
    fn bounded_download(&self) -> AssetDownload<'_> {
        AssetDownload {
            asset_domain: &self.attachment_domain,
            asset_id: &self.attachment_id,
            asset_token: self.attachment_token.as_deref(),
            max_bytes: max_ciphertext_len(self.size_bytes),
        }
    }

    /// `<kupferdraht_home>/<account_id>/attachments/<message_id>.enc`.
    fn compute_cache_path(&self, client: &Arc<Client>) -> Result<PathBuf, KupferdrahtError> {
        let dir = client.attachments_dir().ok_or_else(|| {
            KupferdrahtError::InvalidInput(
                "AttachmentSummary::download: no base_dir configured on the Client".into(),
            )
        })?;
        Ok(dir.join(attachment_cache_file_name(&self.message_id)?))
    }

    /// Read the attachment's content key. It is stored verbatim — the
    /// `attachments.otr_key` column lives in the whole-DB-encrypted
    /// `kupferdraht.db`, and the attachment *content* is separately E2E-encrypted
    /// under this key for Wire — so this is the raw 32-byte AES key.
    fn unpack_key(&self) -> Result<zeroize::Zeroizing<[u8; 32]>, KupferdrahtError> {
        self.key_ct
            .as_slice()
            .try_into()
            .map(zeroize::Zeroizing::new)
            .map_err(|_| {
                KupferdrahtError::Storage(format!(
                    "AttachmentSummary: the content key is {} bytes (expected {RAW_KEY_LEN})",
                    self.key_ct.len()
                ))
            })
    }
}

fn sha_array(sha: &[u8]) -> Result<[u8; 32], KupferdrahtError> {
    sha.try_into().map_err(|_| {
        KupferdrahtError::Storage(format!(
            "AttachmentSummary: sha256 must be 32 bytes, got {}",
            sha.len()
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_attachment(packed_envelope_len: usize) -> AttachmentRecord {
        AttachmentRecord {
            message_id: "m-1".into(),
            room_id: "room-a@wire.test".into(),
            mime_type: "image/png".into(),
            filename: Some("photo.png".into()),
            size_bytes: 1024,
            sha256: vec![0u8; 32],
            key_ct: vec![0u8; packed_envelope_len],
            attachment_id: "attachment-id".into(),
            attachment_token: Some("tok".into()),
            attachment_domain: "example.test".into(),
            cipher: "AES_CBC".into(),
            cache_path: None,
            created_at_ms: 0,
        }
    }

    #[test]
    fn from_attachment_copies_metadata_into_summary() {
        let weak: Weak<Client> = Weak::new();
        let summary = AttachmentSummary::from_attachment(
            sample_attachment(60),
            "alice@wire.test".into(),
            weak,
        );
        assert_eq!(summary.message_id, "m-1");
        assert_eq!(summary.mime_type, "image/png");
        assert_eq!(summary.filename.as_deref(), Some("photo.png"));
        assert_eq!(summary.size_bytes, 1024);
        assert_eq!(summary.sender_user_id, "alice@wire.test");
        assert_eq!(summary.attachment_id, "attachment-id");
        assert_eq!(summary.cipher, "AES_CBC");
    }

    #[test]
    fn partial_eq_ignores_weak_client_back_reference() {
        // Two summaries with identical metadata + distinct
        // Weak<Client> values (both `Weak::new()` since we can't
        // construct a real Client in a unit test) compare equal.
        let a =
            AttachmentSummary::from_attachment(sample_attachment(60), "u@x".into(), Weak::new());
        let b =
            AttachmentSummary::from_attachment(sample_attachment(60), "u@x".into(), Weak::new());
        assert_eq!(a, b);
    }

    #[test]
    fn partial_eq_distinguishes_different_otr_key_envelopes() {
        let mut a = sample_attachment(60);
        let mut b = sample_attachment(60);
        a.key_ct[0] = 0xAA;
        b.key_ct[0] = 0xBB;
        let sa = AttachmentSummary::from_attachment(a, "u@x".into(), Weak::new());
        let sb = AttachmentSummary::from_attachment(b, "u@x".into(), Weak::new());
        assert_ne!(sa, sb, "different key_ct → different summaries");
    }
}

/// Derive the cache file name (`<message_id>.enc`) for an inbound
/// attachment, refusing path-unsafe ids (Step 80). The pure half of
/// `compute_cache_path`, split out so the guard is unit-testable
/// without a `Client`; the send-side cache move shares it.
pub(crate) fn attachment_cache_file_name(message_id: &str) -> Result<String, KupferdrahtError> {
    if !crate::storage::is_path_safe_id(message_id) {
        return Err(KupferdrahtError::InvalidInput(format!(
            "attachment cache: refusing path-unsafe message id {message_id:?}"
        )));
    }
    Ok(format!("{message_id}.enc"))
}

#[cfg(test)]
mod cache_name_tests {
    use super::attachment_cache_file_name;

    /// Step 80 path-site guard (defense in depth below the dispatch
    /// boundary): the cache file name derivation itself refuses
    /// hostile ids instead of joining them.
    #[test]
    fn hostile_message_ids_never_become_cache_names() {
        for bad in ["../evil", "..\\evil", "a/b", "", ".."] {
            assert!(
                attachment_cache_file_name(bad).is_err(),
                "must refuse {bad:?}"
            );
        }
    }

    #[test]
    fn uuid_ids_map_to_enc_names() {
        let name = attachment_cache_file_name("5c22fa31-77aa-4bde-9d10-223344556677")
            .expect("uuid id is fine");
        assert_eq!(name, "5c22fa31-77aa-4bde-9d10-223344556677.enc");
    }
}
