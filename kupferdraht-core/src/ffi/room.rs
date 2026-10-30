// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! FFI wrappers for rooms (Step 20c-2).
//!
//! `ffi::Room` wraps `Arc<kupferdraht::Room>` with owned-return methods; the
//! room value types (`RoomType`, `RoomMember`, `RoomAccess`, `JoinLink`)
//! are FFI-shaped already, so they carry their `uniffi` derives on the
//! core type and cross unchanged. The room methods on `ffi::Client` live
//! here too (a second `#[uniffi::export] impl`).
//!
//! The full room surface lives here — timeline access, paginated
//! `messages`, membership, access policy + guest links. The bulk
//! `join_all_rooms` / `resync_existing_rooms` summaries live in
//! `ffi::admin`.

use std::sync::Arc;

use super::Client;
use super::timeline::{Timeline, TimelineItem, build_options, map_items};
use crate::error::KupferdrahtError;
use crate::rooms::{JoinLink, Room as CoreRoom, RoomAccess, RoomMember};

/// A conversation — the FFI wrapper around the native
/// [`kupferdraht::Room`](crate::rooms::Room).
#[derive(uniffi::Object)]
pub struct Room {
    inner: Arc<CoreRoom>,
}

impl Room {
    pub(super) fn wrap(inner: Arc<CoreRoom>) -> Arc<Self> {
        Arc::new(Self { inner })
    }

    fn wrap_each(rooms: Vec<Arc<CoreRoom>>) -> Vec<Arc<Self>> {
        rooms.into_iter().map(Self::wrap).collect()
    }
}

#[uniffi::export]
impl Room {
    /// Qualified room id `<uuid>@<domain>`.
    #[must_use]
    pub fn id(&self) -> String {
        self.inner.id()
    }

    /// Wire's room name, or the empty string for unnamed rooms (1:1 DMs).
    #[must_use]
    pub fn name(&self) -> String {
        self.inner.name()
    }

    /// The conversation kind.
    #[must_use]
    pub fn room_type(&self) -> crate::rooms::RoomType {
        self.inner.room_type()
    }

    /// The MLS epoch this client last observed for the room.
    #[must_use]
    pub fn epoch(&self) -> u64 {
        self.inner.epoch()
    }

    /// For a 1:1 DM, the single other participant; `None` otherwise.
    #[must_use]
    pub fn counterpart(&self) -> Option<RoomMember> {
        self.inner.counterpart()
    }

    /// The MLS `group_id` raw bytes, or `None` if the room carries none.
    #[must_use]
    pub fn group_id_bytes(&self) -> Option<Vec<u8>> {
        self.inner.group_id_bytes()
    }

    /// The MLS `group_id` as base64, or `None`.
    #[must_use]
    pub fn group_id_b64(&self) -> Option<String> {
        self.inner.group_id_b64().map(str::to_owned)
    }

    /// Wire's `access` array (which join modes the room permits).
    #[must_use]
    pub fn access_modes(&self) -> Vec<String> {
        self.inner.access_modes()
    }

    /// Wire's `access_role` array (which user classes may join).
    #[must_use]
    pub fn access_roles(&self) -> Vec<String> {
        self.inner.access_roles()
    }

    /// A fresh timeline handle for this room.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub fn timeline(&self) -> Result<Arc<Timeline>, KupferdrahtError> {
        self.inner.timeline().map(Timeline::wrap)
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl Room {
    /// The member list from the local MLS group leaves.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn members(&self) -> Result<Vec<RoomMember>, KupferdrahtError> {
        self.inner.members().await
    }

    /// The room's devices — one per MLS leaf, with identity and MLS
    /// thumbprint (Step 95d).
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn devices(&self) -> Result<Vec<crate::rooms::RoomDevice>, KupferdrahtError> {
        self.inner.devices().await
    }

    /// Folded task list of this room (Step 60b, ADR-0014): one entry
    /// per `task_id`, latest full state wins, sorted by due date, then
    /// priority, then first-seen.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn tasks(&self) -> Result<Vec<super::timeline::FoldedTask>, KupferdrahtError> {
        Ok(self
            .inner
            .tasks()
            .await?
            .iter()
            .map(super::timeline::FoldedTask::from)
            .collect())
    }

    /// Whether this room is marked as a dedicated task list (Step 75,
    /// ADR-0020): folded from `TaskListMarker` rows — any member's
    /// last write wins in event order; `false` when no marker exists.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn is_task_list(&self) -> Result<bool, KupferdrahtError> {
        self.inner.is_task_list().await
    }

    /// Set or clear this room's task-list marker (Step 75, ADR-0020):
    /// sends the full marker state (no fallback — official clients
    /// see nothing) and folds it locally. Any member may call this;
    /// there is no admin gate.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn set_task_list(&self, marked: bool) -> Result<(), KupferdrahtError> {
        self.inner.set_task_list(marked).await
    }

    /// Folded case state of this room (Step 72b, ADR-0018).
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn case(&self) -> Result<Option<crate::timeline::CaseSnapshot>, KupferdrahtError> {
        self.inner.case().await
    }

    /// Create or update this room's case via a set-only patch
    /// (Step 72c, ADR-0018); requires conversation-admin role.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn set_case(
        &self,
        patch: crate::timeline::CasePatch,
    ) -> Result<crate::timeline::CaseSnapshot, KupferdrahtError> {
        self.inner.set_case(patch).await
    }

    /// Re-derive + persist the member list, then return it.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn sync_members(&self) -> Result<Vec<RoomMember>, KupferdrahtError> {
        self.inner.sync_members().await
    }

    /// External-commit join for this room.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn join(&self) -> Result<(), KupferdrahtError> {
        self.inner.join().await
    }

    /// Leave this room.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn leave(&self) -> Result<(), KupferdrahtError> {
        self.inner.leave().await
    }

    /// Delete this conversation for everyone (team-admin action,
    /// distinct from `leave`). Only valid for team conversations.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn delete(&self) -> Result<(), KupferdrahtError> {
        self.inner.delete().await
    }

    /// Rename the room; returns the refreshed handle.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn set_name(&self, new_name: String) -> Result<Arc<Room>, KupferdrahtError> {
        self.inner.set_name(&new_name).await.map(Room::wrap)
    }

    /// Invite users (qualified ids) into the room.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn invite(&self, user_ids: Vec<String>) -> Result<Arc<Room>, KupferdrahtError> {
        self.inner.invite(&user_ids).await.map(Room::wrap)
    }

    /// Remove users (qualified ids) from the room.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn kick(&self, user_ids: Vec<String>) -> Result<Arc<Room>, KupferdrahtError> {
        self.inner.kick(&user_ids).await.map(Room::wrap)
    }

    /// Paginated read of this room's persisted timeline (local-only).
    /// `limit` of `None` returns everything; `before` truncates at
    /// that id.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn messages(
        &self,
        limit: Option<u32>,
        before: Option<String>,
    ) -> Result<Vec<TimelineItem>, KupferdrahtError> {
        let opts = build_options(limit, before);
        Ok(map_items(self.inner.messages(&opts).await?))
    }

    /// The failed sends of this room (`Failed` rows, oldest first) —
    /// the consumer's input for "sync came back → resend". Step 79.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn failed_sends(
        &self,
    ) -> Result<Vec<super::timeline::TimelineItem>, KupferdrahtError> {
        Ok(super::timeline::map_items(self.inner.failed_sends().await?))
    }

    /// The room's `(access, access_role)` policy.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn access(&self) -> Result<RoomAccess, KupferdrahtError> {
        self.inner.access().await
    }

    /// Number of unread inbound messages (Step 67) — device-local
    /// read marker; `Timeline::mark_as_read` advances it.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn unread_count(&self) -> Result<u32, KupferdrahtError> {
        self.inner.unread_count().await
    }

    /// Set the room's `access` + `access_role`; returns the refreshed
    /// handle.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn set_access(
        &self,
        access: Vec<String>,
        access_role: Vec<String>,
    ) -> Result<Arc<Room>, KupferdrahtError> {
        self.inner
            .set_access(access, access_role)
            .await
            .map(Room::wrap)
    }

    /// Convenience: open the room to guest-link joins; returns the
    /// refreshed handle.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn open_to_guests(&self) -> Result<Arc<Room>, KupferdrahtError> {
        self.inner.open_to_guests().await.map(Room::wrap)
    }

    /// Create a guest join link (optionally password-protected).
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn create_join_link(
        &self,
        password: Option<String>,
    ) -> Result<JoinLink, KupferdrahtError> {
        self.inner.create_join_link(password).await
    }

    /// The room's current guest join link, if any.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn current_join_link(&self) -> Result<Option<JoinLink>, KupferdrahtError> {
        self.inner.current_join_link().await
    }

    /// Revoke the room's guest join link.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn revoke_join_link(&self) -> Result<(), KupferdrahtError> {
        self.inner.revoke_join_link().await
    }
}

#[uniffi::export]
impl Client {
    /// Rooms the caller has joined (in-memory snapshot).
    #[must_use]
    pub fn joined_rooms(&self) -> Vec<Arc<Room>> {
        Room::wrap_each(self.inner.joined_rooms())
    }

    /// Rooms the caller is invited to but has not joined.
    #[must_use]
    pub fn invited_rooms(&self) -> Vec<Arc<Room>> {
        Room::wrap_each(self.inner.invited_rooms())
    }

    /// Local-cache DM lookup by qualified peer id; no network.
    #[must_use]
    pub fn dm_room(&self, peer: &str) -> Option<Arc<Room>> {
        self.inner.dm_room(peer).map(Room::wrap)
    }

    /// Local-cache lookup of the caller's self conversation; no network.
    #[must_use]
    pub fn self_room(&self) -> Option<Arc<Room>> {
        self.inner.self_room().map(Room::wrap)
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl Client {
    /// In-memory rooms snapshot.
    pub async fn rooms(&self) -> Vec<Arc<Room>> {
        Room::wrap_each(self.inner.rooms().await)
    }

    /// Single-room lookup by qualified id.
    pub async fn room(&self, room_id: &str) -> Option<Arc<Room>> {
        self.inner.room(room_id).await.map(Room::wrap)
    }

    /// Cache-first lookup with a lazy store-hydrate fallback.
    pub async fn room_or_load(&self, room_id: &str) -> Option<Arc<Room>> {
        self.inner.room_or_load(room_id).await.map(Room::wrap)
    }

    /// Create a fresh group conversation.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn create_room(&self, name: &str) -> Result<Arc<Room>, KupferdrahtError> {
        self.inner.create_room(name).await.map(Room::wrap)
    }

    /// Look up (or establish) the MLS 1:1 conversation with `peer`.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn create_dm(&self, peer: &str) -> Result<Arc<Room>, KupferdrahtError> {
        self.inner.create_dm(peer).await.map(Room::wrap)
    }

    /// `dm_room` first, else `create_dm` — exactly one POST per peer.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn dm_with(&self, peer: &str) -> Result<Arc<Room>, KupferdrahtError> {
        self.inner.dm_with(peer).await.map(Room::wrap)
    }

    /// External-commit join for one room by id.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn join_room(&self, room_id: &str) -> Result<(), KupferdrahtError> {
        self.inner.join_room(room_id).await
    }

    /// Leave one room by id.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn leave_room(&self, room_id: &str) -> Result<(), KupferdrahtError> {
        self.inner.leave_room(room_id).await
    }

    /// Delete a team conversation for everyone, by id (team-admin
    /// action). Only valid for team conversations.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn delete_room(&self, room_id: &str) -> Result<(), KupferdrahtError> {
        self.inner.delete_room(room_id).await
    }

    /// Rename a room by id; returns the refreshed handle.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn rename_room(
        &self,
        room_id: &str,
        new_name: &str,
    ) -> Result<Arc<Room>, KupferdrahtError> {
        self.inner
            .rename_room(room_id, new_name)
            .await
            .map(Room::wrap)
    }

    /// Re-derive + persist a room's members by id.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn sync_members(&self, room_id: &str) -> Result<Vec<RoomMember>, KupferdrahtError> {
        self.inner.sync_members(room_id).await
    }
}
