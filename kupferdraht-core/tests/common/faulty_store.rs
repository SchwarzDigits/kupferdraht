// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! A `Store` that forwards every call to a real store and fails chosen
//! methods on demand — the fault-injection seam for tests that must see
//! a storage write fail at a precise point (Step 97a: the status update
//! after a successful upload). Generated from the `Store` trait; every
//! method is forwarded, defaulted ones included, so the wrapped store
//! behaves exactly like the real one until a fault is armed.
#![allow(dead_code)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use kupferdraht::KupferdrahtError;
use kupferdraht::keystore::SecureKeyStore;
use kupferdraht::storage::{
    AttachmentRecord, BackendMlsKey, DeliveryReceiptRecord, DeviceRecord, ExpiredMessageRecord,
    FeatureConfigEntry, GlobalStore, MentionRecord, MessageEditRecord, ReactionRecord,
    ReadReceiptRecord, RecoveryStateRecord, RoomMemberRecord, RoomRecord, SelfUserRecord,
    ServerConfigRecord, SessionRecord, StorageEngineInfo, StorageProvider, Store, SyncStateRecord,
    TaskRecord, TimelineEventRecord, UserPropertyEntry, open_default_global_store,
    open_default_store,
};

type Fault = Box<dyn Fn(&str) -> bool + Send + Sync>;

/// A one-shot pause (Step 97e): the first call whose method name matches
/// parks until [`FaultyStore::release`] and tells the test it arrived.
struct Pause {
    method: String,
    arrived: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}

pub struct FaultyStore {
    inner: Arc<dyn Store>,
    fault: Mutex<Option<Fault>>,
    pause: Mutex<Option<Pause>>,
    release: Arc<tokio::sync::Notify>,
    /// Every method called, in order (Step 109).
    calls: Mutex<Vec<String>>,
}

impl FaultyStore {
    pub fn new(inner: Arc<dyn Store>) -> Self {
        Self {
            inner,
            fault: Mutex::new(None),
            pause: Mutex::new(None),
            release: Arc::new(tokio::sync::Notify::new()),
            calls: Mutex::new(Vec::new()),
        }
    }

    /// The methods called so far, in order.
    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().expect("calls lock").clone()
    }

    /// Forget the calls recorded so far.
    pub fn clear_calls(&self) {
        self.calls.lock().expect("calls lock").clear();
    }

    /// The next call of `method` parks until [`Self::release`]; the
    /// returned `Notify` fires once it arrived.
    pub fn pause_next(&self, method: &str) -> Arc<tokio::sync::Notify> {
        let arrived = Arc::new(tokio::sync::Notify::new());
        *self.pause.lock().expect("pause lock") = Some(Pause {
            method: method.to_string(),
            arrived: arrived.clone(),
            release: self.release.clone(),
        });
        arrived
    }

    /// Let the parked call go on.
    pub fn release(&self) {
        self.release.notify_one();
    }

    /// Record the call, park if a pause is armed for `method`, then apply
    /// the fault check.
    async fn gate(&self, method: &str) -> Result<(), KupferdrahtError> {
        self.calls
            .lock()
            .expect("calls lock")
            .push(method.to_string());
        let pause = {
            let mut armed = self.pause.lock().expect("pause lock");
            if armed.as_ref().is_some_and(|p| p.method == method) {
                armed.take()
            } else {
                None
            }
        };
        if let Some(pause) = pause {
            pause.arrived.notify_one();
            pause.release.notified().await;
        }
        self.check(method)
    }

    /// From now on, every call whose method name `fails` selects returns
    /// `KupferdrahtError::Storage("injected fault: <method>")`.
    pub fn fail_when(&self, fails: Fault) {
        *self.fault.lock().expect("fault lock") = Some(fails);
    }

    /// Disarm the fault.
    pub fn heal(&self) {
        *self.fault.lock().expect("fault lock") = None;
    }

    fn check(&self, method: &str) -> Result<(), KupferdrahtError> {
        let guard = self.fault.lock().expect("fault lock");
        match guard.as_ref() {
            Some(fails) if fails(method) => Err(KupferdrahtError::Storage(format!(
                "injected fault: {method}"
            ))),
            _ => Ok(()),
        }
    }
}

#[async_trait]
impl Store for FaultyStore {
    async fn begin_page(&self) -> Result<(), KupferdrahtError> {
        self.gate("begin_page").await?;
        self.inner.begin_page().await
    }

    async fn savepoint(&self, index: u32) -> Result<(), KupferdrahtError> {
        self.gate("savepoint").await?;
        self.inner.savepoint(index).await
    }

    async fn rollback_to_savepoint(&self, index: u32) -> Result<(), KupferdrahtError> {
        self.gate("rollback_to_savepoint").await?;
        self.inner.rollback_to_savepoint(index).await
    }

    async fn commit_page(&self) -> Result<(), KupferdrahtError> {
        if let Err(e) = self.gate("commit_page").await {
            // A failed commit rolls the page back, as the real store does.
            self.inner.rollback_page();
            return Err(e);
        }
        self.inner.commit_page().await
    }

    fn rollback_page(&self) {
        self.calls
            .lock()
            .expect("calls lock")
            .push("rollback_page".to_string());
        self.inner.rollback_page();
    }

    async fn storage_engine(&self) -> Result<Option<StorageEngineInfo>, KupferdrahtError> {
        self.gate("storage_engine").await?;
        self.inner.storage_engine().await
    }

    async fn save_session(&self, session: &SessionRecord) -> Result<(), KupferdrahtError> {
        self.gate("save_session").await?;
        self.inner.save_session(session).await
    }

    async fn load_session(&self, user_id: &str) -> Result<Option<SessionRecord>, KupferdrahtError> {
        self.gate("load_session").await?;
        self.inner.load_session(user_id).await
    }

    async fn save_device(&self, device: &DeviceRecord) -> Result<(), KupferdrahtError> {
        self.gate("save_device").await?;
        self.inner.save_device(device).await
    }

    async fn load_device(
        &self,
        user_id: &str,
        client_id: &str,
    ) -> Result<Option<DeviceRecord>, KupferdrahtError> {
        self.gate("load_device").await?;
        self.inner.load_device(user_id, client_id).await
    }

    async fn save_backend_mls_key(&self, key: &BackendMlsKey) -> Result<(), KupferdrahtError> {
        self.gate("save_backend_mls_key").await?;
        self.inner.save_backend_mls_key(key).await
    }

    async fn load_backend_mls_key(
        &self,
        purpose: &str,
        signature_scheme: &str,
    ) -> Result<Option<BackendMlsKey>, KupferdrahtError> {
        self.gate("load_backend_mls_key").await?;
        self.inner
            .load_backend_mls_key(purpose, signature_scheme)
            .await
    }

    async fn replace_rooms(
        &self,
        rooms: &[(RoomRecord, Vec<RoomMemberRecord>)],
    ) -> Result<(), KupferdrahtError> {
        self.gate("replace_rooms").await?;
        self.inner.replace_rooms(rooms).await
    }

    async fn clear_messaging_state(&self) -> Result<(), KupferdrahtError> {
        self.gate("clear_messaging_state").await?;
        self.inner.clear_messaging_state().await
    }

    async fn load_rooms(
        &self,
    ) -> Result<Vec<(RoomRecord, Vec<RoomMemberRecord>)>, KupferdrahtError> {
        self.gate("load_rooms").await?;
        self.inner.load_rooms().await
    }

    async fn update_room_epoch(
        &self,
        room_id: &str,
        new_epoch: i64,
    ) -> Result<(), KupferdrahtError> {
        self.gate("update_room_epoch").await?;
        self.inner.update_room_epoch(room_id, new_epoch).await
    }

    async fn replace_room_members(
        &self,
        room_id: &str,
        members: &[RoomMemberRecord],
    ) -> Result<(), KupferdrahtError> {
        self.gate("replace_room_members").await?;
        self.inner.replace_room_members(room_id, members).await
    }

    async fn insert_timeline_event(
        &self,
        event: &TimelineEventRecord,
    ) -> Result<(), KupferdrahtError> {
        self.gate("insert_timeline_event").await?;
        self.inner.insert_timeline_event(event).await
    }

    async fn load_timeline_events(
        &self,
        room_id: &str,
    ) -> Result<Vec<TimelineEventRecord>, KupferdrahtError> {
        self.gate("load_timeline_events").await?;
        self.inner.load_timeline_events(room_id).await
    }

    async fn find_message_room(&self, target_id: &str) -> Result<Vec<String>, KupferdrahtError> {
        self.gate("find_message_room").await?;
        self.inner.find_message_room(target_id).await
    }

    async fn load_timeline_event(
        &self,
        target_id: &str,
    ) -> Result<Option<TimelineEventRecord>, KupferdrahtError> {
        self.gate("load_timeline_event").await?;
        self.inner.load_timeline_event(target_id).await
    }

    async fn load_timeline_event_in_room(
        &self,
        room_id: &str,
        target_id: &str,
    ) -> Result<Option<TimelineEventRecord>, KupferdrahtError> {
        self.gate("load_timeline_event_in_room").await?;
        self.inner
            .load_timeline_event_in_room(room_id, target_id)
            .await
    }

    async fn load_room_devices(
        &self,
        room_id: &str,
    ) -> Result<Option<Vec<kupferdraht::rooms::RoomDevice>>, KupferdrahtError> {
        self.gate("load_room_devices").await?;
        self.inner.load_room_devices(room_id).await
    }

    async fn replace_room_devices(
        &self,
        room_id: &str,
        devices: &[kupferdraht::rooms::RoomDevice],
    ) -> Result<(), KupferdrahtError> {
        self.gate("replace_room_devices").await?;
        self.inner.replace_room_devices(room_id, devices).await
    }

    async fn mark_send_outcome(
        &self,
        room_id: &str,
        message_id: &str,
        delivered: bool,
        expires_at_ms: Option<i64>,
    ) -> Result<bool, KupferdrahtError> {
        self.gate("mark_send_outcome").await?;
        self.inner
            .mark_send_outcome(room_id, message_id, delivered, expires_at_ms)
            .await
    }

    async fn set_read_marker(&self, room_id: &str, at_ms: i64) -> Result<(), KupferdrahtError> {
        self.gate("set_read_marker").await?;
        self.inner.set_read_marker(room_id, at_ms).await
    }

    async fn read_marker(&self, room_id: &str) -> Result<Option<i64>, KupferdrahtError> {
        self.gate("read_marker").await?;
        self.inner.read_marker(room_id).await
    }

    async fn unread_count_for_room(
        &self,
        room_id: &str,
        self_user_id: &str,
    ) -> Result<u32, KupferdrahtError> {
        self.gate("unread_count_for_room").await?;
        self.inner
            .unread_count_for_room(room_id, self_user_id)
            .await
    }

    async fn unread_counts(
        &self,
        self_user_id: &str,
    ) -> Result<HashMap<String, u32>, KupferdrahtError> {
        self.gate("unread_counts").await?;
        self.inner.unread_counts(self_user_id).await
    }

    async fn mark_delivery_received(
        &self,
        room_id: &str,
        message_id: &str,
    ) -> Result<bool, KupferdrahtError> {
        self.gate("mark_delivery_received").await?;
        self.inner.mark_delivery_received(room_id, message_id).await
    }

    async fn mark_message_resent(
        &self,
        room_id: &str,
        message_id: &str,
        sent_at_ms: i64,
        expires_at_ms: Option<i64>,
    ) -> Result<bool, KupferdrahtError> {
        self.gate("mark_message_resent").await?;
        self.inner
            .mark_message_resent(room_id, message_id, sent_at_ms, expires_at_ms)
            .await
    }

    async fn apply_message_edit(
        &self,
        room_id: &str,
        replacing_message_id: &str,
        edit_event_id: &str,
        new_body: &str,
        edited_at_ms: i64,
    ) -> Result<bool, KupferdrahtError> {
        self.gate("apply_message_edit").await?;
        self.inner
            .apply_message_edit(
                room_id,
                replacing_message_id,
                edit_event_id,
                new_body,
                edited_at_ms,
            )
            .await
    }

    async fn replace_mentions(
        &self,
        room_id: &str,
        message_id: &str,
        mentions: &[MentionRecord],
    ) -> Result<(), KupferdrahtError> {
        self.gate("replace_mentions").await?;
        self.inner
            .replace_mentions(room_id, message_id, mentions)
            .await
    }

    // Step 105: the combined writes. Forwarded like every other method, so
    // the wrapped store's atomic bodies run — the trait's sequential
    // defaults would otherwise stand in for them here.
    async fn insert_timeline_event_with_mentions(
        &self,
        event: &TimelineEventRecord,
        mentions: &[MentionRecord],
    ) -> Result<bool, KupferdrahtError> {
        self.gate("insert_timeline_event_with_mentions").await?;
        self.inner
            .insert_timeline_event_with_mentions(event, mentions)
            .await
    }

    async fn insert_timeline_event_with_attachment(
        &self,
        event: &TimelineEventRecord,
        attachment: &AttachmentRecord,
    ) -> Result<bool, KupferdrahtError> {
        self.gate("insert_timeline_event_with_attachment").await?;
        self.inner
            .insert_timeline_event_with_attachment(event, attachment)
            .await
    }

    async fn insert_timeline_event_with_task(
        &self,
        event: &TimelineEventRecord,
        task: &TaskRecord,
    ) -> Result<bool, KupferdrahtError> {
        self.gate("insert_timeline_event_with_task").await?;
        self.inner
            .insert_timeline_event_with_task(event, task)
            .await
    }

    async fn apply_message_edit_with_mentions(
        &self,
        edit: &MessageEditRecord,
        mentions: &[MentionRecord],
    ) -> Result<bool, KupferdrahtError> {
        self.gate("apply_message_edit_with_mentions").await?;
        self.inner
            .apply_message_edit_with_mentions(edit, mentions)
            .await
    }

    async fn load_mentions_for_room(
        &self,
        room_id: &str,
    ) -> Result<Vec<MentionRecord>, KupferdrahtError> {
        self.gate("load_mentions_for_room").await?;
        self.inner.load_mentions_for_room(room_id).await
    }

    async fn apply_read_receipts(
        &self,
        room_id: &str,
        reader_user_id: &str,
        message_ids: &[String],
        confirmed_at_ms: i64,
    ) -> Result<usize, KupferdrahtError> {
        self.gate("apply_read_receipts").await?;
        self.inner
            .apply_read_receipts(room_id, reader_user_id, message_ids, confirmed_at_ms)
            .await
    }

    async fn load_read_receipts_for_room(
        &self,
        room_id: &str,
    ) -> Result<Vec<ReadReceiptRecord>, KupferdrahtError> {
        self.gate("load_read_receipts_for_room").await?;
        self.inner.load_read_receipts_for_room(room_id).await
    }

    async fn apply_delivery_receipts(
        &self,
        room_id: &str,
        confirmer_user_id: &str,
        message_ids: &[String],
        confirmed_at_ms: i64,
    ) -> Result<usize, KupferdrahtError> {
        self.gate("apply_delivery_receipts").await?;
        self.inner
            .apply_delivery_receipts(room_id, confirmer_user_id, message_ids, confirmed_at_ms)
            .await
    }

    async fn load_delivery_receipts_for_room(
        &self,
        room_id: &str,
    ) -> Result<Vec<DeliveryReceiptRecord>, KupferdrahtError> {
        self.gate("load_delivery_receipts_for_room").await?;
        self.inner.load_delivery_receipts_for_room(room_id).await
    }

    async fn canonical_message_id(
        &self,
        room_id: &str,
        target_id: &str,
    ) -> Result<Option<String>, KupferdrahtError> {
        self.gate("canonical_message_id").await?;
        self.inner.canonical_message_id(room_id, target_id).await
    }

    async fn replace_user_reactions(
        &self,
        room_id: &str,
        canonical_message_id: &str,
        sender_user_id: &str,
        emojis: &[String],
        sent_at_ms: i64,
    ) -> Result<(), KupferdrahtError> {
        self.gate("replace_user_reactions").await?;
        self.inner
            .replace_user_reactions(
                room_id,
                canonical_message_id,
                sender_user_id,
                emojis,
                sent_at_ms,
            )
            .await
    }

    async fn load_reactions_for_room(
        &self,
        room_id: &str,
    ) -> Result<Vec<ReactionRecord>, KupferdrahtError> {
        self.gate("load_reactions_for_room").await?;
        self.inner.load_reactions_for_room(room_id).await
    }

    async fn apply_message_delete(
        &self,
        room_id: &str,
        target_message_id: &str,
        redacted_at_ms: i64,
    ) -> Result<bool, KupferdrahtError> {
        self.gate("apply_message_delete").await?;
        self.inner
            .apply_message_delete(room_id, target_message_id, redacted_at_ms)
            .await
    }

    async fn expire_ephemeral_messages(
        &self,
        now_ms: i64,
    ) -> Result<Vec<ExpiredMessageRecord>, KupferdrahtError> {
        self.gate("expire_ephemeral_messages").await?;
        self.inner.expire_ephemeral_messages(now_ms).await
    }

    async fn pending_ephemeral_expiries(&self, now_ms: i64) -> Result<Vec<i64>, KupferdrahtError> {
        self.gate("pending_ephemeral_expiries").await?;
        self.inner.pending_ephemeral_expiries(now_ms).await
    }

    async fn save_sync_state(&self, state: &SyncStateRecord) -> Result<(), KupferdrahtError> {
        self.gate("save_sync_state").await?;
        self.inner.save_sync_state(state).await
    }

    async fn load_sync_state(&self) -> Result<Option<SyncStateRecord>, KupferdrahtError> {
        self.gate("load_sync_state").await?;
        self.inner.load_sync_state().await
    }

    async fn save_recovery_state(
        &self,
        state: &RecoveryStateRecord,
    ) -> Result<(), KupferdrahtError> {
        self.gate("save_recovery_state").await?;
        self.inner.save_recovery_state(state).await
    }

    async fn load_recovery_state(&self) -> Result<Option<RecoveryStateRecord>, KupferdrahtError> {
        self.gate("load_recovery_state").await?;
        self.inner.load_recovery_state().await
    }

    async fn save_self_user(&self, user: &SelfUserRecord) -> Result<(), KupferdrahtError> {
        self.gate("save_self_user").await?;
        self.inner.save_self_user(user).await
    }

    async fn load_self_user(&self, id: &str) -> Result<Option<SelfUserRecord>, KupferdrahtError> {
        self.gate("load_self_user").await?;
        self.inner.load_self_user(id).await
    }

    async fn save_server_config(
        &self,
        config: &ServerConfigRecord,
    ) -> Result<(), KupferdrahtError> {
        self.gate("save_server_config").await?;
        self.inner.save_server_config(config).await
    }

    async fn load_server_config(&self) -> Result<Option<ServerConfigRecord>, KupferdrahtError> {
        self.gate("load_server_config").await?;
        self.inner.load_server_config().await
    }

    async fn replace_feature_configs(
        &self,
        entries: &[FeatureConfigEntry],
    ) -> Result<(), KupferdrahtError> {
        self.gate("replace_feature_configs").await?;
        self.inner.replace_feature_configs(entries).await
    }

    async fn upsert_feature_config(
        &self,
        entry: &FeatureConfigEntry,
    ) -> Result<(), KupferdrahtError> {
        self.gate("upsert_feature_config").await?;
        self.inner.upsert_feature_config(entry).await
    }

    async fn list_feature_configs(&self) -> Result<Vec<FeatureConfigEntry>, KupferdrahtError> {
        self.gate("list_feature_configs").await?;
        self.inner.list_feature_configs().await
    }

    async fn get_feature_config(
        &self,
        name: &str,
    ) -> Result<Option<FeatureConfigEntry>, KupferdrahtError> {
        self.gate("get_feature_config").await?;
        self.inner.get_feature_config(name).await
    }

    async fn replace_user_properties(
        &self,
        entries: &[UserPropertyEntry],
    ) -> Result<(), KupferdrahtError> {
        self.gate("replace_user_properties").await?;
        self.inner.replace_user_properties(entries).await
    }

    async fn upsert_user_property(
        &self,
        entry: &UserPropertyEntry,
    ) -> Result<(), KupferdrahtError> {
        self.gate("upsert_user_property").await?;
        self.inner.upsert_user_property(entry).await
    }

    async fn delete_user_property(&self, key: &str) -> Result<(), KupferdrahtError> {
        self.gate("delete_user_property").await?;
        self.inner.delete_user_property(key).await
    }

    async fn list_user_properties(&self) -> Result<Vec<UserPropertyEntry>, KupferdrahtError> {
        self.gate("list_user_properties").await?;
        self.inner.list_user_properties().await
    }

    async fn get_user_property(
        &self,
        key: &str,
    ) -> Result<Option<UserPropertyEntry>, KupferdrahtError> {
        self.gate("get_user_property").await?;
        self.inner.get_user_property(key).await
    }

    async fn insert_attachment(&self, record: &AttachmentRecord) -> Result<(), KupferdrahtError> {
        self.gate("insert_attachment").await?;
        self.inner.insert_attachment(record).await
    }

    async fn get_attachment(
        &self,
        message_id: &str,
        room_id: &str,
    ) -> Result<Option<AttachmentRecord>, KupferdrahtError> {
        self.gate("get_attachment").await?;
        self.inner.get_attachment(message_id, room_id).await
    }

    async fn list_attachments_for_room(
        &self,
        room_id: &str,
    ) -> Result<Vec<AttachmentRecord>, KupferdrahtError> {
        self.gate("list_attachments_for_room").await?;
        self.inner.list_attachments_for_room(room_id).await
    }

    async fn update_attachment_cache_path(
        &self,
        message_id: &str,
        room_id: &str,
        cache_path: Option<&str>,
    ) -> Result<(), KupferdrahtError> {
        self.gate("update_attachment_cache_path").await?;
        self.inner
            .update_attachment_cache_path(message_id, room_id, cache_path)
            .await
    }

    async fn delete_attachment(
        &self,
        message_id: &str,
        room_id: &str,
    ) -> Result<(), KupferdrahtError> {
        self.gate("delete_attachment").await?;
        self.inner.delete_attachment(message_id, room_id).await
    }

    async fn upsert_task(&self, task: &TaskRecord) -> Result<(), KupferdrahtError> {
        self.gate("upsert_task").await?;
        self.inner.upsert_task(task).await
    }

    async fn replace_room_tasks(
        &self,
        room_id: &str,
        tasks: &[TaskRecord],
    ) -> Result<(), KupferdrahtError> {
        self.gate("replace_room_tasks").await?;
        self.inner.replace_room_tasks(room_id, tasks).await
    }

    async fn load_tasks_for_room(
        &self,
        room_id: &str,
    ) -> Result<Vec<TaskRecord>, KupferdrahtError> {
        self.gate("load_tasks_for_room").await?;
        self.inner.load_tasks_for_room(room_id).await
    }

    async fn load_open_tasks_for_assignee(
        &self,
        assignee: &str,
    ) -> Result<Vec<TaskRecord>, KupferdrahtError> {
        self.gate("load_open_tasks_for_assignee").await?;
        self.inner.load_open_tasks_for_assignee(assignee).await
    }

    async fn load_tasks_for_creator(
        &self,
        creator_user_id: &str,
    ) -> Result<Vec<TaskRecord>, KupferdrahtError> {
        self.gate("load_tasks_for_creator").await?;
        self.inner.load_tasks_for_creator(creator_user_id).await
    }

    async fn count_tasks(&self) -> Result<u64, KupferdrahtError> {
        self.gate("count_tasks").await?;
        self.inner.count_tasks().await
    }

    async fn load_task_timeline_rows(&self) -> Result<Vec<TimelineEventRecord>, KupferdrahtError> {
        self.gate("load_task_timeline_rows").await?;
        self.inner.load_task_timeline_rows().await
    }
}

/// A `StorageProvider` whose account store is a [`FaultyStore`] over the
/// real SQLite store of `home`.
pub struct FaultyProvider {
    home: PathBuf,
    keystore: Arc<dyn SecureKeyStore>,
    store: Mutex<Option<Arc<FaultyStore>>>,
}

impl FaultyProvider {
    pub fn new(home: PathBuf, keystore: Arc<dyn SecureKeyStore>) -> Self {
        Self {
            home,
            keystore,
            store: Mutex::new(None),
        }
    }

    pub fn faulty(&self) -> Arc<FaultyStore> {
        self.store
            .lock()
            .expect("provider lock")
            .clone()
            .expect("the account store was opened")
    }
}

impl StorageProvider for FaultyProvider {
    fn open_global(&self) -> Result<Arc<dyn GlobalStore>, KupferdrahtError> {
        open_default_global_store(&self.home, &self.keystore)
    }

    fn open_store(&self, account_id: &str) -> Result<Arc<dyn Store>, KupferdrahtError> {
        let inner = open_default_store(&self.home, &self.keystore, account_id)?;
        let faulty = Arc::new(FaultyStore::new(inner));
        *self.store.lock().expect("provider lock") = Some(faulty.clone());
        Ok(faulty)
    }
}
