// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `AssetSummary` — public-facing view of an inbound (or
//! self-sent) asset row, plus the `download` / `save_to` methods
//! consumers use to materialise the plaintext. Step 11g.
//!
//! `AssetSummary` holds the metadata needed to render the asset
//! in a UI (mime, filename, size, sender) plus crate-private state
//! needed by `download` / `save_to` (`asset_id` triple, `sha256`,
//! packed `otr_key` envelope, `encryption_algo`, `cache_path`,
//! weak `Client` back-reference). The `Weak<Client>` lets a
//! UI consumer hold the summary across re-syncs without keeping
//! the Client alive; if the Client has been dropped, download
//! returns `InvalidInput`.
//!
//! ## Decrypt pipeline
//!
//! ```text
//! download():
//!   1. AES_GCM gate (warn + InvalidInput per ADR-0011)
//!   2. resolve cache_path OR download to <kupferdraht_home>/<acct>/assets/<msg>.enc
//!   3. SHA-256 verify over the ciphertext
//!   4. unpack otr_key DEK envelope (60B packed = nonce ‖ tag ‖ ct,
//!      32B = legacy plaintext)
//!   5. asset_crypto::decrypt_to_memory (capped at 64 MB)
//! save_to(path) is the same pipeline but uses decrypt_to_path
//! to stream directly to the caller-supplied destination.
//! ```

#![cfg_attr(not(test), allow(dead_code))]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};

use crate::client::Client;
#[cfg(not(target_arch = "wasm32"))]
use crate::encryption::asset_crypto::decrypt_to_memory;
use crate::encryption::asset_crypto::decrypt_to_path;
use crate::error::KupferdrahtError;
use crate::storage::AttachmentRecord;

/// Size of the raw AES-256 `otr_key`.
const RAW_KEY_LEN: usize = 32;

/// Public-facing summary of an asset attachment. Returned to
/// consumers via `MessageBody::Asset` (event handler callback)
/// and `TimelineItemKind::Asset` (timeline read API).
#[derive(Clone)]
pub struct AssetSummary {
    /// `GenericMessage.message_id` of the carrying MLS message.
    pub message_id: String,
    /// Asset mime type from `Asset.Original.mime_type`.
    pub mime_type: String,
    /// Display filename from `Asset.Original.name`.
    pub filename: Option<String>,
    /// Plaintext size from `Asset.Original.size`.
    pub size_bytes: u64,
    /// Qualified user id (`<uuid>@<domain>`) of the sender.
    pub sender_user_id: String,
    /// Qualified room id the asset was sent in.
    pub room_id: String,
    /// Opaque Wire asset locator. Pairs with `asset_domain` to
    /// form the GET URL.
    pub asset_id: String,
    /// Wire `Asset-Token` for private (E2EE) assets. `None` for
    /// public ones (Kupferdraht emits only private uploads).
    pub asset_token: Option<String>,
    /// Wire domain hosting the asset blob.
    pub asset_domain: String,
    /// `"AES_CBC"` (Kupferdraht-emitted + most Wire clients) or
    /// `"AES_GCM"` (decoded but unconsumable — ADR-0011
    /// §AES-GCM toleration). Diagnostic — `download()` rejects
    /// `AES_GCM` rows.
    pub encryption_algo: String,
    /// Absolute path to the on-disk encrypted-cache blob, when
    /// populated. `download()` lazily populates this on first
    /// call.
    pub cache_path: Option<String>,

    // Crate-private state — security material kept off the
    // public surface so consumers can't accidentally log the
    // raw integrity hash or the DEK-wrapped key.
    pub(crate) sha256: Vec<u8>,
    pub(crate) otr_key_ct: Vec<u8>,
    pub(crate) client: Weak<Client>,
}

impl std::fmt::Debug for AssetSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Step 101: secrets redacted (`crate::redact`).
        f.debug_struct("AssetSummary")
            .field("message_id", &self.message_id)
            .field("mime_type", &self.mime_type)
            .field("filename", &self.filename)
            .field("size_bytes", &self.size_bytes)
            .field("sender_user_id", &self.sender_user_id)
            .field("room_id", &self.room_id)
            .field("asset_id", &self.asset_id)
            .field(
                "asset_token",
                &crate::redact::hidden_option(self.asset_token.as_ref()),
            )
            .field("asset_domain", &self.asset_domain)
            .field("encryption_algo", &self.encryption_algo)
            .field("cache_path", &self.cache_path)
            .field("sha256", &self.sha256)
            .field("otr_key_ct", &crate::redact::byte_count(&self.otr_key_ct))
            .finish_non_exhaustive()
    }
}

impl crate::timeline::Timeline {
    /// Look up the persisted asset for `message_id` in this
    /// timeline's room. Returns `None` when the message is not in
    /// the local store or isn't an asset (text / reaction / delete
    /// rows have no `attachments` entry). Step 11h.
    ///
    /// Reach for `summary.download()` / `summary.save_to(path)` on
    /// the returned value to materialise the plaintext.
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` on `SQLite` read failure.
    pub async fn asset(&self, message_id: &str) -> Result<Option<AssetSummary>, KupferdrahtError> {
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
        Ok(Some(AssetSummary::from_attachment(
            attachment,
            sender_user_id,
            std::sync::Arc::downgrade(client_arc),
        )))
    }
}

/// Manual `PartialEq` — ignores the `Weak<Client>` back-reference
/// so the enclosing `MessageBody::Asset` / `TimelineItemKind::Asset`
/// variants can derive `PartialEq` for free. Two summaries are
/// equal iff their public metadata + asset locator + integrity
/// material match.
impl PartialEq for AssetSummary {
    fn eq(&self, other: &Self) -> bool {
        self.message_id == other.message_id
            && self.mime_type == other.mime_type
            && self.filename == other.filename
            && self.size_bytes == other.size_bytes
            && self.sender_user_id == other.sender_user_id
            && self.room_id == other.room_id
            && self.asset_id == other.asset_id
            && self.asset_token == other.asset_token
            && self.asset_domain == other.asset_domain
            && self.sha256 == other.sha256
            && self.otr_key_ct == other.otr_key_ct
            && self.encryption_algo == other.encryption_algo
            && self.cache_path == other.cache_path
    }
}

impl Eq for AssetSummary {}

impl AssetSummary {
    /// Build an `AssetSummary` from an `attachments` row plus the
    /// sender metadata. Crate-private — consumers reach
    /// `AssetSummary` via `MessageBody::Asset(_)` or
    /// `TimelineItemKind::Asset(_)`.
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
            asset_id: attachment.asset_id,
            asset_token: attachment.asset_token,
            asset_domain: attachment.asset_domain,
            sha256: attachment.sha256,
            otr_key_ct: attachment.otr_key_ct,
            encryption_algo: attachment.encryption_algo,
            cache_path: attachment.cache_path,
            client,
        }
    }

    /// Download (or load from cache), verify SHA-256, decrypt, and
    /// return the plaintext bytes. Capped at 64 MB per
    /// [`crate::encryption::asset_crypto::decrypt_to_memory`].
    /// For larger assets use [`Self::save_to`].
    ///
    /// # Errors
    /// * `InvalidInput` — owning Client dropped, asset uses
    ///   `AES_GCM` (warn-and-skip per ADR-0011), or no `SqliteStore`
    ///   attached (no cache directory derivable).
    /// * `IntegrityCheck` — SHA-256 mismatch over the downloaded
    ///   ciphertext.
    /// * `Network` / `BackendRejected` — `/v14/assets/...` failed.
    /// * `Storage` — filesystem error or DEK envelope unpack
    ///   failure.
    /// * `Encoding` — AES-CBC unpadding rejected (wrong key /
    ///   corrupt ciphertext).
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn download(&self) -> Result<Vec<u8>, KupferdrahtError> {
        let cipher_path = self.ensure_cache_populated().await?;
        let key = self.unpack_otr_key()?;
        let sha = sha_array(&self.sha256)?;
        decrypt_to_memory(&cipher_path, &key, &sha)
    }

    /// wasm (W7): no filesystem cache — fetch the ciphertext into memory,
    /// verify SHA-256, and decrypt in memory.
    ///
    /// # Errors
    /// As the native variant, plus `Network`/`BackendRejected` from the fetch.
    #[cfg(target_arch = "wasm32")]
    pub async fn download(&self) -> Result<Vec<u8>, KupferdrahtError> {
        if self.encryption_algo != "AES_CBC" {
            return Err(KupferdrahtError::InvalidInput(format!(
                "asset download: unsupported encryption_algo {}",
                self.encryption_algo
            )));
        }
        let client = self.client.upgrade().ok_or_else(|| {
            KupferdrahtError::InvalidInput("asset download: owning Client has been dropped".into())
        })?;
        let key = self.unpack_otr_key()?;
        let sha = sha_array(&self.sha256)?;
        let access_token = client.access_token().ok_or_else(|| {
            KupferdrahtError::InvalidInput(
                "asset download: no access token (locked client?)".into(),
            )
        })?;
        let ciphertext = client
            .http_download_asset_bytes(
                &access_token,
                &self.asset_domain,
                &self.asset_id,
                self.asset_token.as_deref(),
            )
            .await?;
        crate::encryption::asset_crypto::decrypt_bytes(&ciphertext, &key, &sha)
    }

    /// Same pipeline as [`Self::download`] but streams the
    /// plaintext to `dest_path` — suitable for large assets that
    /// shouldn't sit in memory.
    ///
    /// # Errors
    /// Same as [`Self::download`].
    pub async fn save_to(&self, dest_path: &Path) -> Result<(), KupferdrahtError> {
        let cipher_path = self.ensure_cache_populated().await?;
        let key = self.unpack_otr_key()?;
        let sha = sha_array(&self.sha256)?;
        decrypt_to_path(&cipher_path, &key, &sha, dest_path).map(|_| ())
    }

    /// If `cache_path` is set and the file exists, return it.
    /// Otherwise, gate `AES_GCM`, download to the per-account
    /// cache directory, update the attachment row's `cache_path`,
    /// and return the new path.
    async fn ensure_cache_populated(&self) -> Result<PathBuf, KupferdrahtError> {
        if let Some(p) = &self.cache_path {
            let path = PathBuf::from(p);
            if path.exists() {
                return Ok(path);
            }
            // cache_path was recorded but the file is gone (manual
            // user cleanup, OS cache eviction). Fall through and
            // re-download.
        }
        if self.encryption_algo == "AES_GCM" {
            return Err(KupferdrahtError::InvalidInput(format!(
                "AssetSummary: encryption_algo=AES_GCM unsupported (ADR-0011 §AES-GCM toleration); \
                 asset {} skipped",
                self.asset_id
            )));
        }
        let client = self.client.upgrade().ok_or_else(|| {
            KupferdrahtError::InvalidInput("AssetSummary: Client dropped before download".into())
        })?;
        let cache_path = self.compute_cache_path(&client)?;
        let access_token = client.access_token().ok_or_else(|| {
            KupferdrahtError::InvalidInput("AssetSummary::download: no access token".into())
        })?;
        if let Some(parent) = cache_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                KupferdrahtError::Storage(format!("mkdir {}: {e}", parent.display()))
            })?;
        }
        client
            .http_download_asset(
                &access_token,
                &self.asset_domain,
                &self.asset_id,
                self.asset_token.as_deref(),
                &cache_path,
            )
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

    /// `<kupferdraht_home>/<account_id>/assets/<message_id>.enc`.
    fn compute_cache_path(&self, client: &Arc<Client>) -> Result<PathBuf, KupferdrahtError> {
        let dir = client.assets_dir().ok_or_else(|| {
            KupferdrahtError::InvalidInput(
                "AssetSummary::download: no base_dir configured on the Client".into(),
            )
        })?;
        Ok(dir.join(asset_cache_file_name(&self.message_id)?))
    }

    /// Read the per-asset `otr_key`. It is stored verbatim — the
    /// `attachments.otr_key` column lives in the whole-DB-encrypted
    /// `kupferdraht.db`, and the asset *content* is separately E2E-encrypted
    /// under this key for Wire — so this is the raw 32-byte AES key.
    fn unpack_otr_key(&self) -> Result<zeroize::Zeroizing<[u8; 32]>, KupferdrahtError> {
        self.otr_key_ct
            .as_slice()
            .try_into()
            .map(zeroize::Zeroizing::new)
            .map_err(|_| {
                KupferdrahtError::Storage(format!(
                    "AssetSummary: otr_key is {} bytes (expected {RAW_KEY_LEN})",
                    self.otr_key_ct.len()
                ))
            })
    }
}

fn sha_array(sha: &[u8]) -> Result<[u8; 32], KupferdrahtError> {
    sha.try_into().map_err(|_| {
        KupferdrahtError::Storage(format!(
            "AssetSummary: sha256 must be 32 bytes, got {}",
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
            otr_key_ct: vec![0u8; packed_envelope_len],
            asset_id: "asset-id".into(),
            asset_token: Some("tok".into()),
            asset_domain: "wire.com".into(),
            encryption_algo: "AES_CBC".into(),
            cache_path: None,
            created_at_ms: 0,
        }
    }

    #[test]
    fn from_attachment_copies_metadata_into_summary() {
        let weak: Weak<Client> = Weak::new();
        let summary =
            AssetSummary::from_attachment(sample_attachment(60), "alice@wire.test".into(), weak);
        assert_eq!(summary.message_id, "m-1");
        assert_eq!(summary.mime_type, "image/png");
        assert_eq!(summary.filename.as_deref(), Some("photo.png"));
        assert_eq!(summary.size_bytes, 1024);
        assert_eq!(summary.sender_user_id, "alice@wire.test");
        assert_eq!(summary.asset_id, "asset-id");
        assert_eq!(summary.encryption_algo, "AES_CBC");
    }

    #[test]
    fn partial_eq_ignores_weak_client_back_reference() {
        // Two summaries with identical metadata + distinct
        // Weak<Client> values (both `Weak::new()` since we can't
        // construct a real Client in a unit test) compare equal.
        let a = AssetSummary::from_attachment(sample_attachment(60), "u@x".into(), Weak::new());
        let b = AssetSummary::from_attachment(sample_attachment(60), "u@x".into(), Weak::new());
        assert_eq!(a, b);
    }

    #[test]
    fn partial_eq_distinguishes_different_otr_key_envelopes() {
        let mut a = sample_attachment(60);
        let mut b = sample_attachment(60);
        a.otr_key_ct[0] = 0xAA;
        b.otr_key_ct[0] = 0xBB;
        let sa = AssetSummary::from_attachment(a, "u@x".into(), Weak::new());
        let sb = AssetSummary::from_attachment(b, "u@x".into(), Weak::new());
        assert_ne!(sa, sb, "different otr_key_ct → different summaries");
    }
}

/// Derive the cache file name (`<message_id>.enc`) for an inbound
/// asset, refusing path-unsafe ids (Step 80). The pure half of
/// `compute_cache_path`, split out so the guard is unit-testable
/// without a `Client`; the send-side cache move shares it.
pub(crate) fn asset_cache_file_name(message_id: &str) -> Result<String, KupferdrahtError> {
    if !crate::storage::is_path_safe_id(message_id) {
        return Err(KupferdrahtError::InvalidInput(format!(
            "asset cache: refusing path-unsafe message id {message_id:?}"
        )));
    }
    Ok(format!("{message_id}.enc"))
}

#[cfg(test)]
mod cache_name_tests {
    use super::asset_cache_file_name;

    /// Step 80 path-site guard (defense in depth below the dispatch
    /// boundary): the cache file name derivation itself refuses
    /// hostile ids instead of joining them.
    #[test]
    fn hostile_message_ids_never_become_cache_names() {
        for bad in ["../evil", "..\\evil", "a/b", "", ".."] {
            assert!(asset_cache_file_name(bad).is_err(), "must refuse {bad:?}");
        }
    }

    #[test]
    fn uuid_ids_map_to_enc_names() {
        let name =
            asset_cache_file_name("5c22fa31-77aa-4bde-9d10-223344556677").expect("uuid id is fine");
        assert_eq!(name, "5c22fa31-77aa-4bde-9d10-223344556677.enc");
    }
}
