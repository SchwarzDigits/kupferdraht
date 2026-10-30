// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Storage public contract — the record types and the `Store` trait.
//!
//! This module deliberately has no `SQLite` dependency. Any alternative
//! backend (in-memory mock, remote RPC, …) can implement `Store` by
//! operating purely on these records.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::KupferdrahtError;
use crate::storage::MaybeSendSync;
use crate::storage::engine::StorageEngineInfo;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRecord {
    pub user_id: String,
    /// Wire `client_id` of the device this session is bound to. Step
    /// 7e pinning: every authenticated call routes through the
    /// `(access_token, device_id)` pair, and `Client::resume` /
    /// `Client::resync_existing_rooms` reload the matching MLS state
    /// from `devices.mls_public_key` keyed by this id.
    pub device_id: String,
    pub access_token: String,
    pub refresh_token: String,
    pub cookie_label: Option<String>,
    pub api_version: u32,
    pub access_token_issued_at_ms: i64,
}

impl std::fmt::Debug for SessionRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Step 101: secrets redacted (`crate::redact`).
        f.debug_struct("SessionRecord")
            .field("user_id", &self.user_id)
            .field("device_id", &self.device_id)
            .field("access_token", &crate::redact::Hidden)
            .field("refresh_token", &crate::redact::Hidden)
            .field("cookie_label", &self.cookie_label)
            .field("api_version", &self.api_version)
            .field("access_token_issued_at_ms", &self.access_token_issued_at_ms)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceRecord {
    pub user_id: String,
    pub client_id: String,
    /// MLS signature public key, base64-encoded. Populated at login
    /// from Step 1c.6 onward; `None` between Step 1b registration and
    /// Step 1c `KeyPackage` upload in legacy rows.
    pub mls_public_key: Option<String>,
}

/// The user's own profile as loaded at login from `GET /v14/self`
/// and persisted so a subsequent process can reconstruct `Account`
/// without hitting the backend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelfUserRecord {
    /// Qualified `<localpart>@<domain>` user id (Step 19). The domain is
    /// folded in here; there is no separate (nullable) `domain` column.
    pub id: String,
    pub handle: Option<String>,
    pub name: String,
    pub email: Option<String>,
}

/// The `ServerConfig` values Kupferdraht booted with, persisted so
/// `ClientBuilder::resume` can rebuild `HttpClient` without being
/// re-told which backend to talk to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerConfigRecord {
    pub api_base_url: String,
    pub websocket_url: String,
    pub title: String,
}

/// One row of `backend_mls_keys` — a Wire-backend-scoped public key
/// for a specific `(purpose, signature_scheme)` pair. Today Kupferdraht
/// uses exactly one entry, `("removal", "ecdsa_secp256r1_sha256")`,
/// for the `external_senders` extension in every MLS group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendMlsKey {
    pub purpose: String,
    pub signature_scheme: String,
    pub public_key: Vec<u8>,
}

/// One row of `rooms`. `room_type` is the stringified [`crate::RoomType`]
/// (`"direct_message"` / `"group"` / `"channel"` / `"self_room"`).
/// `last_synced_at_ms` is Unix-epoch milliseconds set by the caller
/// at save time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomRecord {
    pub room_id: String,
    pub group_id: Option<String>,
    pub name: String,
    pub room_type: String,
    pub epoch: i64,
    pub counterpart_user_id: Option<String>,
    pub last_synced_at_ms: i64,
}

/// One row of `room_members`. Members load back with the room they
/// belong to; `replace_rooms` replaces the whole (rooms, members)
/// snapshot atomically per sync.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomMemberRecord {
    pub room_id: String,
    pub user_id: String,
    pub is_self: bool,
    pub display_name: Option<String>,
    pub handle: Option<String>,
    /// Neutral role vocabulary: `"administrator"` | `"member"`
    /// (Step 72a, V26). Unknown strings read back as member. The
    /// serde default covers `IndexedDB` rows written before 72a —
    /// `SQLite` gets the same via V26's `DEFAULT 'member'`.
    #[serde(default = "default_member_role")]
    pub role: String,
}

fn default_member_role() -> String {
    "member".to_string()
}

/// Singleton `sync_state` row — one per device. Step 5b reads /
/// writes this around the WebSocket loop so a reconnect resumes
/// from the last persisted Wire `synchronization` marker without
/// re-reading already-acked events.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncStateRecord {
    pub sync_marker_uuid: Option<String>,
    pub last_event_id: Option<String>,
    pub last_connected_at_ms: Option<i64>,
    /// Unix-ms of the last key-package pool check (Step 63) — the
    /// 24-h throttle stamp for the sync-surface auto-replenishment.
    pub last_key_package_check_at_ms: Option<i64>,
}

/// The account's recovery state (Step 112, ADR-0035): recovery data
/// exists since `exported_at_ms`, and `pin_verifier` checks the current
/// PIN locally. Singleton row; only the browser writes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryStateRecord {
    pub exported_at_ms: i64,
    pub pin_verifier: [u8; 32],
}

/// One row of `mentions`. Step 6-II-g — one (message, span,
/// user) tuple. Mentions belong to a specific version of the
/// body; on edit they are replaced wholesale.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MentionRecord {
    pub message_id: String,
    pub room_id: String,
    pub user_id: String,
    pub start: i32,
    pub length: i32,
}

/// One `MessageEdit` for [`Store::apply_message_edit_with_mentions`]
/// (Step 105). `message_id` is the edited row's stable primary key — the
/// key its mentions live under — resolved **before** the edit advances
/// the row's chain pointer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageEditRecord {
    pub room_id: String,
    pub message_id: String,
    pub replacing_message_id: String,
    pub edit_event_id: String,
    pub new_body: String,
    pub edited_at_ms: i64,
}

/// Whether `e` is the store's "row already exists" failure — the text
/// the `SQLite` store's constraint error carries. Only the default
/// implementations of the combined writes use it (Step 105); the
/// `SQLite` store checks the constraint code itself.
fn is_existing_row_error(e: &KupferdrahtError) -> bool {
    matches!(e, KupferdrahtError::Storage(msg) if msg.contains("UNIQUE constraint failed"))
}

/// One row of `read_receipts`. Step 6-II-f — represents
/// one `(message, reader)` ack. Wire's `Confirmation { type:
/// READ }` event fans out into N rows when it carries N
/// `message_ids` in a single batch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadReceiptRecord {
    pub message_id: String,
    pub room_id: String,
    pub reader_user_id: String,
    pub confirmed_at_ms: i64,
}

/// One row of `delivery_receipts`. Step 25b — represents one
/// `(message, confirmer)` ack that the recipient's device received the
/// message. Wire's `Confirmation { type: DELIVERED }` event fans out into
/// N rows when it carries N `message_ids` in a single batch. Mirror of
/// [`ReadReceiptRecord`]; the two lists are independent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryReceiptRecord {
    pub message_id: String,
    pub room_id: String,
    pub confirmer_user_id: String,
    pub confirmed_at_ms: i64,
}

/// One row of `reactions`. Step 6-II-e — represents one
/// (message, sender, emoji) tuple. Wire's `Reaction` payload
/// is the sender's full set; Kupferdraht decomposes it into one row
/// per emoji so per-message aggregations are cheap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReactionRecord {
    pub message_id: String,
    pub room_id: String,
    pub sender_user_id: String,
    pub emoji: String,
    pub sent_at_ms: i64,
}

/// One row of `timeline_events`. Step 4 writes outgoing text rows
/// only (`kind = "text"`, `delivery_status = "submitted"`); Step 5
/// extends with incoming rows and upgrades submitted →
/// acknowledged. The `body` field is the `GenericMessage.Text.content`
/// plaintext, not the MLS ciphertext.
///
/// `reply_to_id` is set when the item carries a
/// `Text.quote.quoted_message_id` (Step 6-II-b). It is a soft
/// reference; the parent row may legitimately not (yet) be in
/// the store.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimelineEventRecord {
    pub message_id: String,
    pub room_id: String,
    pub sender_user_id: String,
    pub sender_device_id: String,
    pub kind: String,
    pub body: String,
    pub sent_at_ms: i64,
    pub delivery_status: String,
    pub reply_to_id: Option<String>,
    /// Wall-clock at which the most recent `MessageEdit` replaced
    /// the body, in milliseconds since Unix epoch. `None` for an
    /// unedited row. Step 6-II-c.
    pub edited_at_ms: Option<i64>,
    /// Wall-clock at which an inbound or outbound `MessageDelete`
    /// retired the row, in milliseconds since Unix epoch. `None`
    /// for a live row. The `body` is cleared once this is set;
    /// `reply_to_id` references that point at this row still
    /// resolve so consumers can render "(quoting deleted message)"
    /// rather than orphaning the reply. Step 6-II-d.
    pub redacted_at_ms: Option<i64>,
    /// Wall-clock at which this row's ephemeral timer elapses, in
    /// milliseconds since Unix epoch — `Some` only for a
    /// self-deleting message (Wire's `Ephemeral` wrapper, field 18).
    /// The timer starts at dispatch/send time: `sent_at_ms + ttl` on
    /// both the send and the receive path. The stamp survives the
    /// tombstone so an expiry stays distinguishable from a
    /// `MessageDelete`. `None` for every ordinary row. Step 89.
    #[serde(default)]
    pub expires_at_ms: Option<i64>,
    /// The TTL the sender chose for an OWN ephemeral send, in
    /// milliseconds — persisted so a `Failed` row (which carries no
    /// `expires_at_ms`, Step 89) can be re-driven under the same timer
    /// by `Timeline::resend` (Step 79, V28). `None` for every other
    /// row, inbound ephemerals included (their stamp is derived at
    /// dispatch and an inbound row is never resent).
    #[serde(default)]
    pub ephemeral_ttl_ms: Option<i64>,
}

/// One row retired by the ephemeral sweep (`Store::expire_ephemeral_messages`)
/// — the keys the caller needs to emit the tombstone `TimelineDiff`, plus
/// the sender so the caller can tell an inbound row (broadcast the
/// expiry as a delete-for-everyone, V1b) from an own one (local only).
/// Step 89.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpiredMessageRecord {
    pub room_id: String,
    pub message_id: String,
    /// Qualified user id of the row's sender.
    pub sender_user_id: String,
}

/// One row of `feature_configs` — a single Wire team-feature-config
/// entry. Persisted by Step 10's login pipeline integration and by the
/// `feature-config.update` event arm; consumed by `Client::feature_config`
/// and the typed getters layered over it (`Client::file_sharing`, …).
///
/// `status` is nullable because not every Wire feature config carries a
/// `status` field — some are config-only. `config_json` is the raw
/// `config` object as a JSON string, or `"{}"` when the feature ships
/// status-only.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, Serialize, Deserialize)]
pub struct FeatureConfigEntry {
    pub name: String,
    pub status: Option<String>,
    pub config_json: String,
    pub last_synced_at_ms: i64,
}

/// One row of `user_properties` — a single Wire user-property entry.
/// Persisted by Step 10's login pipeline integration and by the
/// `user.properties-set` / `user.properties-delete` event arms;
/// consumed by `Client::user_property` and the typed getters layered
/// over it (`Client::receipt_mode`, …).
///
/// `value` is the raw response body of `GET /v14/properties/{key}` —
/// opaque to the storage layer. Wire mixes JSON-encoded strings (e.g.
/// `"\"on\""`) with raw numeric strings (e.g. `"1"`) here; typed
/// getters strip optional surrounding quotes before parsing.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, Serialize, Deserialize)]
pub struct UserPropertyEntry {
    pub key: String,
    pub value: String,
    pub last_synced_at_ms: i64,
}

/// Persisted asset attachment, anchored to a `timeline_events` row
/// by `(message_id, room_id)`. Step 11d (ADR-0011 §Storage shape).
///
/// `otr_key_ct` holds the 32-byte AES key the asset content was
/// encrypted with for Wire. It is stored verbatim — the
/// `attachments.otr_key` column lives in the whole-DB-encrypted
/// `kupferdraht.db`, so the key is protected at rest by the database
/// encryption rather than a second per-field wrap.
///
/// `cache_path` is `None` when the encrypted ciphertext blob is not
/// in the per-account cache. Set to an absolute filesystem path
/// when the encrypted blob has been downloaded (or, on send-side,
/// moved out of the encrypt-step tempfile and into the cache).
///
/// `encryption_algo` is `"AES_CBC"` for Kupferdraht-emitted assets and
/// for the vast majority of inbound assets. `"AES_GCM"` rows are
/// kept for diagnostics but cannot be decrypted by Kupferdraht until a
/// future ADR pins the GCM byte layout (per ADR-0011).
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachmentRecord {
    pub message_id: String,
    pub room_id: String,
    pub mime_type: String,
    pub filename: Option<String>,
    pub size_bytes: u64,
    pub sha256: Vec<u8>,
    pub otr_key_ct: Vec<u8>,
    pub asset_id: String,
    pub asset_token: Option<String>,
    pub asset_domain: String,
    pub encryption_algo: String,
    pub cache_path: Option<String>,
    pub created_at_ms: i64,
}

impl std::fmt::Debug for AttachmentRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Step 101: secrets redacted (`crate::redact`).
        f.debug_struct("AttachmentRecord")
            .field("message_id", &self.message_id)
            .field("room_id", &self.room_id)
            .field("mime_type", &self.mime_type)
            .field("filename", &self.filename)
            .field("size_bytes", &self.size_bytes)
            .field("sha256", &self.sha256)
            .field("otr_key_ct", &crate::redact::byte_count(&self.otr_key_ct))
            .field("asset_id", &self.asset_id)
            .field(
                "asset_token",
                &crate::redact::hidden_option(self.asset_token.as_ref()),
            )
            .field("asset_domain", &self.asset_domain)
            .field("encryption_algo", &self.encryption_algo)
            .field("cache_path", &self.cache_path)
            .field("created_at_ms", &self.created_at_ms)
            .finish()
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
pub trait Store: MaybeSendSync {
    /// Engine, cipher scheme and header mode of the backend's whole-DB
    /// encryption, read off the live connection (Step 90, ADR-0021) —
    /// what `kupferdraht-cli status` prints. `Ok(None)` for a backend without
    /// such an engine: the default, so custom `Store` implementations
    /// and the browser backend need no change (the task-method pattern).
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` if the engine cannot be queried.
    async fn storage_engine(&self) -> Result<Option<StorageEngineInfo>, KupferdrahtError> {
        Ok(None)
    }

    /// Persist a session record, replacing any existing row for the
    /// same `user_id`.
    ///
    /// # Errors
    /// Returns `KupferdrahtError::Storage` on `SQLite` failure.
    async fn save_session(&self, session: &SessionRecord) -> Result<(), KupferdrahtError>;

    /// Load the session for the given `user_id`, or `Ok(None)` if no
    /// session is stored for that user.
    ///
    /// # Errors
    /// Returns `KupferdrahtError::Storage` on `SQLite` failure and
    /// `KupferdrahtError::Encoding` if a token column holds non-UTF-8 bytes.
    async fn load_session(&self, user_id: &str) -> Result<Option<SessionRecord>, KupferdrahtError>;

    /// Persist a device record for the given user, replacing any
    /// existing row.
    ///
    /// # Errors
    /// Returns `KupferdrahtError::Storage` on `SQLite` failure.
    async fn save_device(&self, device: &DeviceRecord) -> Result<(), KupferdrahtError>;

    /// Load the device record for the `(user_id, client_id)` pair,
    /// or `Ok(None)` if no row exists. The per-`client_id` lookup is
    /// the only correct shape: a single user can own up to 7 Wire
    /// clients (the per-user device cap), so picking "any" device by
    /// `user_id` alone returned arbitrary rows and silently broke
    /// `Client::resume` (CHANGELOG.md §29). Callers consume
    /// `SessionRecord::device_id` to source the second argument.
    ///
    /// # Errors
    /// Returns `KupferdrahtError::Storage` on `SQLite` failure.
    async fn load_device(
        &self,
        user_id: &str,
        client_id: &str,
    ) -> Result<Option<DeviceRecord>, KupferdrahtError>;

    /// Persist a backend MLS public key, replacing any existing row
    /// for the same `(purpose, signature_scheme)` pair.
    ///
    /// # Errors
    /// Returns `KupferdrahtError::Storage` on `SQLite` failure.
    async fn save_backend_mls_key(&self, key: &BackendMlsKey) -> Result<(), KupferdrahtError>;

    /// Load the backend MLS key for the given
    /// `(purpose, signature_scheme)` pair, or `Ok(None)` if no row
    /// exists.
    ///
    /// # Errors
    /// Returns `KupferdrahtError::Storage` on `SQLite` failure.
    async fn load_backend_mls_key(
        &self,
        purpose: &str,
        signature_scheme: &str,
    ) -> Result<Option<BackendMlsKey>, KupferdrahtError>;

    /// Replace the entire rooms snapshot atomically. Every existing
    /// row in `rooms` and `room_members` is dropped and the supplied
    /// snapshot inserted in its place. This matches
    /// `Client::sync_metadata`'s full-refresh semantics.
    ///
    /// # Errors
    /// Returns `KupferdrahtError::Storage` on `SQLite` failure.
    async fn replace_rooms(
        &self,
        rooms: &[(RoomRecord, Vec<RoomMemberRecord>)],
    ) -> Result<(), KupferdrahtError>;

    /// Clear all per-account messaging state (rooms, members, timeline + its
    /// reaction/mention/receipt/attachment overlays, and the sync cursor).
    /// Used when a re-login registers a new device that supersedes a gone one
    /// (Step 40): the new device starts clean and re-joins every room. Leaves
    /// session / device / self-user / server-config / feature-config rows
    /// intact — login re-establishes those.
    ///
    /// # Errors
    /// Returns `KupferdrahtError::Storage` on `SQLite` failure.
    async fn clear_messaging_state(&self) -> Result<(), KupferdrahtError>;

    /// Load every room + its members from storage.
    ///
    /// # Errors
    /// Returns `KupferdrahtError::Storage` on `SQLite` failure and
    /// `KupferdrahtError::Encoding` if a `_ct` column holds non-UTF-8
    /// bytes.
    async fn load_rooms(
        &self,
    ) -> Result<Vec<(RoomRecord, Vec<RoomMemberRecord>)>, KupferdrahtError>;

    /// Bump the `epoch` column of a single `rooms` row. Used by
    /// `Client::join_room` (Step 3) after a successful external
    /// commit so a process restart reads the post-join epoch rather
    /// than the pre-join server epoch.
    ///
    /// # Errors
    /// Returns `KupferdrahtError::Storage` on `SQLite` failure. A missing
    /// `room_id` is treated as a no-op (the in-memory cache is
    /// authoritative; the store may have been replaced between
    /// `sync_metadata` and the join).
    async fn update_room_epoch(
        &self,
        room_id: &str,
        new_epoch: i64,
    ) -> Result<(), KupferdrahtError>;

    /// Replace the `room_members` rows for a single `room_id`
    /// atomically. Used by `Client::sync_members` (Step 3d) after
    /// re-deriving the member list from the local MLS group's leaves.
    /// The `rooms` row itself is untouched.
    ///
    /// # Errors
    /// Returns `KupferdrahtError::Storage` on `SQLite` failure.
    async fn replace_room_members(
        &self,
        room_id: &str,
        members: &[RoomMemberRecord],
    ) -> Result<(), KupferdrahtError>;

    /// Insert a single `timeline_events` row. Step 4 writes outgoing
    /// rows after `Timeline::send_text` succeeds; Step 5 extends to
    /// incoming rows. The `(message_id, room_id)` composite primary
    /// key prevents accidental duplicates if the caller retries.
    ///
    /// # Errors
    /// Returns `KupferdrahtError::Storage` on `SQLite` failure.
    async fn insert_timeline_event(
        &self,
        event: &TimelineEventRecord,
    ) -> Result<(), KupferdrahtError>;

    /// Insert a text row and its mentions as **one** write (Step 105):
    /// either both land or neither. `Ok(false)` when the row already
    /// exists — nothing is written then (an own send's echo, a replayed
    /// notification).
    ///
    /// The default writes in sequence and is **not** atomic; a store that
    /// can, overrides it (the `SQLite` store does).
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` on a failed write.
    async fn insert_timeline_event_with_mentions(
        &self,
        event: &TimelineEventRecord,
        mentions: &[MentionRecord],
    ) -> Result<bool, KupferdrahtError> {
        match self.insert_timeline_event(event).await {
            Ok(()) => {}
            Err(e) if is_existing_row_error(&e) => return Ok(false),
            Err(e) => return Err(e),
        }
        self.replace_mentions(&event.room_id, &event.message_id, mentions)
            .await?;
        Ok(true)
    }

    /// Insert an asset row and its `attachments` row as **one** write
    /// (Step 105); `Ok(false)` when the row already exists (nothing
    /// written). The default writes in sequence and is not atomic.
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` on a failed write.
    async fn insert_timeline_event_with_attachment(
        &self,
        event: &TimelineEventRecord,
        attachment: &AttachmentRecord,
    ) -> Result<bool, KupferdrahtError> {
        match self.insert_timeline_event(event).await {
            Ok(()) => {}
            Err(e) if is_existing_row_error(&e) => return Ok(false),
            Err(e) => return Err(e),
        }
        self.insert_attachment(attachment).await?;
        Ok(true)
    }

    /// Insert a task row and upsert its materialized fold row as **one**
    /// write (Step 105); `Ok(false)` when the row already exists (nothing
    /// written — a replayed old task message must not regress the fold).
    /// The default writes in sequence and is not atomic.
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` on a failed write.
    async fn insert_timeline_event_with_task(
        &self,
        event: &TimelineEventRecord,
        task: &TaskRecord,
    ) -> Result<bool, KupferdrahtError> {
        match self.insert_timeline_event(event).await {
            Ok(()) => {}
            Err(e) if is_existing_row_error(&e) => return Ok(false),
            Err(e) => return Err(e),
        }
        self.upsert_task(task).await?;
        Ok(true)
    }

    /// Apply a `MessageEdit` and replace the row's mentions (under
    /// `edit.message_id`) as **one** write (Step 105). Returns whether the
    /// edit applied — `false` (nothing written) as in
    /// [`Store::apply_message_edit`]. The default writes in sequence and is
    /// not atomic.
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` on a failed write.
    async fn apply_message_edit_with_mentions(
        &self,
        edit: &MessageEditRecord,
        mentions: &[MentionRecord],
    ) -> Result<bool, KupferdrahtError> {
        let applied = self
            .apply_message_edit(
                &edit.room_id,
                &edit.replacing_message_id,
                &edit.edit_event_id,
                &edit.new_body,
                edit.edited_at_ms,
            )
            .await?;
        if applied {
            self.replace_mentions(&edit.room_id, &edit.message_id, mentions)
                .await?;
        }
        Ok(applied)
    }

    /// Load every persisted timeline event for a room ordered
    /// `sent_at_ms` ascending. Empty when the room has no recorded
    /// activity. Used by the CLI for an inspection view; Step 5 will
    /// extend this with paging.
    ///
    /// # Errors
    /// Returns `KupferdrahtError::Storage` on `SQLite` failure or
    /// `KupferdrahtError::Encoding` if a `_ct` column holds non-UTF-8
    /// bytes (today they're plaintext).
    async fn load_timeline_events(
        &self,
        room_id: &str,
    ) -> Result<Vec<TimelineEventRecord>, KupferdrahtError>;

    /// Look up the rooms a given `target_id` participates in, where
    /// `target_id` may match either a stable `message_id` or an
    /// edit-event id stored in `current_edit_event_id` (Wire's edit
    /// chain). Returns an empty vec when the id is not in the local
    /// store; returns multiple entries when the same id was seen in
    /// multiple rooms (composite-PK schema permits it though `UUIDv4`
    /// makes it astronomically unlikely in practice).
    ///
    /// Powers the CLI room-resolution sugar for `mark-read` / `react`
    /// / `delete-message` / `edit` / `reply` — pass only `--message`
    /// and the CLI looks up the room from the local timeline before
    /// driving the underlying room-scoped MLS operation.
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` on `SQLite` read failure.
    async fn find_message_room(&self, target_id: &str) -> Result<Vec<String>, KupferdrahtError>;

    /// Load a single timeline row by id, matching either a stable
    /// `message_id` or an edit-event id stored in
    /// `current_edit_event_id` (Wire's edit chain), mirroring
    /// [`Store::find_message_room`]'s id resolution. Returns `None`
    /// when the id is not in the local store.
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` on `SQLite` read failure;
    /// `KupferdrahtError::Encoding` if the `body` column holds non-UTF-8
    /// bytes (today they're plaintext).
    async fn load_timeline_event(
        &self,
        target_id: &str,
    ) -> Result<Option<TimelineEventRecord>, KupferdrahtError>;

    /// [`Store::load_timeline_event`] restricted to one room — the
    /// target lookup for an inbound edit / delete (Step 95b, ADR-0028):
    /// timeline rows are keyed `(message_id, room_id)`, so the same id
    /// can exist in two rooms. The default filters the room-agnostic
    /// lookup, which keeps a custom `Store` compiling (custom-store
    /// pattern); the shipped store overrides it with a room-scoped read.
    ///
    /// # Errors
    /// As [`Store::load_timeline_event`].
    async fn load_timeline_event_in_room(
        &self,
        room_id: &str,
        target_id: &str,
    ) -> Result<Option<TimelineEventRecord>, KupferdrahtError> {
        Ok(self
            .load_timeline_event(target_id)
            .await?
            .filter(|record| record.room_id == room_id))
    }

    /// The device set last persisted for `room_id` (Step 95d) — the
    /// baseline a device-change diff is computed against. `None` when
    /// no baseline exists yet. Default `Ok(None)` keeps a custom `Store`
    /// compiling; without persistence no change is ever reported.
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` on backend failure.
    async fn load_room_devices(
        &self,
        _room_id: &str,
    ) -> Result<Option<Vec<crate::rooms::RoomDevice>>, KupferdrahtError> {
        Ok(None)
    }

    /// Replace the persisted device set of `room_id` (Step 95d).
    /// Default no-op (custom-store pattern).
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` on backend failure.
    async fn replace_room_devices(
        &self,
        _room_id: &str,
        _devices: &[crate::rooms::RoomDevice],
    ) -> Result<(), KupferdrahtError> {
        Ok(())
    }

    /// Persist the device-local read marker for a room (Step 67):
    /// everything with `sent_at_ms <= at_ms` counts as read. Default
    /// no-op keeps a custom `Store` compiling (custom-store pattern).
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` on backend failure.
    async fn set_read_marker(&self, _room_id: &str, _at_ms: i64) -> Result<(), KupferdrahtError> {
        Ok(())
    }

    /// The room's read marker, `None` when never marked (Step 67 —
    /// no marker means everything inbound counts as unread).
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` on backend failure.
    async fn read_marker(&self, _room_id: &str) -> Result<Option<i64>, KupferdrahtError> {
        Ok(None)
    }

    /// Unread count for one room (Step 67) — same semantics as
    /// [`Store::unread_counts`], scoped. Default `0` (custom-store pattern).
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` on backend failure.
    async fn unread_count_for_room(
        &self,
        _room_id: &str,
        _self_user_id: &str,
    ) -> Result<u32, KupferdrahtError> {
        Ok(0)
    }

    /// Per-room unread counts in one grouped query (Step 67): inbound
    /// rows (`sender_user_id != self_user_id`, not redacted) newer
    /// than each room's marker; rooms without unread rows are absent.
    /// Default empty (custom-store pattern).
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` on backend failure.
    async fn unread_counts(
        &self,
        _self_user_id: &str,
    ) -> Result<std::collections::HashMap<String, u32>, KupferdrahtError> {
        Ok(std::collections::HashMap::new())
    }

    /// Upgrade a `submitted` row to `received` when the Wire echo of
    /// an own message arrives (Step 65, review S12 — `Submitted` is
    /// documented as "awaiting the Wire echo", but the echo never
    /// upgraded it). Returns `true` when a row was upgraded, `false`
    /// when the row is absent or already past `submitted`. Default
    /// no-op (`Ok(false)`) keeps non-SQLite backends compiling until
    /// they implement it — same pattern as the task methods.
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` on backend failure.
    async fn mark_delivery_received(
        &self,
        _room_id: &str,
        _message_id: &str,
    ) -> Result<bool, KupferdrahtError> {
        Ok(false)
    }

    /// Flip a `failed` row back to `submitted` after a successful
    /// resend (Step 79): re-stamp `sent_at_ms` to the resend instant
    /// and set `expires_at_ms` (the re-armed ephemeral timer; `None`
    /// for an ordinary row). The WHERE guard pins the update to a live
    /// `failed` row — `true` when a row flipped, `false` when nothing
    /// matched. Default `Ok(false)` keeps custom backends compiling
    /// (task-method pattern); the core treats `false` as a storage
    /// error after the upload, never as a silent success.
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` on backend failure.
    async fn mark_message_resent(
        &self,
        _room_id: &str,
        _message_id: &str,
        _sent_at_ms: i64,
        _expires_at_ms: Option<i64>,
    ) -> Result<bool, KupferdrahtError> {
        Ok(false)
    }

    /// Settle an own send after its upload (Step 97a): move a `sending`
    /// row to `submitted` (`delivered`) or `failed`, setting
    /// `expires_at_ms` when given (a delivered ephemeral send). The
    /// WHERE guard pins the update to a `sending` row — an echo that
    /// already marked it `received` is never downgraded. `true` when a
    /// row changed. Default `Ok(false)` keeps custom backends compiling
    /// (task-method pattern); the core then leaves the row `sending`,
    /// which reads as `Failed` once the send is over.
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` on backend failure.
    async fn mark_send_outcome(
        &self,
        _room_id: &str,
        _message_id: &str,
        _delivered: bool,
        _expires_at_ms: Option<i64>,
    ) -> Result<bool, KupferdrahtError> {
        Ok(false)
    }

    /// Apply a `MessageEdit` to the row matching `replacing_message_id`
    /// (either the row's original `message_id` or its
    /// `current_edit_event_id` chain pointer): replace the body,
    /// stamp `edited_at_ms`, and remember `edit_event_id` as the
    /// next chain pointer. Step 6-II-c.
    ///
    /// Wire's edit semantics chain — each successive edit references
    /// the previous edit-event's `message_id` rather than the
    /// original message's id (observations.md §34). Kupferdraht keeps the row's
    /// PK stable and tracks the chain in `current_edit_event_id`.
    ///
    /// Idempotent on redelivery: the SQL guard
    /// `(edited_at_ms IS NULL OR edited_at_ms < ?)` blocks a
    /// redelivered older edit from rolling back a newer one.
    ///
    /// Returns `Ok(true)` when a row was updated, `Ok(false)` when
    /// the parent isn't reachable locally (no row with the matching
    /// id or chain pointer) or the edit was older than the
    /// persisted one.
    ///
    /// # Errors
    /// Returns `KupferdrahtError::Storage` on `SQLite` failure.
    async fn apply_message_edit(
        &self,
        room_id: &str,
        replacing_message_id: &str,
        edit_event_id: &str,
        new_body: &str,
        edited_at_ms: i64,
    ) -> Result<bool, KupferdrahtError>;

    /// Replace the mention list for a single message wholesale:
    /// DELETE every existing `mentions` row for `(room_id,
    /// message_id)` then INSERT one row per supplied `mention`.
    /// Step 6-II-g. Mentions are replaced (not merged) on every
    /// inbound text or edit because Wire's `Text.mentions` is the
    /// authoritative full set for that body version.
    ///
    /// # Errors
    /// Returns `KupferdrahtError::Storage` on `SQLite` failure.
    async fn replace_mentions(
        &self,
        room_id: &str,
        message_id: &str,
        mentions: &[MentionRecord],
    ) -> Result<(), KupferdrahtError>;

    /// Load every mention row for a room. Step 6-II-g. Used by
    /// `Client::messages` to overlay `TimelineItem::kind::Text::mentions`.
    ///
    /// # Errors
    /// Returns `KupferdrahtError::Storage` on `SQLite` failure.
    async fn load_mentions_for_room(
        &self,
        room_id: &str,
    ) -> Result<Vec<MentionRecord>, KupferdrahtError>;

    /// Apply a batch of read receipts: for every `message_id` in
    /// `message_ids`, INSERT one row into `read_receipts` keyed by
    /// `(message_id, reader_user_id)`. Idempotent on redelivery
    /// via `INSERT OR IGNORE` on the composite PK. Step 6-II-f.
    ///
    /// Returns the number of rows actually inserted (existing
    /// `(message_id, reader_user_id)` pairs are skipped).
    ///
    /// # Errors
    /// Returns `KupferdrahtError::Storage` on `SQLite` failure.
    async fn apply_read_receipts(
        &self,
        room_id: &str,
        reader_user_id: &str,
        message_ids: &[String],
        confirmed_at_ms: i64,
    ) -> Result<usize, KupferdrahtError>;

    /// Load every read-receipt row for a room. Step 6-II-f. Used
    /// by `Client::messages` to overlay `TimelineItem::read_by`.
    ///
    /// # Errors
    /// Returns `KupferdrahtError::Storage` on `SQLite` failure.
    async fn load_read_receipts_for_room(
        &self,
        room_id: &str,
    ) -> Result<Vec<ReadReceiptRecord>, KupferdrahtError>;

    /// Apply an inbound `Confirmation { type: DELIVERED }` batch — one row
    /// per `(message_id, confirmer_user_id)`, `INSERT OR IGNORE`. Step 25b.
    /// Returns the number of rows actually inserted.
    ///
    /// # Errors
    /// Returns `KupferdrahtError::Storage` on `SQLite` failure.
    async fn apply_delivery_receipts(
        &self,
        room_id: &str,
        confirmer_user_id: &str,
        message_ids: &[String],
        confirmed_at_ms: i64,
    ) -> Result<usize, KupferdrahtError>;

    /// Load every delivery-receipt row for a room. Step 25b. Used to overlay
    /// `TimelineItem::delivered_to`.
    ///
    /// # Errors
    /// Returns `KupferdrahtError::Storage` on `SQLite` failure.
    async fn load_delivery_receipts_for_room(
        &self,
        room_id: &str,
    ) -> Result<Vec<DeliveryReceiptRecord>, KupferdrahtError>;

    /// Resolve `target_id` to the canonical `timeline_events.message_id`
    /// of the row that matches via either the stable PK or the V12
    /// chain pointer (`current_edit_event_id`). Returns `None` when
    /// no row matches. Step 6-II-e.
    ///
    /// # Errors
    /// Returns `KupferdrahtError::Storage` on `SQLite` failure.
    async fn canonical_message_id(
        &self,
        room_id: &str,
        target_id: &str,
    ) -> Result<Option<String>, KupferdrahtError>;

    /// Replace the sender's full set of reactions on a single
    /// message with `emojis`. Step 6-II-e. Mirrors Wire's
    /// "Reaction is the new full state, not a delta" semantic.
    ///
    /// The implementation is `DELETE … WHERE (message_id,
    /// sender_user_id)` followed by per-emoji INSERTs in one
    /// transaction. An empty `emojis` slice clears the user's
    /// reactions on this message. Idempotent on redelivery —
    /// re-applying the same set produces the same final state.
    ///
    /// `canonical_message_id` is the row's stable PK in
    /// `timeline_events` (caller resolves chain pointer first
    /// via the same `(message_id = ? OR current_edit_event_id =
    /// ?)` lookup as `apply_message_edit` / `apply_message_delete`).
    /// Storing reactions under the canonical id keeps the user's
    /// reaction set consistent across an edit chain.
    ///
    /// # Errors
    /// Returns `KupferdrahtError::Storage` on `SQLite` failure.
    async fn replace_user_reactions(
        &self,
        room_id: &str,
        canonical_message_id: &str,
        sender_user_id: &str,
        emojis: &[String],
        sent_at_ms: i64,
    ) -> Result<(), KupferdrahtError>;

    /// Load every reaction row for a room, grouped by `message_id`.
    /// Used by `Client::messages` to overlay reactions onto the
    /// returned `TimelineItem`s. Step 6-II-e.
    ///
    /// # Errors
    /// Returns `KupferdrahtError::Storage` on `SQLite` failure.
    async fn load_reactions_for_room(
        &self,
        room_id: &str,
    ) -> Result<Vec<ReactionRecord>, KupferdrahtError>;

    /// Apply a `MessageDelete` to the row matching `target_message_id`
    /// (either the row's original `message_id` or its
    /// `current_edit_event_id` chain pointer): clear the body and
    /// stamp `redacted_at_ms`. Step 6-II-d.
    ///
    /// Idempotent on redelivery: the SQL guard
    /// `(redacted_at_ms IS NULL OR redacted_at_ms < ?)` blocks a
    /// redelivered older delete from re-firing.
    ///
    /// Returns `Ok(true)` when a row was updated, `Ok(false)` when
    /// the parent isn't reachable locally (no row with the matching
    /// id or chain pointer) or the delete was older than the
    /// persisted one.
    ///
    /// # Errors
    /// Returns `KupferdrahtError::Storage` on `SQLite` failure.
    async fn apply_message_delete(
        &self,
        room_id: &str,
        target_message_id: &str,
        redacted_at_ms: i64,
    ) -> Result<bool, KupferdrahtError>;

    /// Tombstone every live row whose ephemeral timer has elapsed
    /// (`expires_at_ms <= now_ms`, not yet redacted): clear the body
    /// and stamp `redacted_at_ms = now_ms` — the same local tombstone
    /// an inbound `MessageDelete` produces, so reply chains keep
    /// resolving. Returns the retired `(room_id, message_id)` keys so
    /// the caller can emit one `TimelineDiff::UpdateAt` each.
    /// Idempotent: a second sweep at the same instant returns empty.
    /// Default no-op (empty) keeps a custom `Store` compiling until it
    /// implements it (the task-method pattern); the shipped store
    /// implements it on every target. Step 89.
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` on backend failure.
    async fn expire_ephemeral_messages(
        &self,
        _now_ms: i64,
    ) -> Result<Vec<ExpiredMessageRecord>, KupferdrahtError> {
        Ok(Vec::new())
    }

    /// The distinct future expiry instants (`expires_at_ms > now_ms`)
    /// of live ephemeral rows, ascending — what a resuming client
    /// re-arms its local timers from. Default empty (custom-store
    /// pattern). Step 89.
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` on backend failure.
    async fn pending_ephemeral_expiries(&self, _now_ms: i64) -> Result<Vec<i64>, KupferdrahtError> {
        Ok(Vec::new())
    }

    /// Persist the singleton `sync_state` row. Step 5b writes this
    /// after each `synchronization` frame so a reconnect resumes
    /// from the last cursor.
    ///
    /// # Errors
    /// Returns `KupferdrahtError::Storage` on `SQLite` failure.
    async fn save_sync_state(&self, state: &SyncStateRecord) -> Result<(), KupferdrahtError>;

    /// Load the singleton `sync_state` row, or `Ok(None)` if no
    /// stream has ever connected on this device.
    ///
    /// # Errors
    /// Returns `KupferdrahtError::Storage` on `SQLite` failure.
    async fn load_sync_state(&self) -> Result<Option<SyncStateRecord>, KupferdrahtError>;

    /// Persist the account's recovery state (Step 112, ADR-0035),
    /// replacing the singleton row.
    ///
    /// # Errors
    /// Returns `KupferdrahtError::Storage` on `SQLite` failure.
    async fn save_recovery_state(
        &self,
        state: &RecoveryStateRecord,
    ) -> Result<(), KupferdrahtError>;

    /// Load the account's recovery state, or `Ok(None)` while no recovery
    /// data was made.
    ///
    /// # Errors
    /// Returns `KupferdrahtError::Storage` on `SQLite` failure.
    async fn load_recovery_state(&self) -> Result<Option<RecoveryStateRecord>, KupferdrahtError>;

    /// Close the database (Step 114): a purge deletes it next. Every later
    /// operation refuses with `SessionExpired`. The default does nothing —
    /// a store without a database to close.
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` when the database cannot be closed.
    fn close(&self) -> Result<(), KupferdrahtError> {
        Ok(())
    }

    /// Persist the user's own profile, replacing any existing row
    /// keyed by `id`.
    ///
    /// # Errors
    /// Returns `KupferdrahtError::Storage` on `SQLite` failure.
    async fn save_self_user(&self, user: &SelfUserRecord) -> Result<(), KupferdrahtError>;

    /// Load the self-user profile for the given `id`, or `Ok(None)`.
    ///
    /// # Errors
    /// Returns `KupferdrahtError::Storage` on `SQLite` failure and
    /// `KupferdrahtError::Encoding` if a `_ct` column holds non-UTF-8
    /// bytes.
    async fn load_self_user(&self, id: &str) -> Result<Option<SelfUserRecord>, KupferdrahtError>;

    /// Persist the singleton `server_config` row.
    ///
    /// # Errors
    /// Returns `KupferdrahtError::Storage` on `SQLite` failure.
    async fn save_server_config(&self, config: &ServerConfigRecord)
    -> Result<(), KupferdrahtError>;

    /// Load the singleton `server_config` row, or `Ok(None)` if
    /// nothing has been written yet.
    ///
    /// # Errors
    /// Returns `KupferdrahtError::Storage` on `SQLite` failure.
    async fn load_server_config(&self) -> Result<Option<ServerConfigRecord>, KupferdrahtError>;

    /// Replace the entire `feature_configs` snapshot atomically.
    /// Every existing row is dropped and `entries` inserted in its
    /// place — matches the `GET /v14/feature-configs` whole-team
    /// response shape that Step 10c's login pipeline and Step 10d's
    /// `feature-config.update` event arm both use. An empty slice
    /// clears the table.
    ///
    /// # Errors
    /// Returns `KupferdrahtError::Storage` on `SQLite` failure.
    async fn replace_feature_configs(
        &self,
        entries: &[FeatureConfigEntry],
    ) -> Result<(), KupferdrahtError>;

    /// Upsert one feature-config row keyed by `entry.name`. Used
    /// when a future event ships a single named feature config
    /// without forcing a whole-table refresh. Step 10d uses
    /// `replace_feature_configs` for `feature-config.update`
    /// because Wire's API is whole-response — `upsert` is included
    /// for symmetry with `upsert_user_property`.
    ///
    /// # Errors
    /// Returns `KupferdrahtError::Storage` on `SQLite` failure.
    async fn upsert_feature_config(
        &self,
        entry: &FeatureConfigEntry,
    ) -> Result<(), KupferdrahtError>;

    /// Load every persisted feature-config row. Order is unspecified
    /// — callers index by `name`. Used by Step 10e's
    /// `Client::feature_configs`.
    ///
    /// # Errors
    /// Returns `KupferdrahtError::Storage` on `SQLite` failure.
    async fn list_feature_configs(&self) -> Result<Vec<FeatureConfigEntry>, KupferdrahtError>;

    /// Load one feature-config row by `name`, or `Ok(None)` if no
    /// row matches. Used by Step 10e's `Client::feature_config`
    /// and the typed getters layered over it
    /// (`Client::file_sharing`).
    ///
    /// # Errors
    /// Returns `KupferdrahtError::Storage` on `SQLite` failure.
    async fn get_feature_config(
        &self,
        name: &str,
    ) -> Result<Option<FeatureConfigEntry>, KupferdrahtError>;

    /// Replace the entire `user_properties` snapshot atomically.
    /// Step 10c's login pipeline calls this once after fetching
    /// `GET /v14/properties` + per-key values. An empty slice
    /// clears the table.
    ///
    /// # Errors
    /// Returns `KupferdrahtError::Storage` on `SQLite` failure.
    async fn replace_user_properties(
        &self,
        entries: &[UserPropertyEntry],
    ) -> Result<(), KupferdrahtError>;

    /// Upsert one user-property row keyed by `entry.key`. Step
    /// 10d's `user.properties-set` event arm calls this after
    /// re-fetching the new value from `GET /v14/properties/{key}`.
    ///
    /// # Errors
    /// Returns `KupferdrahtError::Storage` on `SQLite` failure.
    async fn upsert_user_property(&self, entry: &UserPropertyEntry)
    -> Result<(), KupferdrahtError>;

    /// Delete the `user_properties` row for `key`. Idempotent on a
    /// missing key — Wire's `user.properties-delete` event arm
    /// (Step 10d) routes to this method, and the local store may
    /// already be in sync (e.g. the row was cleared by an earlier
    /// `replace_user_properties` call). Returning an error in that
    /// case would force the dispatch to log noise.
    ///
    /// # Errors
    /// Returns `KupferdrahtError::Storage` on `SQLite` failure.
    async fn delete_user_property(&self, key: &str) -> Result<(), KupferdrahtError>;

    /// Load every persisted user-property row. Order is unspecified
    /// — callers index by `key`. Used by Step 10e's
    /// `Client::user_properties`.
    ///
    /// # Errors
    /// Returns `KupferdrahtError::Storage` on `SQLite` failure.
    async fn list_user_properties(&self) -> Result<Vec<UserPropertyEntry>, KupferdrahtError>;

    /// Load one user-property row by `key`, or `Ok(None)` if no
    /// row matches. Used by Step 10e's `Client::user_property`
    /// and the typed getters layered over it
    /// (`Client::receipt_mode`).
    ///
    /// # Errors
    /// Returns `KupferdrahtError::Storage` on `SQLite` failure.
    async fn get_user_property(
        &self,
        key: &str,
    ) -> Result<Option<UserPropertyEntry>, KupferdrahtError>;

    /// Insert an asset attachment row, anchored to its parent
    /// `timeline_events` row by `(message_id, room_id)`. Caller is
    /// responsible for inserting the parent `timeline_events` row
    /// first — the FK rejects orphan attachments. Step 11d.
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` on `SQLite` failure (FK violation,
    /// duplicate primary key, etc.).
    async fn insert_attachment(&self, record: &AttachmentRecord) -> Result<(), KupferdrahtError>;

    /// Look up the attachment row for `(message_id, room_id)`.
    /// `Ok(None)` when the message has no asset (pure-text message).
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` on `SQLite` failure.
    async fn get_attachment(
        &self,
        message_id: &str,
        room_id: &str,
    ) -> Result<Option<AttachmentRecord>, KupferdrahtError>;

    /// List every attachment for a room, ordered by `created_at_ms`
    /// ascending (oldest first — matches `timeline_events` ordering).
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` on `SQLite` failure.
    async fn list_attachments_for_room(
        &self,
        room_id: &str,
    ) -> Result<Vec<AttachmentRecord>, KupferdrahtError>;

    /// Set or clear the local-cache path on an attachment row.
    /// `Some(path)` when the encrypted ciphertext has been moved
    /// into the per-account cache; `None` when the cache entry
    /// has been evicted.
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` on `SQLite` failure or no-op when
    /// the row is absent (silent — the contract is "set the path
    /// if the row exists").
    async fn update_attachment_cache_path(
        &self,
        message_id: &str,
        room_id: &str,
        cache_path: Option<&str>,
    ) -> Result<(), KupferdrahtError>;

    /// Remove an attachment row. Idempotent — deleting a non-
    /// existent row is a no-op. Step 11d.
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` on `SQLite` failure.
    async fn delete_attachment(
        &self,
        message_id: &str,
        room_id: &str,
    ) -> Result<(), KupferdrahtError>;

    // ── Tasks — materialized fold cache (Step 60c, ADR-0014) ────────
    //
    // The `tasks` table holds one row per `(room_id, task_id)` — the
    // latest folded state, maintained on every task-message persist
    // and rebuilt per room after a redaction. The methods below have
    // **default no-op bodies** so stores that have not wired task
    // views yet (IndexedDB/WASM — see open.md) keep compiling: they
    // report zero tasks rather than erroring.

    /// Insert or update one materialized task row. On conflict of
    /// `(room_id, task_id)` every state field is replaced **except**
    /// `creator_user_id` / `created_at_ms` (first writer wins).
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` on `SQLite` failure.
    async fn upsert_task(&self, task: &TaskRecord) -> Result<(), KupferdrahtError> {
        let _ = task;
        Ok(())
    }

    /// Replace ALL materialized task rows of one room (rebuild after
    /// a redaction, or backfill after the V23 migration).
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` on `SQLite` failure.
    async fn replace_room_tasks(
        &self,
        room_id: &str,
        tasks: &[TaskRecord],
    ) -> Result<(), KupferdrahtError> {
        let _ = (room_id, tasks);
        Ok(())
    }

    /// All materialized task rows of one room.
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` on `SQLite` failure.
    async fn load_tasks_for_room(
        &self,
        room_id: &str,
    ) -> Result<Vec<TaskRecord>, KupferdrahtError> {
        let _ = room_id;
        Ok(Vec::new())
    }

    /// Cross-room "my open work" rows: `assignee` matches and
    /// `status` is `open` or `in_progress` (index-backed).
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` on `SQLite` failure.
    async fn load_open_tasks_for_assignee(
        &self,
        assignee: &str,
    ) -> Result<Vec<TaskRecord>, KupferdrahtError> {
        let _ = assignee;
        Ok(Vec::new())
    }

    /// Cross-room "what I delegated" rows: `creator_user_id` matches
    /// (any status — filtering by assignee happens in the timeline
    /// layer).
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` on `SQLite` failure.
    async fn load_tasks_for_creator(
        &self,
        creator_user_id: &str,
    ) -> Result<Vec<TaskRecord>, KupferdrahtError> {
        let _ = creator_user_id;
        Ok(Vec::new())
    }

    /// Number of materialized task rows — the backfill trigger reads
    /// this once per resume.
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` on `SQLite` failure.
    async fn count_tasks(&self) -> Result<u64, KupferdrahtError> {
        Ok(0)
    }

    /// Every `kind = 'task'` timeline row across all rooms, in
    /// arrival order per room — the backfill source after the V23
    /// migration.
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` on `SQLite` failure.
    async fn load_task_timeline_rows(&self) -> Result<Vec<TimelineEventRecord>, KupferdrahtError> {
        Ok(Vec::new())
    }

    /// Open the transaction of one drained page (Step 109, ADR-0037): the
    /// drain's writes until [`Self::commit_page`] land together or not at
    /// all, and a write from outside the drain waits until the page ends.
    /// Called only by the drain, inside `storage::page_scope`.
    ///
    /// The default groups nothing: every write commits on its own, as
    /// before — correct, only slower on remote storage. A store that can,
    /// overrides the five page methods together (the `SQLite` store does).
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` when the transaction cannot begin.
    async fn begin_page(&self) -> Result<(), KupferdrahtError> {
        Ok(())
    }

    /// Mark the start of the page's notification `index`, so that
    /// [`Self::rollback_to_savepoint`] can undo it alone.
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` on `SQLite` failure.
    async fn savepoint(&self, index: u32) -> Result<(), KupferdrahtError> {
        let _ = index;
        Ok(())
    }

    /// Undo everything the page wrote since [`Self::savepoint`]`(index)`;
    /// what came before stays in the page.
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` on `SQLite` failure.
    async fn rollback_to_savepoint(&self, index: u32) -> Result<(), KupferdrahtError> {
        let _ = index;
        Ok(())
    }

    /// Commit the page. Writes that waited for it go on afterwards.
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` when the commit fails; the page is rolled
    /// back then.
    async fn commit_page(&self) -> Result<(), KupferdrahtError> {
        Ok(())
    }

    /// Roll the page back (a no-op without one). Writes that waited for it
    /// go on afterwards. Synchronous, so that a dropped page can roll back.
    fn rollback_page(&self) {}
}

/// One materialized task row (Step 60c, ADR-0014): the folded state
/// of one task plus the fold metadata. `status` / `priority` travel
/// as the same strings the `timeline_events` JSON body uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRecord {
    pub room_id: String,
    pub task_id: String,
    pub title: String,
    pub description: Option<String>,
    pub assignee: Option<String>,
    pub due_at_ms: Option<i64>,
    pub status: String,
    pub priority: Option<String>,
    pub sort_key: Option<String>,
    pub creator_user_id: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub latest_message_id: String,
}
