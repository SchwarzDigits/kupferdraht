// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `SqliteStore` — the canonical [`Store`] implementation, backed by
//! `rusqlite` and the refinery migrations under
//! `src/storage/migrations/`.
//!
//! Per-table CRUD lives in sibling submodules (`session`,
//! `self_user`, `server_config`, `devices`, `backend_mls_keys`,
//! `rooms`, `timeline_events`, `mentions`, `reactions`,
//! `read_receipts`, `sync_state`). Each submodule is a focused
//! ~50-200 LOC unit with its own `#[cfg(test)]` block colocated
//! with the SQL it covers — that's the audit-deferred per-table
//! split shipping here. The single `impl Store for SqliteStore`
//! block at the bottom of this file delegates to each submodule's
//! `pub(super)` free functions so Rust's "one trait-impl per
//! type" rule is satisfied without splintering across files.
//!
//! `SqliteStore` itself and the connection accessor ([`Self::lock`])
//! live here because they are cross-cutting. The store opens **ready**
//! (keyed at `open`) — there is no locked/unlocked connection
//! lifecycle; the concrete store owns its encryption.
//! Whole-DB encryption (`SQLite3MC`, ChaCha20-Poly1305 — ADR-0021)
//! protects every column at rest, so the store layer reads / writes
//! column values verbatim — there is no per-field encrypt/decrypt step.
//! The same store runs in the browser (Step 91, ADR-0022): `path` is
//! then a file name in the OPFS `sahpool` pool, and the journal stays a
//! rollback journal (the VFS has no shared-memory WAL).

mod attachments;
mod backend_mls_keys;
mod delivery_receipts;
mod devices;
mod engine;
mod event_writes;
mod feature_configs;
mod mentions;
mod page;
mod reactions;
mod read_markers;
mod read_receipts;
mod recovery_state;
mod room_devices;
mod rooms;
mod self_user;
mod server_config;
mod session;
mod sync_state;
mod tasks;
mod timeline_events;
mod user_properties;
mod wipe;
mod write_tx;

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use rusqlite::Connection;

use crate::error::KupferdrahtError;
use crate::storage::StorageEngineInfo;
use crate::storage::records::{
    AttachmentRecord, BackendMlsKey, DeliveryReceiptRecord, DeviceRecord, ExpiredMessageRecord,
    FeatureConfigEntry, MentionRecord, ReactionRecord, ReadReceiptRecord, RecoveryStateRecord,
    RoomMemberRecord, RoomRecord, SelfUserRecord, ServerConfigRecord, SessionRecord, Store,
    SyncStateRecord, TimelineEventRecord, UserPropertyEntry,
};
use crate::storage::schema::run_migrations;

pub struct SqliteStore {
    /// The keyed `rusqlite` connection. The store is opened **ready**:
    /// the concrete `SQLite` backend applies its whole-DB key and runs
    /// migrations at construction ([`Self::open`]), so there is no
    /// locked state and the connection is always present.
    connection: Mutex<Connection>,
    /// Closed by a purge (Step 114): every later operation is refused.
    closed: AtomicBool,
    /// The database's path: in the browser the key of its VFS, whose
    /// failure every operation checks first (Step 111b).
    #[cfg(target_arch = "wasm32")]
    path: std::path::PathBuf,
    /// The page gate (Step 109, ADR-0037): a drained page holds it
    /// exclusively from `begin_page` to its commit or rollback; every
    /// write from outside the page takes it shared, so it waits for the
    /// page and a page waits for writes already running.
    page_gate: Arc<tokio::sync::RwLock<()>>,
    /// The open page's hold on the gate.
    page: Mutex<Option<tokio::sync::OwnedRwLockWriteGuard<()>>>,
}

/// RAII accessor over the connection, returned by [`SqliteStore::lock`].
/// Every per-table CRUD helper holds one briefly via `self.lock()?`.
pub struct ConnGuard<'a> {
    guard: std::sync::MutexGuard<'a, Connection>,
}

impl std::ops::Deref for ConnGuard<'_> {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        &self.guard
    }
}

impl std::ops::DerefMut for ConnGuard<'_> {
    fn deref_mut(&mut self) -> &mut Connection {
        &mut self.guard
    }
}

impl SqliteStore {
    /// Open the whole-DB-encrypted database at `path`, ready to use: apply
    /// the per-database [`DbKey`](crate::storage::db_key::DbKey)
    /// (`SQLite3MC` chacha20 scheme, plaintext header 32, raw key — the
    /// key verified by a first read), switch to WAL, set the multi-process
    /// pragmas (`busy_timeout`, `synchronous = FULL` — Step 85) and run
    /// migrations. The concrete store owns its encryption — it is opened
    /// ready, with no separate key step the `Store` trait must know about.
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` if the file cannot be opened, the key does
    /// not open it (a wrong key fails here, at open — Step 90), or
    /// migrations fail.
    pub fn open(
        path: &Path,
        key: &crate::storage::db_key::DbKey,
    ) -> Result<Self, KupferdrahtError> {
        let mut conn = crate::storage::open_connection(path)?;
        crate::storage::apply_whole_db_key(&conn, key)?;
        // WAL is native-only (ADR-0022 decision 5): the browser's VFS provides
        // no shared memory — it keeps SQLite's rollback journal.
        #[cfg(not(target_arch = "wasm32"))]
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| KupferdrahtError::Storage(format!("set journal_mode=WAL: {e}")))?;
        // Step 85: busy_timeout + explicit synchronous=FULL (native).
        #[cfg(not(target_arch = "wasm32"))]
        crate::storage::set_multi_process_pragmas(&conn)?;
        conn.pragma_update(None, "foreign_keys", "ON")
            .map_err(|e| KupferdrahtError::Storage(format!("set foreign_keys=ON: {e}")))?;
        run_migrations(&mut conn)?;
        Ok(Self {
            connection: Mutex::new(conn),
            closed: AtomicBool::new(false),
            #[cfg(target_arch = "wasm32")]
            path: path.to_path_buf(),
            page_gate: Arc::new(tokio::sync::RwLock::new(())),
            page: Mutex::new(None),
        })
    }

    /// Wait until a write may run (Step 109, ADR-0037): immediately inside
    /// the drain's page scope, otherwise once no page is open. The permit is
    /// held for the write.
    async fn write_permit(&self) -> Option<tokio::sync::OwnedRwLockReadGuard<()>> {
        if crate::storage::page::in_page_scope() {
            return None;
        }
        Some(self.page_gate.clone().read_owned().await)
    }

    /// Acquire the underlying `rusqlite::Connection`. Exposed for
    /// integration tests that assert on raw column shapes; not part of
    /// the matrix-rust-sdk-shaped public API. Hold the guard briefly.
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` if the inner mutex is poisoned;
    /// `SessionExpired` once a purge closed the store (Step 114); in the
    /// browser `Evicted` once the instance stopped, `ServiceUnavailable` /
    /// `StorageAccessDenied` once the database's VFS reported it failed.
    pub fn lock(&self) -> Result<ConnGuard<'_>, KupferdrahtError> {
        // Step 92 (ADR-0023): an evicted browser instance must not touch the
        // files another tab is about to open — every operation fails closed.
        // Step 111b: a takeover by another instance stops the instance.
        // Step 114: a purged account's store stays closed.
        if self.closed.load(Ordering::Acquire) {
            return Err(KupferdrahtError::SessionExpired);
        }
        let guard = self
            .connection
            .lock()
            .map_err(|e| KupferdrahtError::Storage(format!("mutex poisoned: {e}")))?;
        // Step 113 (ADR-0038): while the storage server is unreachable a
        // cheap read lets the VFS heal the database, or the operation fails
        // at once.
        #[cfg(target_arch = "wasm32")]
        crate::storage::browser::check_database(&self.path, || {
            guard
                .query_row("SELECT count(*) FROM sqlite_master", [], |row| {
                    row.get::<_, i64>(0)
                })
                .is_ok()
        })?;
        Ok(ConnGuard { guard })
    }
}

/// Decode a column read as `Vec<u8>` into a `String`, surfacing
/// invalid UTF-8 as `KupferdrahtError::Encoding` rather than a panic.
/// Shared by every per-table assembler that hits a `_ct` column.
pub(super) fn utf8_column(bytes: Vec<u8>, column: &str) -> Result<String, KupferdrahtError> {
    String::from_utf8(bytes).map_err(|e| {
        KupferdrahtError::Encoding(format!("storage column `{column}` is not valid UTF-8: {e}"))
    })
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl Store for SqliteStore {
    async fn storage_engine(&self) -> Result<Option<StorageEngineInfo>, KupferdrahtError> {
        engine::storage_engine(self).map(Some)
    }

    async fn begin_page(&self) -> Result<(), KupferdrahtError> {
        let hold = self.page_gate.clone().write_owned().await;
        page::begin_page(self, hold)
    }

    async fn savepoint(&self, index: u32) -> Result<(), KupferdrahtError> {
        page::savepoint(self, index)
    }

    async fn rollback_to_savepoint(&self, index: u32) -> Result<(), KupferdrahtError> {
        page::rollback_to_savepoint(self, index)
    }

    async fn commit_page(&self) -> Result<(), KupferdrahtError> {
        page::commit_page(self)
    }

    fn rollback_page(&self) {
        page::rollback_page(self);
    }

    async fn save_session(&self, session_record: &SessionRecord) -> Result<(), KupferdrahtError> {
        let _permit = self.write_permit().await;
        session::save_session(self, session_record)
    }

    async fn load_session(&self, user_id: &str) -> Result<Option<SessionRecord>, KupferdrahtError> {
        session::load_session(self, user_id)
    }

    async fn save_device(&self, device: &DeviceRecord) -> Result<(), KupferdrahtError> {
        let _permit = self.write_permit().await;
        devices::save_device(self, device)
    }

    async fn load_device(
        &self,
        user_id: &str,
        client_id: &str,
    ) -> Result<Option<DeviceRecord>, KupferdrahtError> {
        devices::load_device(self, user_id, client_id)
    }

    async fn save_backend_mls_key(&self, key: &BackendMlsKey) -> Result<(), KupferdrahtError> {
        let _permit = self.write_permit().await;
        backend_mls_keys::save_backend_mls_key(self, key)
    }

    async fn load_backend_mls_key(
        &self,
        purpose: &str,
        signature_scheme: &str,
    ) -> Result<Option<BackendMlsKey>, KupferdrahtError> {
        backend_mls_keys::load_backend_mls_key(self, purpose, signature_scheme)
    }

    async fn replace_rooms(
        &self,
        rooms_in: &[(RoomRecord, Vec<RoomMemberRecord>)],
    ) -> Result<(), KupferdrahtError> {
        let _permit = self.write_permit().await;
        rooms::replace_rooms(self, rooms_in)
    }

    async fn clear_messaging_state(&self) -> Result<(), KupferdrahtError> {
        let _permit = self.write_permit().await;
        wipe::clear_messaging_state(self)
    }

    async fn replace_room_members(
        &self,
        room_id: &str,
        members: &[RoomMemberRecord],
    ) -> Result<(), KupferdrahtError> {
        let _permit = self.write_permit().await;
        rooms::replace_room_members(self, room_id, members)
    }

    async fn update_room_epoch(
        &self,
        room_id: &str,
        new_epoch: i64,
    ) -> Result<(), KupferdrahtError> {
        let _permit = self.write_permit().await;
        rooms::update_room_epoch(self, room_id, new_epoch)
    }

    async fn load_rooms(
        &self,
    ) -> Result<Vec<(RoomRecord, Vec<RoomMemberRecord>)>, KupferdrahtError> {
        rooms::load_rooms(self)
    }

    async fn save_self_user(&self, user: &SelfUserRecord) -> Result<(), KupferdrahtError> {
        let _permit = self.write_permit().await;
        self_user::save_self_user(self, user)
    }

    async fn load_self_user(&self, id: &str) -> Result<Option<SelfUserRecord>, KupferdrahtError> {
        self_user::load_self_user(self, id)
    }

    async fn save_server_config(
        &self,
        config: &ServerConfigRecord,
    ) -> Result<(), KupferdrahtError> {
        let _permit = self.write_permit().await;
        server_config::save_server_config(self, config)
    }

    async fn load_server_config(&self) -> Result<Option<ServerConfigRecord>, KupferdrahtError> {
        server_config::load_server_config(self)
    }

    async fn mark_delivery_received(
        &self,
        room_id: &str,
        message_id: &str,
    ) -> Result<bool, KupferdrahtError> {
        let _permit = self.write_permit().await;
        timeline_events::mark_delivery_received(self, room_id, message_id)
    }

    async fn mark_message_resent(
        &self,
        room_id: &str,
        message_id: &str,
        sent_at_ms: i64,
        expires_at_ms: Option<i64>,
    ) -> Result<bool, KupferdrahtError> {
        let _permit = self.write_permit().await;
        timeline_events::mark_message_resent(self, room_id, message_id, sent_at_ms, expires_at_ms)
    }

    async fn mark_send_outcome(
        &self,
        room_id: &str,
        message_id: &str,
        delivered: bool,
        expires_at_ms: Option<i64>,
    ) -> Result<bool, KupferdrahtError> {
        let _permit = self.write_permit().await;
        timeline_events::mark_send_outcome(self, room_id, message_id, delivered, expires_at_ms)
    }

    async fn set_read_marker(&self, room_id: &str, at_ms: i64) -> Result<(), KupferdrahtError> {
        let _permit = self.write_permit().await;
        read_markers::set_read_marker(self, room_id, at_ms)
    }

    async fn read_marker(&self, room_id: &str) -> Result<Option<i64>, KupferdrahtError> {
        read_markers::read_marker(self, room_id)
    }

    async fn unread_count_for_room(
        &self,
        room_id: &str,
        self_user_id: &str,
    ) -> Result<u32, KupferdrahtError> {
        read_markers::unread_count_for_room(self, room_id, self_user_id)
    }

    async fn unread_counts(
        &self,
        self_user_id: &str,
    ) -> Result<std::collections::HashMap<String, u32>, KupferdrahtError> {
        read_markers::unread_counts(self, self_user_id)
    }

    async fn insert_timeline_event(
        &self,
        event: &TimelineEventRecord,
    ) -> Result<(), KupferdrahtError> {
        let _permit = self.write_permit().await;
        timeline_events::insert_timeline_event(self, event)
    }

    async fn insert_timeline_event_with_mentions(
        &self,
        event: &TimelineEventRecord,
        mentions: &[MentionRecord],
    ) -> Result<bool, KupferdrahtError> {
        let _permit = self.write_permit().await;
        event_writes::insert_timeline_event_with_mentions(self, event, mentions)
    }

    async fn insert_timeline_event_with_attachment(
        &self,
        event: &TimelineEventRecord,
        attachment: &AttachmentRecord,
    ) -> Result<bool, KupferdrahtError> {
        let _permit = self.write_permit().await;
        event_writes::insert_timeline_event_with_attachment(self, event, attachment)
    }

    async fn insert_timeline_event_with_task(
        &self,
        event: &TimelineEventRecord,
        task: &crate::storage::TaskRecord,
    ) -> Result<bool, KupferdrahtError> {
        let _permit = self.write_permit().await;
        event_writes::insert_timeline_event_with_task(self, event, task)
    }

    async fn apply_message_edit_with_mentions(
        &self,
        edit: &crate::storage::MessageEditRecord,
        mentions: &[MentionRecord],
    ) -> Result<bool, KupferdrahtError> {
        let _permit = self.write_permit().await;
        event_writes::apply_message_edit_with_mentions(self, edit, mentions)
    }

    async fn upsert_task(&self, task: &crate::storage::TaskRecord) -> Result<(), KupferdrahtError> {
        let _permit = self.write_permit().await;
        tasks::upsert_task(self, task)
    }

    async fn replace_room_tasks(
        &self,
        room_id: &str,
        records: &[crate::storage::TaskRecord],
    ) -> Result<(), KupferdrahtError> {
        let _permit = self.write_permit().await;
        tasks::replace_room_tasks(self, room_id, records)
    }

    async fn load_tasks_for_room(
        &self,
        room_id: &str,
    ) -> Result<Vec<crate::storage::TaskRecord>, KupferdrahtError> {
        tasks::load_tasks_for_room(self, room_id)
    }

    async fn load_open_tasks_for_assignee(
        &self,
        assignee: &str,
    ) -> Result<Vec<crate::storage::TaskRecord>, KupferdrahtError> {
        tasks::load_open_tasks_for_assignee(self, assignee)
    }

    async fn load_tasks_for_creator(
        &self,
        creator_user_id: &str,
    ) -> Result<Vec<crate::storage::TaskRecord>, KupferdrahtError> {
        tasks::load_tasks_for_creator(self, creator_user_id)
    }

    async fn count_tasks(&self) -> Result<u64, KupferdrahtError> {
        tasks::count_tasks(self)
    }

    async fn load_task_timeline_rows(&self) -> Result<Vec<TimelineEventRecord>, KupferdrahtError> {
        tasks::load_task_timeline_rows(self)
    }

    async fn save_sync_state(&self, state: &SyncStateRecord) -> Result<(), KupferdrahtError> {
        let _permit = self.write_permit().await;
        sync_state::save_sync_state(self, state)
    }

    async fn load_sync_state(&self) -> Result<Option<SyncStateRecord>, KupferdrahtError> {
        sync_state::load_sync_state(self)
    }

    async fn save_recovery_state(
        &self,
        state: &RecoveryStateRecord,
    ) -> Result<(), KupferdrahtError> {
        let _permit = self.write_permit().await;
        recovery_state::save_recovery_state(self, state)
    }

    async fn load_recovery_state(&self) -> Result<Option<RecoveryStateRecord>, KupferdrahtError> {
        recovery_state::load_recovery_state(self)
    }

    /// Swap the database's connection for an empty in-memory one — which
    /// closes the database — and refuse every later operation.
    fn close(&self) -> Result<(), KupferdrahtError> {
        let mut guard = self
            .connection
            .lock()
            .map_err(|e| KupferdrahtError::Storage(format!("mutex poisoned: {e}")))?;
        self.closed.store(true, Ordering::Release);
        let empty = Connection::open_in_memory()
            .map_err(|e| KupferdrahtError::Storage(format!("close: an in-memory stand-in: {e}")))?;
        let open = std::mem::replace(&mut *guard, empty);
        open.close()
            .map_err(|(_, e)| KupferdrahtError::Storage(format!("close: {e}")))?;
        debug_assert!(self.closed.load(Ordering::Acquire), "closed for good");
        Ok(())
    }

    async fn load_timeline_events(
        &self,
        room_id: &str,
    ) -> Result<Vec<TimelineEventRecord>, KupferdrahtError> {
        timeline_events::load_timeline_events(self, room_id)
    }

    async fn find_message_room(&self, target_id: &str) -> Result<Vec<String>, KupferdrahtError> {
        timeline_events::find_message_room(self, target_id)
    }

    async fn load_timeline_event(
        &self,
        target_id: &str,
    ) -> Result<Option<TimelineEventRecord>, KupferdrahtError> {
        timeline_events::load_timeline_event(self, target_id)
    }

    async fn load_timeline_event_in_room(
        &self,
        room_id: &str,
        target_id: &str,
    ) -> Result<Option<TimelineEventRecord>, KupferdrahtError> {
        timeline_events::load_timeline_event_in_room(self, room_id, target_id)
    }

    async fn load_room_devices(
        &self,
        room_id: &str,
    ) -> Result<Option<Vec<crate::rooms::RoomDevice>>, KupferdrahtError> {
        room_devices::load_room_devices(self, room_id)
    }

    async fn replace_room_devices(
        &self,
        room_id: &str,
        devices: &[crate::rooms::RoomDevice],
    ) -> Result<(), KupferdrahtError> {
        let _permit = self.write_permit().await;
        room_devices::replace_room_devices(self, room_id, devices)
    }

    async fn replace_mentions(
        &self,
        room_id: &str,
        message_id: &str,
        mentions_in: &[MentionRecord],
    ) -> Result<(), KupferdrahtError> {
        let _permit = self.write_permit().await;
        mentions::replace_mentions(self, room_id, message_id, mentions_in)
    }

    async fn load_mentions_for_room(
        &self,
        room_id: &str,
    ) -> Result<Vec<MentionRecord>, KupferdrahtError> {
        mentions::load_mentions_for_room(self, room_id)
    }

    async fn apply_read_receipts(
        &self,
        room_id: &str,
        reader_user_id: &str,
        message_ids: &[String],
        confirmed_at_ms: i64,
    ) -> Result<usize, KupferdrahtError> {
        let _permit = self.write_permit().await;
        read_receipts::apply_read_receipts(
            self,
            room_id,
            reader_user_id,
            message_ids,
            confirmed_at_ms,
        )
    }

    async fn load_read_receipts_for_room(
        &self,
        room_id: &str,
    ) -> Result<Vec<ReadReceiptRecord>, KupferdrahtError> {
        read_receipts::load_read_receipts_for_room(self, room_id)
    }

    async fn apply_delivery_receipts(
        &self,
        room_id: &str,
        confirmer_user_id: &str,
        message_ids: &[String],
        confirmed_at_ms: i64,
    ) -> Result<usize, KupferdrahtError> {
        let _permit = self.write_permit().await;
        delivery_receipts::apply_delivery_receipts(
            self,
            room_id,
            confirmer_user_id,
            message_ids,
            confirmed_at_ms,
        )
    }

    async fn load_delivery_receipts_for_room(
        &self,
        room_id: &str,
    ) -> Result<Vec<DeliveryReceiptRecord>, KupferdrahtError> {
        delivery_receipts::load_delivery_receipts_for_room(self, room_id)
    }

    async fn canonical_message_id(
        &self,
        room_id: &str,
        target_id: &str,
    ) -> Result<Option<String>, KupferdrahtError> {
        timeline_events::canonical_message_id(self, room_id, target_id)
    }

    async fn replace_user_reactions(
        &self,
        room_id: &str,
        canonical_message_id: &str,
        sender_user_id: &str,
        emojis: &[String],
        sent_at_ms: i64,
    ) -> Result<(), KupferdrahtError> {
        let _permit = self.write_permit().await;
        reactions::replace_user_reactions(
            self,
            room_id,
            canonical_message_id,
            sender_user_id,
            emojis,
            sent_at_ms,
        )
    }

    async fn load_reactions_for_room(
        &self,
        room_id: &str,
    ) -> Result<Vec<ReactionRecord>, KupferdrahtError> {
        reactions::load_reactions_for_room(self, room_id)
    }

    async fn apply_message_delete(
        &self,
        room_id: &str,
        target_message_id: &str,
        redacted_at_ms: i64,
    ) -> Result<bool, KupferdrahtError> {
        let _permit = self.write_permit().await;
        timeline_events::apply_message_delete(self, room_id, target_message_id, redacted_at_ms)
    }

    async fn expire_ephemeral_messages(
        &self,
        now_ms: i64,
    ) -> Result<Vec<ExpiredMessageRecord>, KupferdrahtError> {
        let _permit = self.write_permit().await;
        timeline_events::expire_ephemeral_messages(self, now_ms)
    }

    async fn pending_ephemeral_expiries(&self, now_ms: i64) -> Result<Vec<i64>, KupferdrahtError> {
        timeline_events::pending_ephemeral_expiries(self, now_ms)
    }

    async fn apply_message_edit(
        &self,
        room_id: &str,
        replacing_message_id: &str,
        edit_event_id: &str,
        new_body: &str,
        edited_at_ms: i64,
    ) -> Result<bool, KupferdrahtError> {
        let _permit = self.write_permit().await;
        timeline_events::apply_message_edit(
            self,
            room_id,
            replacing_message_id,
            edit_event_id,
            new_body,
            edited_at_ms,
        )
    }

    async fn replace_feature_configs(
        &self,
        entries: &[FeatureConfigEntry],
    ) -> Result<(), KupferdrahtError> {
        let _permit = self.write_permit().await;
        feature_configs::replace_feature_configs(self, entries)
    }

    async fn upsert_feature_config(
        &self,
        entry: &FeatureConfigEntry,
    ) -> Result<(), KupferdrahtError> {
        let _permit = self.write_permit().await;
        feature_configs::upsert_feature_config(self, entry)
    }

    async fn list_feature_configs(&self) -> Result<Vec<FeatureConfigEntry>, KupferdrahtError> {
        feature_configs::list_feature_configs(self)
    }

    async fn get_feature_config(
        &self,
        name: &str,
    ) -> Result<Option<FeatureConfigEntry>, KupferdrahtError> {
        feature_configs::get_feature_config(self, name)
    }

    async fn replace_user_properties(
        &self,
        entries: &[UserPropertyEntry],
    ) -> Result<(), KupferdrahtError> {
        let _permit = self.write_permit().await;
        user_properties::replace_user_properties(self, entries)
    }

    async fn upsert_user_property(
        &self,
        entry: &UserPropertyEntry,
    ) -> Result<(), KupferdrahtError> {
        let _permit = self.write_permit().await;
        user_properties::upsert_user_property(self, entry)
    }

    async fn delete_user_property(&self, key: &str) -> Result<(), KupferdrahtError> {
        let _permit = self.write_permit().await;
        user_properties::delete_user_property(self, key)
    }

    async fn list_user_properties(&self) -> Result<Vec<UserPropertyEntry>, KupferdrahtError> {
        user_properties::list_user_properties(self)
    }

    async fn get_user_property(
        &self,
        key: &str,
    ) -> Result<Option<UserPropertyEntry>, KupferdrahtError> {
        user_properties::get_user_property(self, key)
    }

    async fn insert_attachment(&self, record: &AttachmentRecord) -> Result<(), KupferdrahtError> {
        let _permit = self.write_permit().await;
        attachments::insert_attachment(self, record)
    }

    async fn get_attachment(
        &self,
        message_id: &str,
        room_id: &str,
    ) -> Result<Option<AttachmentRecord>, KupferdrahtError> {
        attachments::get_attachment(self, message_id, room_id)
    }

    async fn list_attachments_for_room(
        &self,
        room_id: &str,
    ) -> Result<Vec<AttachmentRecord>, KupferdrahtError> {
        attachments::list_attachments_for_room(self, room_id)
    }

    async fn update_attachment_cache_path(
        &self,
        message_id: &str,
        room_id: &str,
        cache_path: Option<&str>,
    ) -> Result<(), KupferdrahtError> {
        let _permit = self.write_permit().await;
        attachments::update_attachment_cache_path(self, message_id, room_id, cache_path)
    }

    async fn delete_attachment(
        &self,
        message_id: &str,
        room_id: &str,
    ) -> Result<(), KupferdrahtError> {
        let _permit = self.write_permit().await;
        attachments::delete_attachment(self, message_id, room_id)
    }
}

/// Fixed key material (a raw 32-byte key) used to open test stores.
/// Whole-DB encryption means a [`DbKey`](crate::storage::db_key::DbKey)
/// must be supplied at `open`, so every fixture opens with this value.
#[cfg(test)]
pub(super) fn test_db_key() -> crate::storage::db_key::DbKey {
    crate::storage::db_key::DbKey::from_keystore_bytes(&[0x42; 32]).expect("32-byte test material")
}

/// Test fixture used by every per-table `#[cfg(test)] mod tests`.
/// Centralised here so each submodule's tests share the same
/// open-on-tempdir setup without duplicating the boilerplate. The store
/// is opened **ready** (keyed) — there is no separate unlock step.
#[cfg(test)]
pub(super) fn fresh_store() -> (tempfile::TempDir, SqliteStore) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("kupferdraht.db");
    let store = SqliteStore::open(&path, &test_db_key()).expect("open");
    (dir, store)
}

/// Invalid UTF-8 byte sequence — lone continuation bytes that
/// cannot appear in any valid UTF-8 string. Shared between the
/// session and rooms test modules so corruption-detection tests
/// agree on the same vector.
#[cfg(test)]
pub(super) const INVALID_UTF8: &[u8] = &[0xFF, 0xFE, 0xFD];
