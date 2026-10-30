// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `Timeline::send_file` outbound orchestrator + helpers — Step 11f.
//!
//! Pipeline (per ADR-0011 §Streaming model and plan §11f):
//!
//! ```text
//! send_file(path, mime_type):
//!   1. file_sharing_check()             // Step 10e policy gate
//!   2. encrypt_file(plaintext_path)     // Step 11b → EncryptedAsset
//!   3. compute_md5_b64(ciphertext_path) // Wire's Content-MD5 hdr
//!   4. http.upload_asset(...)           // Step 11c → AssetUploadResponse
//!   5. build_asset_generic_message(...) // Step 11a proto
//!   6. encrypt_and_upload(plaintext)    // existing MLS encrypt + POST /v14/mls/messages
//!   7. pack DEK envelope over otr_key
//!   8. insert_timeline_event + insert_attachment
//!   9. move tempfile to per-account cache (~/.kupferdraht/<acct>/assets/<msg>.enc)
//!  10. return MessageHandle
//! ```
//!
//! Step-10 carryover §C: `Enabled` is the dominant `fileSharing` case
//! on Wire v14 deployments; `Disabled` is the edge case the gate
//! exists for.
//!
//! Failure mode after partial progress: if MLS-send fails AFTER the
//! /v14/assets upload succeeded, the asset blob is leaked on Wire's
//! storage (`retention=persistent`, no auto-cleanup). Kupferdraht accepts
//! this — a defensive `delete_asset` on failure is a future
//! enhancement; the cost of the leak is one Wire blob.

#![cfg_attr(not(test), allow(dead_code))]

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use base64::Engine as _;
use md5::{Digest as _, Md5};

use crate::client::{Client, FileSharingPolicy};
use crate::encryption::asset_crypto::{EncryptedAsset, STREAM_CHUNK, encrypt_file, encrypt_reader};
use crate::error::KupferdrahtError;
use crate::storage::{AttachmentRecord, TimelineEventRecord};
use crate::timeline::{DeliveryStatus, MessageHandle, Timeline};
use crate::wire::http::assets::AssetMultipart;
use crate::wire::proto::{AssetMessageInput, build_asset_generic_message};

impl Timeline {
    /// Encrypt-and-send a file as an `Asset` `GenericMessage`. Step 11f.
    ///
    /// Pipeline: AES-256-CBC encrypt to a tempfile (Step 11b) →
    /// upload via `POST /v14/assets` (Step 11c) → build the MLS
    /// reference via `build_asset_generic_message` (Step 11a) →
    /// encrypt+send through the same MLS pipeline `send_text` uses →
    /// persist `timeline_events` + `attachments` rows (Step 11d).
    ///
    /// Consults `Client::file_sharing()` (Step 10e) before touching
    /// `/v14/assets` — `Disabled` → `InvalidInput` with no Wire
    /// call, `Enabled` → proceed (Step-10 carryover §C: this is the
    /// common case).
    ///
    /// # Errors
    /// * `InvalidInput` — room not in cache, `file_sharing`
    ///   policy rejected, or the source file is unreadable.
    /// * `Storage` — local filesystem / `SQLite` error.
    /// * `Network` / `BackendRejected` — `/v14/assets` or
    ///   `/v14/mls/messages` failed.
    /// * `Encoding` — protobuf / response-body shape error.
    pub async fn send_file(
        &self,
        path: &Path,
        mime_type: &str,
    ) -> Result<MessageHandle, KupferdrahtError> {
        self.client_handle()
            .send_file(self.room_id(), path, mime_type)
            .await
    }

    /// Streaming counterpart of [`Self::send_file`]: send a file from an
    /// [`AssetSource`] byte stream rather than a filesystem path (no temp copy
    /// of the plaintext; e.g. an Android `content://` `InputStream`).
    /// `filename` is the display name — the stream carries no path to derive
    /// it from.
    ///
    /// # Errors
    /// Same as [`Self::send_file`], plus any `KupferdrahtError` the `AssetSource`
    /// returns while reading.
    pub async fn send_file_stream(
        &self,
        source: Arc<dyn AssetSource>,
        filename: &str,
        mime_type: &str,
    ) -> Result<MessageHandle, KupferdrahtError> {
        self.client_handle()
            .send_file_stream(self.room_id(), source, filename, mime_type)
            .await
    }
}

impl Client {
    /// See [`Timeline::send_file`] for the caller-facing contract.
    pub(crate) async fn send_file(
        &self,
        room_id: &str,
        path: &Path,
        mime_type: &str,
    ) -> Result<MessageHandle, KupferdrahtError> {
        debug_assert!(!room_id.is_empty(), "send_file: room_id must be non-empty");
        // 1. Policy gate — fail fast before any tempfile / Wire call.
        self.file_sharing_check().await?;
        // 2. Verify the room exists + has MLS state before doing
        //    crypto work that would otherwise be wasted.
        let group_id = self.resolve_room_group_id(room_id, "send_file").await?;
        // 3. Encrypt the plaintext (streamed) to a tempfile under base_dir.
        let ciphertext_dir = self.asset_staging_dir()?;
        let encrypted = encrypt_file(path, &ciphertext_dir)?;
        let filename = path.file_name().and_then(|s| s.to_str());
        // 4-8: Content-MD5 → upload → MLS reference → MLS send → persist → emit.
        self.finish_asset_send(
            room_id,
            &group_id,
            encrypted,
            mime_type,
            filename,
            "send_file",
        )
        .await
    }

    /// Streaming variant of [`Self::send_file`]: encrypt + send a file from a
    /// foreign [`AssetSource`] byte stream instead of a filesystem path, so the
    /// caller need not materialise the plaintext on disk (e.g. an Android
    /// `content://` `InputStream`). Same memory profile — the plaintext is read
    /// chunk-by-chunk, never fully buffered.
    pub(crate) async fn send_file_stream(
        &self,
        room_id: &str,
        source: Arc<dyn AssetSource>,
        filename: &str,
        mime_type: &str,
    ) -> Result<MessageHandle, KupferdrahtError> {
        debug_assert!(
            !room_id.is_empty(),
            "send_file_stream: room_id must be non-empty"
        );
        self.file_sharing_check().await?;
        let group_id = self
            .resolve_room_group_id(room_id, "send_file_stream")
            .await?;
        let ciphertext_dir = self.asset_staging_dir()?;
        let encrypted = encrypt_reader(AssetSourceReader::new(source), &ciphertext_dir)?;
        self.finish_asset_send(
            room_id,
            &group_id,
            encrypted,
            mime_type,
            Some(filename),
            "send_file_stream",
        )
        .await
    }

    /// Shared tail of both send-file paths (Steps 4-8): Content-MD5 → upload to
    /// `/v14/assets` → build the MLS `Asset` reference → persist
    /// `timeline_events` + `attachments` as `Sending` → emit the live append →
    /// MLS encrypt + send → settle the row.
    ///
    /// A `/v14/assets` failure surfaces as an `Err` with no row (an upload
    /// failure, not a failed-message bubble). The row is written before the MLS
    /// send (Step 97a), so an MLS-send failure or a cancelled send leaves a
    /// row that reads as `Failed` and resends without a second blob upload
    /// (`encrypt_and_upload` ratchets the group *before* the HTTP call, so a
    /// resend re-encrypts a fresh `GenericMessage`). Step 25a.
    async fn finish_asset_send(
        &self,
        room_id: &str,
        group_id: &[u8],
        encrypted: EncryptedAsset,
        mime_type: &str,
        filename: Option<&str>,
        op: &'static str,
    ) -> Result<MessageHandle, KupferdrahtError> {
        let md5_b64 = compute_md5_b64(&encrypted.ciphertext_path)?;
        let upload = self
            .upload_asset_blob(room_id, &encrypted, mime_type, filename, &md5_b64)
            .await?;
        let asset_domain = upload
            .domain
            .clone()
            .unwrap_or_else(|| default_asset_domain(self));
        let (message_id, plaintext_pb) = build_asset_generic_message(AssetMessageInput {
            mime_type,
            size: encrypted.plaintext_size,
            filename,
            otr_key: &**encrypted.key_material(),
            sha256: &encrypted.sha256,
            asset_id: &upload.key,
            asset_token: upload.token.as_deref(),
            asset_domain: &asset_domain,
        })?;
        let _in_flight = self.begin_send(&message_id);
        let sent_at_ms = crate::time::now_ms();
        if let Some(persisted) = self
            .persist_outgoing_asset(
                room_id,
                &message_id,
                mime_type,
                filename,
                &encrypted,
                &upload,
                sent_at_ms,
                DeliveryStatus::Sending,
            )
            .await?
        {
            log_tempfile_cache_move(&encrypted.ciphertext_path, persisted.cache_path.as_deref());
            self.emit_outgoing_asset_append(persisted.attachment, persisted.timeline, room_id);
        }
        let mls_upload = self.encrypt_and_upload(group_id, op, &plaintext_pb).await;
        self.finish_send(room_id, &message_id, mls_upload.is_ok(), None)
            .await;
        mls_upload?;
        Ok(MessageHandle {
            message_id,
            sent_at_ms,
        })
    }

    /// Build the asset `TimelineItem` from the just-persisted rows and emit a
    /// live `TimelineDiff::Append`, mirroring `emit_outgoing_text_append` —
    /// so the sender's own outbound asset (success or `Failed`) surfaces
    /// immediately instead of only on the next timeline read. Step 25a.
    fn emit_outgoing_asset_append(
        &self,
        attachment: AttachmentRecord,
        timeline: TimelineEventRecord,
        room_id: &str,
    ) {
        let summary = crate::timeline::AssetSummary::from_attachment(
            attachment,
            self.own_qualified_user_id(),
            self.weak(),
        );
        let mut item = crate::timeline::TimelineItem::from_record(timeline);
        item.kind = crate::timeline::TimelineItemKind::Asset(Box::new(summary));
        self.dispatch_timeline_diff(&crate::timeline::TimelineDiff::Append {
            room_id: room_id.to_string(),
            item,
        });
    }

    /// Directory the encrypt step stages the ciphertext tempfile in. Prefer the
    /// per-account asset dir under `base_dir` — always present in production,
    /// disk-backed, and the same filesystem as the final asset cache (so the
    /// post-upload move is a cheap rename). Fall back to the OS cache dir only
    /// for an in-memory client built without `base_dir`. That OS-cache fallback
    /// is unavailable on Android (`dirs::cache_dir()` → `None`), which is why
    /// the `base_dir` path is the production one.
    fn asset_staging_dir(&self) -> Result<PathBuf, KupferdrahtError> {
        let dir = match self.assets_dir() {
            Some(assets) => assets.join("staging"),
            None => dirs::cache_dir()
                .ok_or_else(|| {
                    KupferdrahtError::Environment("no OS cache directory available".into())
                })?
                .join("kupferdraht")
                .join("asset-encrypt"),
        };
        std::fs::create_dir_all(&dir).map_err(|e| {
            KupferdrahtError::Storage(format!("mkdir asset staging dir {}: {e}", dir.display()))
        })?;
        Ok(dir)
    }

    /// Step 10e policy projection — reject early when the team's
    /// `fileSharing` policy is `Disabled`. `Enabled` is the common
    /// case (Step-10 carryover §C).
    async fn file_sharing_check(&self) -> Result<(), KupferdrahtError> {
        match self.file_sharing().await? {
            FileSharingPolicy::Enabled => Ok(()),
            FileSharingPolicy::Disabled => Err(KupferdrahtError::InvalidInput(
                "send_file: team policy disables file sharing".into(),
            )),
        }
    }

    /// Thin wrapper over `HttpClient::upload_asset` that forwards the
    /// `EncryptedAsset` size + the caller's mime/filename. `conv_id` is derived
    /// from the qualified `room_id` (`<uuid>@<domain>`) — Wire requires it in the
    /// audit metadata when the team has asset audit logging enabled
    /// (`400 missing-audit-metadata` otherwise).
    async fn upload_asset_blob(
        &self,
        room_id: &str,
        encrypted: &EncryptedAsset,
        mime_type: &str,
        filename: Option<&str>,
        md5_b64: &str,
    ) -> Result<crate::wire::http::assets::AssetUploadResponse, KupferdrahtError> {
        let access_token = self.access_token().ok_or_else(|| {
            KupferdrahtError::InvalidInput("send_file: no access token (locked client?)".into())
        })?;
        self.http_upload_asset(
            &access_token,
            AssetMultipart {
                conv_id: room_id.split_once('@'),
                filename,
                filetype: Some(mime_type),
                ciphertext_path: &encrypted.ciphertext_path,
                ciphertext_size: encrypted.ciphertext_size,
                md5_b64,
            },
        )
        .await
    }

    /// Persist the outbound `timeline_events` + `attachments` rows
    /// and move the encrypted tempfile into the per-account cache.
    /// Returns the final cache path (when caching is enabled).
    #[allow(clippy::too_many_arguments)]
    async fn persist_outgoing_asset(
        &self,
        room_id: &str,
        message_id: &str,
        mime_type: &str,
        filename: Option<&str>,
        encrypted: &EncryptedAsset,
        upload: &crate::wire::http::assets::AssetUploadResponse,
        sent_at_ms: i64,
        status: DeliveryStatus,
    ) -> Result<Option<OutgoingAsset>, KupferdrahtError> {
        debug_assert!(
            !message_id.is_empty(),
            "persist_outgoing_asset: message_id required"
        );
        let Some(store) = self.store_handle().cloned() else {
            return Ok(None);
        };
        let sender_user_id = self.own_qualified_user_id();
        let device_id = self
            .device_id()
            .ok_or_else(|| {
                KupferdrahtError::InvalidInput("send_file: no device_id (locked client?)".into())
            })?
            .to_string();
        // 8a. Insert the `timeline_events` row first — the
        // attachments FK depends on it.
        let timeline = TimelineEventRecord {
            message_id: message_id.to_string(),
            room_id: room_id.to_string(),
            sender_user_id,
            sender_device_id: device_id,
            kind: "asset".into(),
            body: filename.unwrap_or_default().to_string(),
            sent_at_ms,
            delivery_status: status.as_db_str().into(),
            reply_to_id: None,
            edited_at_ms: None,
            redacted_at_ms: None,
            expires_at_ms: None,
            ephemeral_ttl_ms: None,
        };
        // 8b. Store the otr_key verbatim: the `attachments.otr_key`
        // column lives in the whole-DB-encrypted `kupferdraht.db`, and the
        // asset *content* is separately E2E-encrypted under this key for
        // Wire (`asset_crypto`), so no extra wrapping is needed here.
        let envelope = encrypted.key_material().to_vec();
        // 8c. Move the encrypted tempfile to the per-account cache
        // so a future `download` on this same device hits a local
        // file. Cache directory creation is best-effort; failure
        // leaves the attachment row with `cache_path = None` and
        // the consumer falls back to a re-download from Wire.
        let cache_path = self.move_tempfile_to_cache(&encrypted.ciphertext_path, message_id)?;
        let cache_path_str = cache_path
            .as_deref()
            .and_then(|p| p.to_str())
            .map(str::to_string);
        let attachment = AttachmentRecord {
            message_id: message_id.to_string(),
            room_id: room_id.to_string(),
            mime_type: mime_type.to_string(),
            filename: filename.map(str::to_string),
            size_bytes: encrypted.plaintext_size,
            sha256: encrypted.sha256.to_vec(),
            otr_key_ct: envelope,
            asset_id: upload.key.clone(),
            asset_token: upload.token.clone(),
            asset_domain: upload
                .domain
                .clone()
                .unwrap_or_else(|| default_asset_domain(self)),
            encryption_algo: "AES_CBC".into(),
            cache_path: cache_path_str,
            created_at_ms: sent_at_ms,
        };
        // Step 105: the row and its attachment are one write.
        if !store
            .insert_timeline_event_with_attachment(&timeline, &attachment)
            .await?
        {
            return Err(crate::timeline::pipeline::outgoing_row_exists(message_id));
        }
        Ok(Some(OutgoingAsset {
            cache_path,
            attachment,
            timeline,
        }))
    }

    /// `<kupferdraht_home>/<account_id>/assets/<message_id>.enc`. `None`
    /// when no `base_dir` is configured (in-memory test path).
    /// Side-effect: creates the directory if missing.
    fn move_tempfile_to_cache(
        &self,
        ciphertext_path: &Path,
        message_id: &str,
    ) -> Result<Option<PathBuf>, KupferdrahtError> {
        let Some(dir) = self.assets_dir() else {
            return Ok(None);
        };
        std::fs::create_dir_all(&dir)
            .map_err(|e| KupferdrahtError::Storage(format!("mkdir {}: {e}", dir.display())))?;
        // Step 80: our own ids are UUIDs, but the derivation shares
        // the guarded helper so no future caller can regress this.
        let dest = dir.join(super::asset_receive::asset_cache_file_name(message_id)?);
        std::fs::rename(ciphertext_path, &dest).map_err(|e| {
            KupferdrahtError::Storage(format!(
                "move {} -> {}: {e}",
                ciphertext_path.display(),
                dest.display()
            ))
        })?;
        Ok(Some(dest))
    }
}

// ── wasm: in-memory asset send (W7) ─────────────────────────────────────────
#[cfg(target_arch = "wasm32")]
impl Client {
    /// Send `bytes` as an encrypted `Asset` message, entirely in memory — the
    /// browser flavor of [`Self::send_file`] (no tempfile staging, no cache
    /// move). Encrypt → upload buffer → build the MLS reference → MLS send →
    /// persist `timeline_events` + `attachments` rows (`cache_path = None`).
    pub(crate) async fn send_bytes(
        &self,
        room_id: &str,
        bytes: &[u8],
        filename: &str,
        mime_type: &str,
    ) -> Result<MessageHandle, KupferdrahtError> {
        self.file_sharing_check().await?;
        let group_id = self.resolve_room_group_id(room_id, "send_bytes").await?;

        let enc = crate::encryption::asset_crypto::encrypt_bytes(bytes)?;
        let md5_b64 = {
            let mut hasher = Md5::new();
            hasher.update(&enc.ciphertext);
            base64::engine::general_purpose::STANDARD.encode(hasher.finalize())
        };

        let access_token = self.access_token().ok_or_else(|| {
            KupferdrahtError::InvalidInput("send_bytes: no access token (locked client?)".into())
        })?;
        let filename_opt = (!filename.is_empty()).then_some(filename);
        // Wire requires the conversation reference in the audit metadata when the
        // team has asset audit logging enabled (`400 missing-audit-metadata`
        // otherwise). `room_id` is the qualified `<uuid>@<domain>`.
        let conv_id = room_id.split_once('@');
        let upload = self
            .http_upload_asset_bytes(
                &access_token,
                conv_id,
                filename_opt,
                Some(mime_type),
                &enc.ciphertext,
                &md5_b64,
            )
            .await?;

        let asset_domain = upload
            .domain
            .clone()
            .unwrap_or_else(|| default_asset_domain(self));
        let (message_id, plaintext_pb) = build_asset_generic_message(AssetMessageInput {
            mime_type,
            size: enc.plaintext_size,
            filename: filename_opt,
            otr_key: &**enc.key_material(),
            sha256: &enc.sha256,
            asset_id: &upload.key,
            asset_token: upload.token.as_deref(),
            asset_domain: &asset_domain,
        })?;
        // Step 97a: the rows are written as `Sending` before the MLS send.
        let _in_flight = self.begin_send(&message_id);
        let sent_at_ms = crate::time::now_ms();
        self.persist_outgoing_bytes_asset(&OutgoingBytes {
            room_id,
            message_id: &message_id,
            filename,
            mime_type,
            enc: &enc,
            upload: &upload,
            sent_at_ms,
        })
        .await?;
        let mls_upload = self
            .encrypt_and_upload(&group_id, "send_bytes", &plaintext_pb)
            .await;
        self.finish_send(room_id, &message_id, mls_upload.is_ok(), None)
            .await;
        mls_upload?;
        Ok(MessageHandle {
            message_id,
            sent_at_ms,
        })
    }
}

/// One in-memory asset send for [`Client::persist_outgoing_bytes_asset`].
#[cfg(target_arch = "wasm32")]
struct OutgoingBytes<'a> {
    room_id: &'a str,
    message_id: &'a str,
    filename: &'a str,
    mime_type: &'a str,
    enc: &'a crate::encryption::asset_crypto::EncryptedBytes,
    upload: &'a crate::wire::http::assets::AssetUploadResponse,
    sent_at_ms: i64,
}

#[cfg(target_arch = "wasm32")]
impl Client {
    /// Persist the outbound in-memory asset as `Sending` (Step 97a):
    /// `timeline_events` + `attachments` (`cache_path = None`). A no-op
    /// without a store.
    ///
    /// # Errors
    /// `InvalidInput` without a device id; storage errors.
    async fn persist_outgoing_bytes_asset(
        &self,
        out: &OutgoingBytes<'_>,
    ) -> Result<(), KupferdrahtError> {
        let Some(store) = self.store_handle().cloned() else {
            return Ok(());
        };
        let device_id = self
            .device_id()
            .ok_or_else(|| {
                KupferdrahtError::InvalidInput("send_bytes: no device_id (locked client?)".into())
            })?
            .to_string();
        let timeline = TimelineEventRecord {
            message_id: out.message_id.to_string(),
            room_id: out.room_id.to_string(),
            sender_user_id: self.own_qualified_user_id(),
            sender_device_id: device_id,
            kind: "asset".into(),
            body: out.filename.to_string(),
            sent_at_ms: out.sent_at_ms,
            delivery_status: DeliveryStatus::Sending.as_db_str().into(),
            reply_to_id: None,
            edited_at_ms: None,
            redacted_at_ms: None,
            expires_at_ms: None,
            ephemeral_ttl_ms: None,
        };
        // Step 105: the row and its attachment are one write.
        if store
            .insert_timeline_event_with_attachment(&timeline, &self.bytes_attachment_record(out))
            .await?
        {
            Ok(())
        } else {
            Err(crate::timeline::pipeline::outgoing_row_exists(
                out.message_id,
            ))
        }
    }

    /// The `attachments` row of an in-memory asset send.
    fn bytes_attachment_record(&self, out: &OutgoingBytes<'_>) -> AttachmentRecord {
        AttachmentRecord {
            message_id: out.message_id.to_string(),
            room_id: out.room_id.to_string(),
            mime_type: out.mime_type.to_string(),
            filename: (!out.filename.is_empty()).then(|| out.filename.to_string()),
            size_bytes: out.enc.plaintext_size,
            sha256: out.enc.sha256.to_vec(),
            otr_key_ct: out.enc.key_material().to_vec(),
            asset_id: out.upload.key.clone(),
            asset_token: out.upload.token.clone(),
            asset_domain: out
                .upload
                .domain
                .clone()
                .unwrap_or_else(|| default_asset_domain(self)),
            encryption_algo: "AES_CBC".into(),
            cache_path: None,
            created_at_ms: out.sent_at_ms,
        }
    }
}

/// The persisted outbound-asset state `send_file` needs to emit a live
/// `TimelineDiff::Append` (mirroring text): the cache path (for logging) plus
/// the attachment + timeline rows that build the asset `TimelineItem`.
struct OutgoingAsset {
    cache_path: Option<PathBuf>,
    attachment: AttachmentRecord,
    timeline: TimelineEventRecord,
}

/// Stream-compute `base64(MD5(file_bytes))` over `path`. Used as
/// Wire's transport-level `Content-MD5` header on the multipart
/// upload body (`docs/wire/asset-protocol.md` §Upload request body).
fn compute_md5_b64(path: &Path) -> Result<String, KupferdrahtError> {
    use std::fs::File;
    use std::io::BufReader;
    let file = File::open(path)
        .map_err(|e| KupferdrahtError::Storage(format!("open {} for md5: {e}", path.display())))?;
    let mut reader = BufReader::with_capacity(64 * 1024, file);
    let mut hasher = Md5::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| KupferdrahtError::Storage(format!("read for md5: {e}")))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let digest = hasher.finalize();
    Ok(base64::engine::general_purpose::STANDARD.encode(digest))
}

/// Foreign-implemented byte source for [`Client::send_file_stream`]. The app
/// feeds the plaintext in chunks (e.g. from an Android `content://`
/// `InputStream`), so no filesystem path / temp copy is needed.
#[uniffi::export(with_foreign)]
pub trait AssetSource: Send + Sync {
    /// Read up to `max_len` bytes from the source. Return an **empty** `Vec`
    /// at end of input. `max_len` is Kupferdraht's encrypt chunk size
    /// (`STREAM_CHUNK`, 64 KiB).
    ///
    /// # Errors
    /// Any `KupferdrahtError` aborts the send before anything reaches Wire.
    fn read_chunk(&self, max_len: u32) -> Result<Vec<u8>, KupferdrahtError>;
}

/// `std::io::Read` adapter over an [`AssetSource`] callback: buffers one
/// foreign chunk at a time and treats an empty chunk as EOF, so the existing
/// `encrypt_reader` pipeline streams straight from the foreign source.
struct AssetSourceReader {
    source: Arc<dyn AssetSource>,
    buf: Vec<u8>,
    pos: usize,
    eof: bool,
}

impl AssetSourceReader {
    fn new(source: Arc<dyn AssetSource>) -> Self {
        Self {
            source,
            buf: Vec::new(),
            pos: 0,
            eof: false,
        }
    }
}

impl Read for AssetSourceReader {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        if self.pos >= self.buf.len() {
            if self.eof {
                return Ok(0);
            }
            let max = u32::try_from(STREAM_CHUNK).unwrap_or(u32::MAX);
            self.buf = self
                .source
                .read_chunk(max)
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            self.pos = 0;
            if self.buf.is_empty() {
                self.eof = true;
                return Ok(0);
            }
        }
        let n = out.len().min(self.buf.len() - self.pos);
        out[..n].copy_from_slice(&self.buf[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

/// Diagnostic: log where the tempfile ended up. Failure is silent
/// — the move already happened successfully (or returned an
/// explicit Err earlier).
fn log_tempfile_cache_move(_temp: &Path, _dest: Option<&Path>) {
    // The persist path produces all the info needed; this is a
    // hook for future tracing if we want detailed cache-move
    // metrics. Kept as a no-op so the call site stays simple.
}

/// Fallback `asset_domain` for an upload response that omits
/// `domain`: the user's home backend domain, i.e. the backend that
/// stored the blob. Pre-federation Wire servers omitted the field;
/// a federated Wire v14 backend sets it, so the fallback only fires
/// against older deployments.
fn default_asset_domain(client: &Client) -> String {
    client.account().domain().to_owned()
}

#[cfg(test)]
mod asset_source_reader_tests {
    use super::{AssetSource, AssetSourceReader};
    use crate::error::KupferdrahtError;
    use std::collections::VecDeque;
    use std::io::Read;
    use std::sync::{Arc, Mutex};

    struct VecSource {
        chunks: Mutex<VecDeque<Vec<u8>>>,
    }

    impl VecSource {
        fn new(chunks: Vec<Vec<u8>>) -> Arc<Self> {
            Arc::new(Self {
                chunks: Mutex::new(chunks.into()),
            })
        }
    }

    impl AssetSource for VecSource {
        fn read_chunk(&self, _max_len: u32) -> Result<Vec<u8>, KupferdrahtError> {
            Ok(self
                .chunks
                .lock()
                .expect("lock")
                .pop_front()
                .unwrap_or_default())
        }
    }

    #[test]
    fn reassembles_chunks_and_stops_at_eof() {
        let source = VecSource::new(vec![
            b"hello ".to_vec(),
            b"streamed ".to_vec(),
            b"world".to_vec(),
        ]);
        let mut reader = AssetSourceReader::new(source);
        let mut out = Vec::new();
        reader.read_to_end(&mut out).expect("read");
        assert_eq!(out, b"hello streamed world");
    }

    struct ErrSource;

    impl AssetSource for ErrSource {
        fn read_chunk(&self, _max_len: u32) -> Result<Vec<u8>, KupferdrahtError> {
            Err(KupferdrahtError::Storage("boom".into()))
        }
    }

    #[test]
    fn source_error_surfaces() {
        let mut reader = AssetSourceReader::new(Arc::new(ErrSource));
        let mut out = Vec::new();
        let err = reader.read_to_end(&mut out).expect_err("must error");
        assert!(err.to_string().contains("boom"));
    }
}
