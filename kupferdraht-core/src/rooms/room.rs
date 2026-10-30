// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `Room` + `RoomType` + `RoomMember` + the Wire→Room adapter.

use std::collections::HashMap;
use std::sync::{Arc, Weak};

use base64::Engine as _;

use crate::client::Client;
use crate::error::KupferdrahtError;
use crate::storage::{RoomMemberRecord, RoomRecord};
use crate::wire::dto::{ConvType, OwnConversation, Protocol, QualifiedId, UserProfile};

/// Kind of room. Matches matrix-rust-sdk's coarse room taxonomy
/// where we can; Wire-specific wrinkles (`SelfRoom`) get their own
/// variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum RoomType {
    /// 1:1 DM. `Room::counterpart` returns the single other
    /// participant.
    DirectMessage,
    /// Multi-party group chat.
    Group,
    /// Channel (broadcast-shaped group chat; Wire-specific).
    Channel,
    /// Self-room (Wire's notes-to-self conversation). Has only the
    /// self member.
    SelfRoom,
}

impl RoomType {
    /// Stable stringification used as the `room_type` column value
    /// in the `rooms` table. Kept in sync with
    /// [`RoomType::from_str_persisted`].
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DirectMessage => "direct_message",
            Self::Group => "group",
            Self::Channel => "channel",
            Self::SelfRoom => "self_room",
        }
    }

    /// Parse the `room_type` column back into `RoomType`. Returns
    /// `None` for an unknown string — the caller decides whether to
    /// skip the row or surface an error.
    #[must_use]
    pub fn from_str_persisted(s: &str) -> Option<Self> {
        match s {
            "direct_message" => Some(Self::DirectMessage),
            "group" => Some(Self::Group),
            "channel" => Some(Self::Channel),
            "self_room" => Some(Self::SelfRoom),
            _ => None,
        }
    }
}

/// A member's conversation-level role (Step 72a, ADR-0018). Wire
/// knows exactly two — the conversation creator and anyone promoted
/// is an administrator, everyone else a member; Kupferdraht maps any
/// other server role name to `Member`. matrix-rust-sdk analog:
/// `RoomMemberRole::Administrator` / `User` (power-level derived);
/// Kupferdraht keeps the two-level model Wire actually has. Case snapshots
/// (ADR-0018) fold their core fields only from administrators.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum RoomMemberRole {
    Administrator,
    Member,
}

impl RoomMemberRole {
    /// Neutral persistence vocabulary (V26 `room_members.role`).
    #[must_use]
    pub(crate) fn as_db_str(self) -> &'static str {
        match self {
            Self::Administrator => "administrator",
            Self::Member => "member",
        }
    }

    /// Inverse of [`Self::as_db_str`]. Unknown strings (a newer
    /// Kupferdraht's vocabulary) read back as `Member` — never an error,
    /// never an invented privilege.
    #[must_use]
    pub(crate) fn from_db_str(raw: &str) -> Self {
        if raw == "administrator" {
            Self::Administrator
        } else {
            Self::Member
        }
    }
}

/// One participant in a `Room`. Cycle 2b fills in the qualified
/// `user_id` and the `is_self` flag only; `display_name` and
/// `handle` are populated by cycle 2c's `POST /v14/list-users`
/// resolution; `role` comes from the conversation metadata
/// (Step 72a).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct RoomMember {
    pub user_id: String,
    pub is_self: bool,
    pub display_name: Option<String>,
    pub handle: Option<String>,
    pub role: RoomMemberRole,
}

/// A single conversation. A snapshot from the last `Client::sync_metadata`
/// or `Client::load_rooms_from_store` call, augmented with a [`Weak`]
/// back-reference to the owning [`Client`] so instance methods like
/// [`Room::join`] can drive `Client::join_room` without the caller
/// plumbing the handle through.
#[derive(Debug, Clone)]
pub struct Room {
    id: String,
    name: String,
    #[allow(clippy::struct_field_names)]
    room_type: RoomType,
    epoch: u64,
    counterpart: Option<RoomMember>,
    members: Vec<RoomMember>,
    /// MLS `group_id` as delivered by Wire (base64 of the raw bytes).
    /// `None` on rooms whose metadata did not carry one — defensive
    /// since after the proteus filter every surviving row is MLS and
    /// Wire guarantees `group_id` on those.
    group_id: Option<String>,
    /// Wire's `access` array — Step 8b. Indicates which join modes
    /// the conversation permits. `["invite"]` is members-only;
    /// `["invite","code"]` permits guest-link redeem; `"link"` and
    /// `"private"` are also possible. Empty when Wire's response
    /// omits the field.
    access: Vec<String>,
    /// Wire's `access_role` array — Step 8b. User classes allowed
    /// to join.
    access_role: Vec<String>,
    /// Weak handle back to the owning `Client`. `Weak::new()` (which
    /// never upgrades) on rooms constructed by unit tests that do not
    /// exercise the callback path.
    client: Weak<Client>,
}

impl Room {
    /// The owning `Client` for a crate-internal room operation `op`
    /// (Step 95d's `Room::devices` lives in `rooms/devices.rs`).
    pub(crate) fn owning_client(&self, op: &str) -> Result<Arc<Client>, KupferdrahtError> {
        self.client.upgrade().ok_or_else(|| {
            KupferdrahtError::InvalidInput(format!(
                "Room: owning Client has been dropped; cannot call {op}()"
            ))
        })
    }

    /// Qualified room id: `<uuid>@<domain>` (Wire's
    /// `Qualified_ConvId` rendered as a single string). Stable key
    /// for `Client::room`.
    #[must_use]
    pub fn id(&self) -> String {
        self.id.clone()
    }

    /// Wire's room name, or the empty string if the server returned
    /// none (e.g. 1:1 DMs are unnamed — use `counterpart().display_name`
    /// for a humane label).
    #[must_use]
    pub fn name(&self) -> String {
        self.name.clone()
    }

    #[must_use]
    pub fn room_type(&self) -> RoomType {
        self.room_type
    }

    /// MLS group epoch from Wire's metadata. `0` for non-MLS rooms
    /// (shouldn't happen today — we filter `protocol: "proteus"` —
    /// but preserved as a sensible default).
    #[must_use]
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// For `RoomType::DirectMessage` with exactly one other
    /// participant, the counterpart. `None` for groups, channels,
    /// self-rooms, or DMs with an unexpected member count.
    #[must_use]
    pub fn counterpart(&self) -> Option<RoomMember> {
        self.counterpart.clone()
    }

    /// Snapshot of current members. Cycle 2b returns a cached copy
    /// synchronously (wrapped in `async` to match the public
    /// signature from PLAN.md — cycle 2d re-fetches from storage and
    /// the `await` starts earning its keep there). Later cycles may
    /// re-fetch from the backend.
    ///
    /// # Errors
    /// Currently never fails; the `Result` is for forward
    /// compatibility with backing-store errors introduced in cycle
    /// 2d / Step 4.
    #[allow(unknown_lints, clippy::unused_async_trait_impl)]
    #[allow(clippy::unused_async)]
    pub async fn members(&self) -> Result<Vec<RoomMember>, KupferdrahtError> {
        Ok(self.members.clone())
    }

    /// The folded case state of this room (Step 72b, ADR-0018):
    /// last admin-authorized snapshot core + arrival-ordered
    /// display-only extensions, derived from the persisted
    /// `timeline_events` rows (`kind='case'`; redacted rows drop out
    /// of the fold). `None` when no authorized snapshot exists.
    ///
    /// # Errors
    /// * `KupferdrahtError::InvalidInput` if the owning `Client` is gone
    ///   or was built without storage.
    /// * `KupferdrahtError::Storage` for row-load failures.
    pub async fn case(&self) -> Result<Option<crate::timeline::CaseSnapshot>, KupferdrahtError> {
        let client = self.client.upgrade().ok_or_else(|| {
            KupferdrahtError::InvalidInput(
                "Room: owning Client has been dropped; cannot call case()".into(),
            )
        })?;
        client.folded_case(&self.id).await
    }

    /// Create or update this room's case (Step 72c, ADR-0018): apply
    /// the set-only `patch` onto the current fold, bump `revision`,
    /// send the resulting full snapshot (no text fallback), fold it
    /// locally, and return it. Client-side gate: the caller's own
    /// member role must be `Administrator` — the receive-side fold
    /// would ignore a non-admin snapshot anyway (`InvalidInput`).
    ///
    /// # Errors
    /// * `KupferdrahtError::InvalidInput` if the owning `Client` is gone,
    ///   the caller is not a conversation administrator, or the room
    ///   has no local MLS group.
    /// * Encode / MLS / transport / storage errors from the send.
    pub async fn set_case(
        &self,
        patch: crate::timeline::CasePatch,
    ) -> Result<crate::timeline::CaseSnapshot, KupferdrahtError> {
        let client = self.client.upgrade().ok_or_else(|| {
            KupferdrahtError::InvalidInput(
                "Room: owning Client has been dropped; cannot call set_case()".into(),
            )
        })?;
        client.set_case(&self.id, patch).await
    }

    /// Whether this room is marked as a dedicated task list (Step 75,
    /// ADR-0020): folded from `TaskListMarker` rows — any member's
    /// last write wins in event order; redacted rows drop out. `false`
    /// when no marker row exists.
    ///
    /// # Errors
    /// * `KupferdrahtError::InvalidInput` if the owning `Client` is gone
    ///   or was built without storage.
    /// * `KupferdrahtError::Storage` for row-load failures.
    pub async fn is_task_list(&self) -> Result<bool, KupferdrahtError> {
        let client = self.client.upgrade().ok_or_else(|| {
            KupferdrahtError::InvalidInput(
                "Room: owning Client has been dropped; cannot call is_task_list()".into(),
            )
        })?;
        client.folded_task_list_marker(&self.id).await
    }

    /// Set or clear this room's task-list marker (Step 75, ADR-0020):
    /// sends the full marker state (no text fallback — official
    /// clients see nothing) and folds it locally. Any member may call
    /// this; there is no admin gate (task-family model — the marker
    /// structures navigation, it grants no authority).
    ///
    /// # Errors
    /// * `KupferdrahtError::InvalidInput` if the owning `Client` is gone
    ///   or the room has no local MLS group.
    /// * Encode / MLS / transport / storage errors from the send.
    pub async fn set_task_list(&self, marked: bool) -> Result<(), KupferdrahtError> {
        let client = self.client.upgrade().ok_or_else(|| {
            KupferdrahtError::InvalidInput(
                "Room: owning Client has been dropped; cannot call set_task_list()".into(),
            )
        })?;
        client.set_task_list(&self.id, marked).await
    }

    /// Raw MLS `group_id` bytes if Wire reported one for this room.
    /// Obtained by base64-decoding the `group_id` column / Wire field.
    /// `None` if the room has no `group_id` on file, the value is not
    /// valid base64, or it decodes to nothing (Step 95c).
    #[must_use]
    pub fn group_id_bytes(&self) -> Option<Vec<u8>> {
        self.group_id
            .as_ref()
            .and_then(|s| base64::engine::general_purpose::STANDARD.decode(s).ok())
            .filter(|bytes| !bytes.is_empty())
    }

    /// Base64-encoded MLS `group_id` as delivered by Wire's metadata.
    /// Exposed for persistence round-tripping and CLI diagnostics.
    #[must_use]
    pub fn group_id_b64(&self) -> Option<&str> {
        self.group_id.as_deref()
    }

    /// Join this room's MLS group via external commit. Delegates to
    /// [`Client::join_room`]. On return local MLS state has advanced
    /// to `server_epoch + 1` and the room's in-memory `epoch` reflects
    /// the same value.
    ///
    /// # Errors
    /// * `KupferdrahtError::InvalidInput` if the owning `Client` has been
    ///   dropped (should not happen for a `Room` obtained via
    ///   `Client::rooms` / `Client::room`).
    /// * Transport / MLS / backend errors from `Client::join_room`.
    pub async fn join(&self) -> Result<(), KupferdrahtError> {
        let client = self.client.upgrade().ok_or_else(|| {
            KupferdrahtError::InvalidInput(
                "Room: owning Client has been dropped; cannot call join()".into(),
            )
        })?;
        client.join_room(&self.id).await
    }

    /// Folded task list of this room (Step 60b, ADR-0014): one entry
    /// per `task_id`, latest full state wins (LWW in arrival order),
    /// sorted by due date, then priority, then first-seen.
    ///
    /// # Errors
    /// * `InvalidInput` when the owning `Client` was dropped or no
    ///   `Store` is attached.
    /// * `Storage` from the SQL read.
    pub async fn tasks(&self) -> Result<Vec<crate::timeline::FoldedTask>, KupferdrahtError> {
        let client = self.client.upgrade().ok_or_else(|| {
            KupferdrahtError::InvalidInput(
                "Room: owning Client has been dropped; cannot call tasks()".into(),
            )
        })?;
        client.room_tasks(&self.id).await
    }

    /// Number of unread inbound messages in this room (Step 67):
    /// rows from other senders, not redacted, newer than the
    /// device-local read marker. **No marker → everything inbound
    /// counts**; `Timeline::mark_as_read`
    /// advances the marker. Device-local — the cross-device read
    /// position rides the self-conversation in a later step.
    ///
    /// matrix-rust-sdk analog: `Room::unread_notification_counts`
    /// (Kupferdraht: a single total, no notification/mention split yet).
    ///
    /// # Errors
    /// `KupferdrahtError::InvalidInput` when the owning `Client` is gone;
    /// `Storage` on read failure.
    pub async fn unread_count(&self) -> Result<u32, KupferdrahtError> {
        let client = self.client.upgrade().ok_or_else(|| {
            KupferdrahtError::InvalidInput("unread_count: owning Client dropped".into())
        })?;
        let Some(store) = client.store_handle() else {
            return Ok(0);
        };
        let self_id = client
            .user_id()
            .map(|u| u.as_str().to_string())
            .ok_or_else(|| {
                KupferdrahtError::InvalidInput("unread_count: no authenticated user".into())
            })?;
        store.unread_count_for_room(&self.id, &self_id).await
    }

    /// Acquire a [`Timeline`] for this room. Cheap to call —
    /// constructs an `Arc<Timeline>` carrying an `Arc<Client>` plus
    /// the room id by value. Each call returns a fresh handle; the
    /// caller is free to drop it after a single send.
    ///
    /// PLAN.md's signature is `async`, mirroring matrix-rust-sdk's
    /// `Room::timeline`. Kupferdraht's construction is synchronous today
    /// — the `Result` shape future-proofs against incoming-event
    /// caches added in Step 5.
    ///
    /// # Errors
    /// `KupferdrahtError::InvalidInput` if the owning `Client` was
    /// dropped (cannot happen for `Room`s obtained via
    /// `Client::rooms` / `Client::room`).
    pub fn timeline(&self) -> Result<std::sync::Arc<crate::timeline::Timeline>, KupferdrahtError> {
        let client = self.client.upgrade().ok_or_else(|| {
            KupferdrahtError::InvalidInput(
                "Room: owning Client has been dropped; cannot call timeline()".into(),
            )
        })?;
        Ok(crate::timeline::Timeline::new(client, self.id.clone()))
    }

    /// Invite one or more Wire users into this room's MLS group.
    /// Delegates to [`Client::invite_members`]. On return Wire has
    /// accepted the Add-commit and the room's metadata (epoch +
    /// member list) has been refreshed via `sync_metadata`.
    ///
    /// Step 14e: returns the refreshed `Arc<Room>` (the same handle
    /// `Client::room(id)` yields from then on, verified via
    /// `Arc::ptr_eq`).
    ///
    /// `user_ids` are qualified `<uuid>@<domain>` strings.
    ///
    /// # Errors
    /// * `KupferdrahtError::InvalidInput` if the owning `Client` has been
    ///   dropped or the input list is empty.
    /// * Transport / MLS / backend errors from `Client::invite_members`.
    pub async fn invite(
        &self,
        user_ids: &[String],
    ) -> Result<std::sync::Arc<Self>, KupferdrahtError> {
        let client = self.client.upgrade().ok_or_else(|| {
            KupferdrahtError::InvalidInput(
                "Room: owning Client has been dropped; cannot call invite()".into(),
            )
        })?;
        client.invite_members(&self.id, user_ids).await
    }

    /// Remove every leaf belonging to one of `user_ids` from this
    /// room's MLS group. Delegates to [`Client::kick_members`]. On
    /// return Wire has accepted the Remove-commit and the room's
    /// metadata (epoch + member list) has been refreshed via
    /// `sync_metadata`. Step 6d / Step 14e (return shape).
    ///
    /// Step 14e: returns the refreshed `Arc<Room>`.
    ///
    /// `user_ids` are qualified `<uuid>@<domain>` strings — the same
    /// shape `Room::members().user_id` exposes.
    ///
    /// **Self-kick** is intentionally rejected. Removing your own
    /// last device from a room collapses your own MLS state in a
    /// way that needs the proper logout / device-revocation
    /// lifecycle.
    ///
    /// # Errors
    /// * `KupferdrahtError::InvalidInput` if the owning `Client` has been
    ///   dropped, the input list is empty, the caller is in the
    ///   list, or no leaf in the local group matches any listed
    ///   user.
    /// * Transport / MLS / backend errors from `Client::kick_members`.
    pub async fn kick(
        &self,
        user_ids: &[String],
    ) -> Result<std::sync::Arc<Self>, KupferdrahtError> {
        let client = self.client.upgrade().ok_or_else(|| {
            KupferdrahtError::InvalidInput(
                "Room: owning Client has been dropped; cannot call kick()".into(),
            )
        })?;
        client.kick_members(&self.id, user_ids).await
    }

    /// Read paginated message history for this room from the local
    /// `timeline_events` store. Step 6-II-a; matrix-rust-sdk analog:
    /// `Room::messages`. Delegates to [`Client::messages`].
    ///
    /// Returned items are oldest-first.
    ///
    /// # Errors
    /// * `InvalidInput` if the owning `Client` has been dropped or
    ///   no `Store` is attached.
    /// * `Storage` from the SQL read.
    pub async fn messages(
        &self,
        opts: &crate::timeline::MessagesOptions,
    ) -> Result<Vec<crate::timeline::TimelineItem>, KupferdrahtError> {
        let client = self.client.upgrade().ok_or_else(|| {
            KupferdrahtError::InvalidInput(
                "Room: owning Client has been dropped; cannot call messages()".into(),
            )
        })?;
        client.messages(&self.id, opts).await
    }

    /// The failed sends of this room — `Failed` rows, oldest first —
    /// so a consumer can wire "sync came back → resend" without a
    /// scheduler in the core. Delegates to [`Client::failed_sends`].
    /// Step 79.
    ///
    /// matrix-rust-sdk analog: the `SendingFailed` local echoes a
    /// `RoomSendQueue::subscribe()` yields.
    ///
    /// # Errors
    /// * `InvalidInput` if the owning `Client` has been dropped or
    ///   no `Store` is attached.
    /// * `Storage` from the SQL read.
    pub async fn failed_sends(
        &self,
    ) -> Result<Vec<crate::timeline::TimelineItem>, KupferdrahtError> {
        let client = self.client.upgrade().ok_or_else(|| {
            KupferdrahtError::InvalidInput(
                "Room: owning Client has been dropped; cannot call failed_sends()".into(),
            )
        })?;
        client.failed_sends(&self.id).await
    }

    /// Remove the caller's own leaf from this conversation's MLS
    /// group via a self-Remove commit. Delegates to
    /// [`Client::leave_room`]. After return, the local openmls group
    /// state has been discarded and `Client::rooms()` no longer
    /// lists this room. Step 6e-1.
    ///
    /// matrix-rust-sdk analog: `Room::leave`.
    ///
    /// # Errors
    /// * `InvalidInput` if the owning `Client` has been dropped or
    ///   the room has no local MLS state (we never joined).
    /// * Backend / network / openmls errors from `Client::leave_room`.
    pub async fn leave(&self) -> Result<(), KupferdrahtError> {
        let client = self.client.upgrade().ok_or_else(|| {
            KupferdrahtError::InvalidInput(
                "Room: owning Client has been dropped; cannot call leave()".into(),
            )
        })?;
        client.leave_room(&self.id).await
    }

    /// Delete this conversation for **all** members (a team-admin
    /// action, distinct from [`Room::leave`]). Delegates to
    /// [`Client::delete_room`], which deletes via
    /// `DELETE /v{N}/teams/{tid}/conversations/{cid}`. Only valid for
    /// team conversations; a 1:1 or non-team group yields `InvalidInput`
    /// (leave those instead).
    ///
    /// # Errors
    /// * `InvalidInput` if the owning `Client` has been dropped or the
    ///   room is not a team conversation.
    /// * Backend / network errors from `Client::delete_room`.
    pub async fn delete(&self) -> Result<(), KupferdrahtError> {
        let client = self.client.upgrade().ok_or_else(|| {
            KupferdrahtError::InvalidInput(
                "Room: owning Client has been dropped; cannot call delete()".into(),
            )
        })?;
        client.delete_room(&self.id).await
    }

    /// Rename this conversation server-side via
    /// `PUT /v{N}/conversations/{domain}/{id}/name`. Delegates to
    /// [`Client::rename_room`]. Wire emits a `conversation.rename`
    /// event to all other members.
    ///
    /// Step 14e: `Client::rename_room` now refreshes the local
    /// cache via `sync_metadata` and returns the freshly-cached
    /// `Arc<Room>` carrying the new `name` — this delegator
    /// propagates that handle directly.
    ///
    /// matrix-rust-sdk analog: `Room::set_name`.
    ///
    /// # Errors
    /// * `InvalidInput` if the owning `Client` has been dropped or
    ///   `new_name` is empty.
    /// * Backend / network errors from `Client::rename_room`.
    pub async fn set_name(&self, new_name: &str) -> Result<std::sync::Arc<Self>, KupferdrahtError> {
        let client = self.client.upgrade().ok_or_else(|| {
            KupferdrahtError::InvalidInput(
                "Room: owning Client has been dropped; cannot call set_name()".into(),
            )
        })?;
        client.rename_room(&self.id, new_name).await
    }

    /// Re-derive the member list from the local MLS group state
    /// (openmls leaves) and resolve display-name / handle via Wire
    /// `POST /v14/list-users`. Delegates to [`Client::sync_members`].
    ///
    /// Returns the freshly-resolved member list (same `Vec` that
    /// lands in the room-members cache + `SQLite` store).
    ///
    /// Re-derive this room's member list from the local MLS group
    /// state and refresh display info via Wire's user directory.
    /// Delegates to [`Client::sync_members`] under the hood.
    ///
    /// Step 14b — renamed from `refresh_members` to match
    /// matrix-rust-sdk and retire the last `refresh_*` verb in the
    /// public API (finding §41 verb taxonomy).
    ///
    /// # Errors
    /// * `KupferdrahtError::InvalidInput` if the owning `Client` has been
    ///   dropped or the room has no local MLS state (usually because
    ///   `Room::join` has not been called yet).
    /// * Network / backend errors from the user-directory lookup.
    /// * `KupferdrahtError::Storage` from the persistence round trip.
    pub async fn sync_members(&self) -> Result<Vec<RoomMember>, KupferdrahtError> {
        let client = self.client.upgrade().ok_or_else(|| {
            KupferdrahtError::InvalidInput(
                "Room: owning Client has been dropped; cannot sync_members()".into(),
            )
        })?;
        client.sync_members(&self.id).await
    }

    /// Issue (or fetch the existing) reusable guest-link for this
    /// conversation via `POST /v{N}/conversations/{cnv}/code` — Step 8.
    /// `password` is `None` for an open link or `Some("…")` (≥ 8 chars)
    /// for a password-protected one. Delegates to
    /// [`Client::create_join_link`].
    ///
    /// Returns the Kupferdraht-public projection of Wire's
    /// `ConversationCodeInfo`: `JoinLink::url` is the `uri` field;
    /// `JoinLink::password_protected` echoes `has_password`.
    ///
    /// # Errors
    /// * `InvalidInput` when the owning `Client` has been dropped or
    ///   `password` is `Some("")`.
    /// * `BackendRejected` (`403 access-denied` for non-owner roles).
    /// * Network / Encoding from the underlying HTTP call.
    pub async fn create_join_link(
        &self,
        password: Option<String>,
    ) -> Result<JoinLink, KupferdrahtError> {
        let client = self.client.upgrade().ok_or_else(|| {
            KupferdrahtError::InvalidInput(
                "Room: owning Client has been dropped; cannot create_join_link()".into(),
            )
        })?;
        client.create_join_link(&self.id, password.as_deref()).await
    }

    /// Fetch the conversation's current guest link via
    /// `GET /v{N}/conversations/{cnv}/code` — Step 8. `Ok(None)` when
    /// no link is on file (Wire returns `404 no-conversation-code`).
    /// Delegates to [`Client::current_join_link`].
    ///
    /// # Errors
    /// * `InvalidInput` when the owning `Client` has been dropped.
    /// * `BackendRejected` for non-2xx responses other than the
    ///   no-code 404.
    /// * Network / Encoding from the underlying HTTP call.
    pub async fn current_join_link(&self) -> Result<Option<JoinLink>, KupferdrahtError> {
        let client = self.client.upgrade().ok_or_else(|| {
            KupferdrahtError::InvalidInput(
                "Room: owning Client has been dropped; cannot current_join_link()".into(),
            )
        })?;
        client.current_join_link(&self.id).await
    }

    /// Revoke the conversation's current guest link via
    /// `DELETE /v{N}/conversations/{cnv}/code` — Step 8. Idempotent:
    /// revoking a non-existent code is `Ok(())`. Delegates to
    /// [`Client::revoke_join_link`].
    ///
    /// # Errors
    /// * `InvalidInput` when the owning `Client` has been dropped.
    /// * `BackendRejected` for non-2xx responses other than the
    ///   no-code 404.
    /// * Network from the underlying HTTP call.
    pub async fn revoke_join_link(&self) -> Result<(), KupferdrahtError> {
        let client = self.client.upgrade().ok_or_else(|| {
            KupferdrahtError::InvalidInput(
                "Room: owning Client has been dropped; cannot revoke_join_link()".into(),
            )
        })?;
        client.revoke_join_link(&self.id).await
    }

    /// Wire's `access` array: which join modes this conversation
    /// permits (`"invite"`, `"code"`, `"link"`, `"private"`). Step 8b.
    /// Empty when the room was hydrated from the local Store
    /// (`load_rooms_from_store`) without a fresh `sync_metadata`
    /// round-trip — `sync_metadata` refreshes the value.
    #[must_use]
    pub fn access_modes(&self) -> Vec<String> {
        self.access.clone()
    }

    /// Wire's `access_role` array: which user classes may join
    /// (`"team_member"`, `"non_team_member"`, `"guest"`,
    /// `"service"`). Step 8b. Empty / fresh-on-sync semantics
    /// match [`Self::access_modes`].
    #[must_use]
    pub fn access_roles(&self) -> Vec<String> {
        self.access_role.clone()
    }

    /// Step 14d — fetch the live `(access, access_role)` pair from
    /// Wire and return it as a typed [`RoomAccess`]. Issues a single
    /// `POST /v{N}/conversations/list` against this room's qualified
    /// id and projects the response's `access` + `access_role`
    /// arrays into the typed shape.
    ///
    /// Replaces the removed `Client::fetch_room_access(room_id) ->
    /// Result<(Vec<String>, Vec<String>)>` — same HTTP call, method
    /// moves to `Room` so room-scoped operations sit on `Room`
    /// (matrix-rust-sdk's per-room-accessor pattern). First method
    /// on `Room` that issues a Wire HTTP call.
    ///
    /// Use this when the cached [`Self::access_modes`] /
    /// [`Self::access_roles`] snapshots are stale (e.g. after
    /// `load_rooms_from_store` without a follow-up `sync_metadata`,
    /// or to confirm the server-side state matches a recent
    /// `Room::set_access` mutation).
    ///
    /// # Errors
    /// * `InvalidInput` if the owning `Client` has been dropped or
    ///   Wire doesn't return a conversation for this room id.
    /// * `BackendRejected` / `Network` / `Encoding` from the HTTP
    ///   layer.
    pub async fn access(&self) -> Result<RoomAccess, KupferdrahtError> {
        let client = self.client.upgrade().ok_or_else(|| {
            KupferdrahtError::InvalidInput(
                "Room: owning Client has been dropped; cannot call access()".into(),
            )
        })?;
        client.list_room_access(&self.id).await
    }

    /// Replace the conversation's `access` and `access_role` arrays
    /// via `PUT /v{N}/conversations/{domain}/{cnv}/access`. Step 8b.
    /// `Room::create_join_link` requires `access` to include
    /// `"code"`; `Client::join_via_link` redeemers also need
    /// `access_role` to include `"guest"` (and typically
    /// `"non_team_member"`).
    ///
    /// Delegates to [`Client::set_room_access`]. Step 14e: returns
    /// the refreshed `Arc<Room>` (cache updated post-PUT via
    /// `sync_metadata`).
    ///
    /// # Errors
    /// * `InvalidInput` when the owning `Client` has been dropped
    ///   or either list is empty.
    /// * `BackendRejected` (`403 access-denied` /
    ///   `403 invalid-op` / `403 action-denied` for unauthorized
    ///   callers; `404 no-conversation` on missing room).
    pub async fn set_access(
        &self,
        access: Vec<String>,
        access_role: Vec<String>,
    ) -> Result<std::sync::Arc<Self>, KupferdrahtError> {
        let client = self.client.upgrade().ok_or_else(|| {
            KupferdrahtError::InvalidInput(
                "Room: owning Client has been dropped; cannot set_access()".into(),
            )
        })?;
        client
            .set_room_access(&self.id, &access, &access_role)
            .await
    }

    /// Convenience over [`Self::set_access`] that opens the
    /// conversation up for guest-link joins via the canonical
    /// `access=["invite","code"]`,
    /// `access_role=["team_member","non_team_member","guest"]`
    /// configuration. Step 8b. Most Kupferdraht-created conversations
    /// default to `access=["invite"]` (members only); calling this
    /// once is the prerequisite for `Room::create_join_link`.
    ///
    /// # Errors
    /// Same as [`Self::set_access`].
    pub async fn open_to_guests(&self) -> Result<std::sync::Arc<Self>, KupferdrahtError> {
        self.set_access(
            vec!["invite".into(), "code".into()],
            vec![
                "team_member".into(),
                "non_team_member".into(),
                "guest".into(),
            ],
        )
        .await
    }
}

/// Wire's reusable guest-link, projected into a Kupferdraht-public type
/// (the matrix-rust-sdk public surface stays free of Wire field
/// names per ADR-0009). Issued by `Room::create_join_link` and
/// fetched by `Room::current_join_link`.
///
/// `url` is the link the backend issues: its operator's account page
/// with the link's `key` and `code` in the query
/// (`https://account.<operator>/conversation-join/?key=…&code=…`).
/// `password_protected` is
/// `true` when the link was issued with a `password` argument and
/// requires that same password on redeem.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct JoinLink {
    pub url: String,
    pub password_protected: bool,
}

/// Typed projection of Wire's `(access, access_role)` pair on a
/// conversation. Step 14d — replaces the bare
/// `(Vec<String>, Vec<String>)` tuple `Client::fetch_room_access`
/// returned, and lands the first method on `Room` that issues a Wire
/// HTTP call (`Room::access()`).
///
/// `modes` mirrors Wire's `access` array — which join modes the
/// conversation permits (`"invite"`, `"code"`, `"link"`, `"private"`).
///
/// `roles` mirrors Wire's `access_role` array — which user classes
/// may join (`"team_member"`, `"non_team_member"`, `"guest"`,
/// `"service"`).
///
/// matrix-rust-sdk has no direct analog because Matrix' room access
/// is state-event-driven (`m.room.join_rules`); Wire's `access` /
/// `access_role` are first-class fields on `Conversation`.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct RoomAccess {
    pub modes: Vec<String>,
    pub roles: Vec<String>,
}

/// Overwrite the in-memory `members` list on a `Room` snapshot. Used
/// by `Client::sync_members` after the openmls-derived + Wire-
/// backfilled member set is computed.
pub(crate) fn with_members(mut room: Room, members: Vec<RoomMember>) -> Room {
    room.members = members;
    room
}

/// Replace the in-memory `epoch` on an owned `Room` snapshot. Used by
/// `Client::join_room` after the commit bundle is accepted by Wire so
/// the cache reflects the new local group state.
pub(crate) fn with_epoch(mut room: Room, new_epoch: u64) -> Room {
    room.epoch = new_epoch;
    room
}

/// Adapt Wire's `OwnConversation` wire-shape into a `Room`. Returns
/// `None` when the conversation's protocol is `"proteus"` — Kupferdraht
/// is MLS-only (ADR-0001) and Proteus conversations are filtered at
/// this boundary so no Wire-terminology leaks past it.
///
/// `client` is a weak back-reference used by instance methods like
/// [`Room::join`] to call into the owning `Client`; pass `Weak::new()`
/// for test harnesses that build `Room` as a pure data container.
pub(crate) fn room_from_conversation(conv: OwnConversation, client: Weak<Client>) -> Option<Room> {
    if matches!(conv.protocol, Protocol::Proteus) {
        return None;
    }
    // Step 95c: a conversation without an id or domain is unaddressable
    // (`@domain` fails every per-room endpoint) — skipped.
    if conv.qualified_id.id.is_empty() || conv.qualified_id.domain.is_empty() {
        tracing::warn!("conversation without an id or domain skipped");
        return None;
    }
    let room_type = map_room_type(conv.conv_type);
    let (members, counterpart) = build_room_members(&conv, room_type);
    Some(Room {
        id: qualify(&conv.qualified_id),
        name: conv.name.unwrap_or_default(),
        room_type,
        epoch: conv.epoch,
        counterpart,
        members,
        // An empty group id is no group (Step 95c): every MLS entry
        // point would receive an empty `GroupId`.
        group_id: conv.group_id.filter(|g| !g.is_empty()),
        access: conv.access,
        access_role: conv.access_role,
        client,
    })
}

fn map_room_type(t: ConvType) -> RoomType {
    match t {
        ConvType::Group => RoomType::Group,
        ConvType::SelfRoom => RoomType::SelfRoom,
        ConvType::DirectMessage => RoomType::DirectMessage,
        ConvType::Channel => RoomType::Channel,
    }
}

fn build_room_members(
    conv: &OwnConversation,
    room_type: RoomType,
) -> (Vec<RoomMember>, Option<RoomMember>) {
    let self_member = RoomMember {
        user_id: qualify(&conv.members.self_member.qualified_id),
        is_self: true,
        display_name: None,
        handle: None,
        role: role_from_wire(conv.members.self_member.conversation_role.as_deref()),
    };
    let others: Vec<RoomMember> = conv
        .members
        .others
        .iter()
        .map(|o| RoomMember {
            user_id: qualify(&o.qualified_id),
            is_self: false,
            display_name: None,
            handle: None,
            role: role_from_wire(o.conversation_role.as_deref()),
        })
        .collect();
    let counterpart = if room_type == RoomType::DirectMessage && others.len() == 1 {
        Some(others[0].clone())
    } else {
        None
    };
    let mut members = Vec::with_capacity(1 + others.len());
    members.push(self_member);
    members.extend(others);
    (members, counterpart)
}

/// Map Wire's `conversation_role` onto the neutral role enum. The
/// Wire literal comparison lives in `wire/dto.rs` (ADR-0009); absent
/// and custom roles are members (Step 72a).
fn role_from_wire(role: Option<&str>) -> RoomMemberRole {
    if crate::wire::dto::conversation_role_is_admin(role) {
        RoomMemberRole::Administrator
    } else {
        RoomMemberRole::Member
    }
}

fn qualify(qid: &QualifiedId) -> String {
    qid.to_string()
}

/// Parse a qualified `<uuid>@<domain>` room id into a `QualifiedId` for
/// Wire requests that address a conversation by its federated parts.
/// Returns `None` for a malformed string. (User ids go through
/// [`UserId`](crate::identity::UserId), not this function.)
pub(crate) fn parse_room_id(room_id: &str) -> Option<QualifiedId> {
    let (id, domain) = room_id.split_once('@')?;
    if id.is_empty() || domain.is_empty() {
        return None;
    }
    Some(QualifiedId {
        id: id.to_string(),
        domain: domain.to_string(),
    })
}

/// Collect every non-self member's qualified id across a set of
/// rooms, deduplicated. This is the input list for
/// `HttpClient::list_users` during `Client::sync_metadata`.
pub(crate) fn non_self_qualified_ids(rooms: &[Room]) -> Vec<QualifiedId> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for room in rooms {
        for m in &room.members {
            if m.is_self {
                continue;
            }
            if !seen.insert(m.user_id.clone()) {
                continue;
            }
            if let Ok(uid) = crate::identity::UserId::parse(&m.user_id) {
                out.push(QualifiedId::from(&uid));
            }
        }
    }
    out
}

/// Convert a `Room` into the `(RoomRecord, Vec<RoomMemberRecord>)`
/// tuple that the `Store` persists. `last_synced_at_ms` is supplied
/// by the caller so a single `sync_metadata` call stamps every
/// written row with the same timestamp.
pub(crate) fn room_to_records(
    room: &Room,
    last_synced_at_ms: i64,
) -> (RoomRecord, Vec<RoomMemberRecord>) {
    let record = RoomRecord {
        room_id: room.id.clone(),
        group_id: room.group_id.clone(),
        name: room.name.clone(),
        room_type: room.room_type.as_str().to_string(),
        epoch: i64::try_from(room.epoch).unwrap_or(i64::MAX),
        counterpart_user_id: room.counterpart.as_ref().map(|m| m.user_id.clone()),
        last_synced_at_ms,
    };
    let members = room
        .members
        .iter()
        .map(|m| RoomMemberRecord {
            room_id: room.id.clone(),
            user_id: m.user_id.clone(),
            is_self: m.is_self,
            display_name: m.display_name.clone(),
            handle: m.handle.clone(),
            role: m.role.as_db_str().to_string(),
        })
        .collect();
    (record, members)
}

/// Reconstruct `Room`s from persisted `(RoomRecord, members)`
/// tuples. Rows whose `room_type` column carries an unknown string
/// are skipped (a schema upgrade that introduces a new variant must
/// not break an older binary).
///
/// `client` is the weak back-reference to the owning `Client`; every
/// reconstructed room carries a clone so `Room::join` works on a
/// hydrated cache exactly as on a freshly-synced one.
pub(crate) fn rooms_from_records(
    records: Vec<(RoomRecord, Vec<RoomMemberRecord>)>,
    client: &Weak<Client>,
) -> Vec<Room> {
    records
        .into_iter()
        .filter_map(|(r, mrs)| {
            let room_type = RoomType::from_str_persisted(&r.room_type)?;
            let members: Vec<RoomMember> = mrs
                .into_iter()
                .map(|m| RoomMember {
                    user_id: m.user_id,
                    is_self: m.is_self,
                    display_name: m.display_name,
                    handle: m.handle,
                    role: RoomMemberRole::from_db_str(&m.role),
                })
                .collect();
            let counterpart = r
                .counterpart_user_id
                .and_then(|cp_id| members.iter().find(|m| m.user_id == cp_id).cloned());
            let epoch = u64::try_from(r.epoch).unwrap_or(0);
            Some(Room {
                id: r.room_id,
                name: r.name,
                room_type,
                epoch,
                counterpart,
                members,
                group_id: r.group_id,
                // Step 8b: not persisted to the Store yet — access /
                // access_role are refreshed by the next sync_metadata
                // round-trip and exposed via `Room::access*` accessors.
                access: Vec::new(),
                access_role: Vec::new(),
                client: client.clone(),
            })
        })
        .collect()
}

/// Pre-populate the self-member's `display_name` / `handle` from
/// the caller's `Account` snapshot, so the CLI's `rooms` output
/// doesn't render own membership as an unnamed row. `list-users`
/// deliberately excludes self (the self-resolve path lives in
/// `GET /v14/self` and is already owned by `Account`).
pub(crate) fn apply_self_profile(mut room: Room, display_name: &str, handle: Option<&str>) -> Room {
    for m in &mut room.members {
        if m.is_self {
            m.display_name = Some(display_name.to_string());
            m.handle = handle.map(str::to_string);
        }
    }
    room
}

/// Apply a user-directory lookup (from
/// `HttpClient::list_users`) to every member of the supplied room,
/// including the counterpart slot. A member whose qualified id is
/// not in the lookup keeps its `None` `display_name` / `handle`.
pub(crate) fn apply_user_profiles(mut room: Room, lookup: &HashMap<String, UserProfile>) -> Room {
    for m in &mut room.members {
        if let Some(p) = lookup.get(&m.user_id) {
            m.display_name = Some(p.name.clone());
            m.handle.clone_from(&p.handle);
        }
    }
    if let Some(cp) = room.counterpart.as_mut()
        && let Some(p) = lookup.get(&cp.user_id)
    {
        cp.display_name = Some(p.name.clone());
        cp.handle.clone_from(&p.handle);
    }
    room
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::dto::{ConvMemberOther, ConvMemberSelf, OwnConvMembers};

    fn qid(id: &str, domain: &str) -> QualifiedId {
        QualifiedId {
            id: id.to_string(),
            domain: domain.to_string(),
        }
    }

    fn member_self(id: &str, domain: &str) -> ConvMemberSelf {
        ConvMemberSelf {
            conversation_role: None,
            qualified_id: qid(id, domain),
        }
    }

    fn member_other(id: &str, domain: &str) -> ConvMemberOther {
        ConvMemberOther {
            conversation_role: None,
            qualified_id: qid(id, domain),
        }
    }

    fn make(
        conv_type: ConvType,
        protocol: Protocol,
        others_ids: &[(&str, &str)],
    ) -> OwnConversation {
        OwnConversation {
            qualified_id: qid("room-id", "example.test"),
            conv_type,
            protocol,
            epoch: 7,
            group_id: Some("Z3JwLWFh".into()),
            name: Some("Engineering".into()),
            access: Vec::new(),
            access_role: Vec::new(),
            team: None,
            members: OwnConvMembers {
                self_member: member_self("me", "example.test"),
                others: others_ids.iter().map(|(i, d)| member_other(i, d)).collect(),
            },
        }
    }

    #[test]
    fn proteus_conversations_are_filtered() {
        let conv = make(
            ConvType::DirectMessage,
            Protocol::Proteus,
            &[("bob", "example.test")],
        );
        assert!(room_from_conversation(conv, Weak::new()).is_none());
    }

    #[test]
    fn mls_and_mixed_protocol_rooms_pass_through() {
        for protocol in [Protocol::Mls, Protocol::Mixed] {
            let conv = make(ConvType::Group, protocol, &[("bob", "example.test")]);
            assert!(
                room_from_conversation(conv, Weak::new()).is_some(),
                "{protocol:?} must pass through"
            );
        }
    }

    #[test]
    fn conv_type_maps_one_to_one() {
        for (wire, expected) in [
            (ConvType::Group, RoomType::Group),
            (ConvType::SelfRoom, RoomType::SelfRoom),
            (ConvType::DirectMessage, RoomType::DirectMessage),
            (ConvType::Channel, RoomType::Channel),
        ] {
            let conv = make(wire, Protocol::Mls, &[]);
            assert_eq!(
                room_from_conversation(conv, Weak::new())
                    .expect("mapped")
                    .room_type(),
                expected,
                "Wire ConvType::{wire:?} must map to RoomType::{expected:?}"
            );
        }
    }

    #[test]
    fn counterpart_is_some_for_one_on_one_direct_message() {
        let conv = make(
            ConvType::DirectMessage,
            Protocol::Mls,
            &[("bob", "example.test")],
        );
        let room = room_from_conversation(conv, Weak::new()).expect("mapped");
        let cp = room
            .counterpart()
            .expect("DM with one other has counterpart");
        assert_eq!(cp.user_id, "bob@example.test");
        assert!(!cp.is_self);
    }

    #[test]
    fn counterpart_is_none_for_groups_and_channels() {
        for t in [RoomType::Group, RoomType::Channel] {
            let conv = make(
                match t {
                    RoomType::Group => ConvType::Group,
                    RoomType::Channel => ConvType::Channel,
                    _ => unreachable!(),
                },
                Protocol::Mls,
                &[("alice", "example.test"), ("bob", "example.test")],
            );
            let room = room_from_conversation(conv, Weak::new()).expect("mapped");
            assert!(room.counterpart().is_none(), "{t:?} has no counterpart");
        }
    }

    #[test]
    fn counterpart_is_none_for_self_room() {
        let conv = make(ConvType::SelfRoom, Protocol::Mls, &[]);
        let room = room_from_conversation(conv, Weak::new()).expect("mapped");
        assert!(room.counterpart().is_none());
    }

    #[test]
    fn counterpart_is_none_for_dm_with_unexpected_member_count() {
        // A 1:1 DM with zero or two+ "others" is malformed per Wire's
        // DM contract. Counterpart returns None rather than guessing.
        let conv = make(
            ConvType::DirectMessage,
            Protocol::Mls,
            &[("alice", "example.test"), ("bob", "example.test")],
        );
        let room = room_from_conversation(conv, Weak::new()).expect("mapped");
        assert!(room.counterpart().is_none());
    }

    #[tokio::test]
    async fn room_fields_round_trip_from_dto() {
        let conv = make(
            ConvType::Group,
            Protocol::Mls,
            &[("bob", "example.test"), ("carol", "other.test")],
        );
        let room = room_from_conversation(conv, Weak::new()).expect("mapped");
        assert_eq!(room.id(), "room-id@example.test");
        assert_eq!(room.name(), "Engineering");
        assert_eq!(room.epoch(), 7);
        // members contains self + others, self first.
        let ms = room.members().await.expect("members");
        assert_eq!(ms.len(), 3);
        assert!(ms[0].is_self);
        assert_eq!(ms[0].user_id, "me@example.test");
        assert!(!ms[1].is_self);
        assert_eq!(ms[1].user_id, "bob@example.test");
        assert_eq!(ms[2].user_id, "carol@other.test");
    }
}
