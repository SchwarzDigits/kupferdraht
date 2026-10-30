// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `Client` room operations — the room-list accessors, room /
//! DM creation, room-metadata mutators, and guest-link (join-link)
//! lifecycle. Split out of [`super`] in Step 29; every item
//! here is a method on [`Client`] (or the `unqualified_room_id` free
//! helper the join-link methods share). The public surface mirrors
//! `matrix_sdk::Client` / `matrix_sdk::Room` — see
//! `docs/api-mapping-matrix-rust-sdk.md`.

#![allow(clippy::doc_markdown)]

use std::sync::{Arc, Mutex as StdMutex};

use base64::Engine as _;

use crate::error::KupferdrahtError;
use crate::identity::UserId;
use crate::rooms::{JoinLink, Room, RoomAccess, RoomType, parse_room_id};
use crate::wire::dto::QualifiedId;

use super::Client;

/// Split a qualified `<uuid>@<domain>` room id and return the
/// unqualified UUID half. Step 8's code-link endpoints (`POST /
/// GET / DELETE /v14/conversations/{cnv}/code`) take the local
/// id only, unlike the rename / member-management routes which
/// take the qualified pair.
fn unqualified_room_id<'a>(room_id: &'a str, op: &str) -> Result<&'a str, KupferdrahtError> {
    match room_id.split_once('@') {
        Some((local, domain)) if !local.is_empty() && !domain.is_empty() => Ok(local),
        _ => Err(KupferdrahtError::InvalidInput(format!(
            "{op}: room_id `{room_id}` must be qualified <uuid>@<domain>"
        ))),
    }
}

impl Client {
    /// `PUT /v{N}/conversations/{domain}/{id}/name` — rename a
    /// conversation. Wire emits a `conversation.rename` event to all
    /// members; the receive path will see it on the next sync.
    /// matrix-rust-sdk equivalent: `Room::set_name`.
    ///
    /// Implementation lives on `Client` so the public-API entry point
    /// `Room::set_name` can delegate here without duplicating the
    /// transport / token plumbing.
    ///
    /// Step 14e: after the PUT succeeds, the local rooms cache is
    /// refreshed via `sync_metadata` and the freshly-cached
    /// `Arc<Room>` (now carrying the new `name`) is returned. This
    /// is the same handle `Client::room(room_id)` yields from then on
    /// — verified by `Arc::ptr_eq`.
    ///
    /// # Errors
    /// * `InvalidInput` if `room_id` is unqualified, `new_name` is
    ///   empty, or the room is not in the cache after refresh
    ///   (defensive — should not happen if the PUT succeeded).
    /// * `BackendRejected` on `403 invalid-op` (insufficient role)
    ///   or `404 no-conversation` (unknown room).
    pub async fn rename_room(
        &self,
        room_id: &str,
        new_name: &str,
    ) -> Result<Arc<Room>, KupferdrahtError> {
        if new_name.is_empty() {
            return Err(KupferdrahtError::InvalidInput(
                "rename_room: new_name must be non-empty".into(),
            ));
        }
        self.http
            .rename_conversation(
                self.api_version,
                &self.current_access_token(),
                room_id,
                new_name,
            )
            .await?;
        self.sync_metadata().await?;
        self.room(room_id).await.ok_or_else(|| {
            KupferdrahtError::InvalidInput(format!(
                "rename_room: {room_id} missing from cache after refresh"
            ))
        })
    }

    /// `DELETE /v{N}/teams/{tid}/conversations/{cid}` — delete a team
    /// conversation for **all** members (a team-admin action, distinct
    /// from `leave_room`). The owning team id comes from the
    /// conversation's `team` field, falling back to the signed-in user's
    /// own team (`GET /self`) when Wire's conversation-list response
    /// omits it. An account that belongs to no team yields
    /// `InvalidInput`. On success the local cache is refreshed, so the
    /// room drops out.
    ///
    /// # Errors
    /// * `InvalidInput` if `room_id` is unqualified, Wire doesn't return
    ///   the conversation, or neither the conversation nor the account
    ///   has an owning team.
    /// * `BackendRejected` on `403` (insufficient role). A `404`
    ///   (already gone) is treated as idempotent success.
    pub async fn delete_room(&self, room_id: &str) -> Result<(), KupferdrahtError> {
        let qid = parse_room_id(room_id).ok_or_else(|| {
            KupferdrahtError::InvalidInput(format!(
                "delete_room: room_id `{room_id}` must be qualified <uuid>@<domain>"
            ))
        })?;
        let conv_local_id = qid.id.clone();
        let resp = self
            .http
            .list_conversations(self.api_version, &self.current_access_token(), &[qid])
            .await?;
        let conv = resp.found.into_iter().next().ok_or_else(|| {
            KupferdrahtError::InvalidInput(format!(
                "delete_room: Wire did not return a conversation for {room_id}"
            ))
        })?;
        // Wire's conversation-list response often omits the owning
        // `team`; fall back to the signed-in user's own team (the team
        // whose conversations they can administer) when it's absent.
        // An empty team is no team (Step 95c — it would be the empty
        // segment of the team-scoped DELETE path).
        let team_id = match conv.team.filter(|t| !t.is_empty()) {
            Some(team) => team,
            None => self
                .http
                .fetch_self(self.api_version, &self.current_access_token())
                .await?
                .team
                .filter(|t| !t.is_empty())
                .ok_or_else(|| {
                    KupferdrahtError::InvalidInput(format!(
                        "delete_room: {room_id} has no owning team and the account is not in \
                         a team — only team conversations can be deleted (use leave_room)"
                    ))
                })?,
        };
        self.http
            .delete_team_conversation(
                self.api_version,
                &self.current_access_token(),
                &team_id,
                &conv_local_id,
            )
            .await?;
        self.sync_metadata().await
    }

    /// Remove a room from the in-memory cache by qualified id; returns
    /// `true` if one was removed. Used by the inbound
    /// `conversation.delete` handler so a conversation another admin
    /// deleted drops out immediately, without waiting for the next
    /// metadata refresh. The persisted snapshot self-corrects on the
    /// next `sync_metadata`. Step 23.
    pub(crate) fn remove_cached_room(&self, room_id: &str) -> bool {
        let mut rooms = self.rooms.write().expect("rooms lock poisoned");
        let before = rooms.len();
        rooms.retain(|r| r.id() != room_id);
        rooms.len() != before
    }

    /// Issue (or fetch) a reusable guest-link for the conversation
    /// at `room_id` (qualified `<uuid>@<domain>`). The Kupferdraht-public
    /// entrypoint `Room::create_join_link` delegates here. Step 8.
    ///
    /// `password` is `None` for an open link or `Some("…")` (≥ 8
    /// chars per Wire's schema; we enforce non-empty here and let
    /// Wire reject the rest).
    ///
    /// # Errors
    /// * `InvalidInput` if `room_id` is unqualified or `password`
    ///   is `Some("")`.
    /// * `BackendRejected` / `Network` / `Encoding` from
    ///   `HttpClient::create_conversation_code`.
    pub async fn create_join_link(
        &self,
        room_id: &str,
        password: Option<&str>,
    ) -> Result<JoinLink, KupferdrahtError> {
        let local_id = unqualified_room_id(room_id, "create_join_link")?;
        if matches!(password, Some(p) if p.is_empty()) {
            return Err(KupferdrahtError::InvalidInput(
                "create_join_link: password, when supplied, must be non-empty".into(),
            ));
        }
        let info = self
            .http
            .create_conversation_code(
                self.api_version,
                &self.current_access_token(),
                local_id,
                password,
            )
            .await?;
        Ok(JoinLink {
            url: info.uri,
            password_protected: info.has_password,
        })
    }

    /// Fetch the conversation's current guest link or `Ok(None)`
    /// when none is on file. Step 8. Delegates to
    /// `HttpClient::get_conversation_code`.
    ///
    /// # Errors
    /// * `InvalidInput` if `room_id` is unqualified.
    /// * `BackendRejected` / `Network` / `Encoding` from the HTTP
    ///   layer (other than the no-code 404 which surfaces as `None`).
    pub async fn current_join_link(
        &self,
        room_id: &str,
    ) -> Result<Option<JoinLink>, KupferdrahtError> {
        let local_id = unqualified_room_id(room_id, "current_join_link")?;
        let info = self
            .http
            .get_conversation_code(self.api_version, &self.current_access_token(), local_id)
            .await?;
        Ok(info.map(|i| JoinLink {
            url: i.uri,
            password_protected: i.has_password,
        }))
    }

    /// Revoke the conversation's current guest link. Idempotent:
    /// revoking a non-existent code is `Ok(())`. Step 8. Delegates
    /// to `HttpClient::delete_conversation_code`.
    ///
    /// # Errors
    /// * `InvalidInput` if `room_id` is unqualified.
    /// * `BackendRejected` / `Network` from the HTTP layer.
    pub async fn revoke_join_link(&self, room_id: &str) -> Result<(), KupferdrahtError> {
        let local_id = unqualified_room_id(room_id, "revoke_join_link")?;
        self.http
            .delete_conversation_code(self.api_version, &self.current_access_token(), local_id)
            .await
    }

    /// `PUT /v{N}/conversations/{domain}/{cnv}/access` — Step 8b.
    /// Replaces the conversation's access modes + roles. Common
    /// flow: open up an invite-only group for guest links via
    /// `access=["invite","code"]`,
    /// `access_role=["team_member","non_team_member","guest"]`.
    /// `Room::open_to_guests` is the convenience wrapper around
    /// that canonical configuration.
    ///
    /// Step 14e: after the PUT succeeds, the local rooms cache is
    /// refreshed via `sync_metadata` and the freshly-cached
    /// `Arc<Room>` (now carrying the new `access` / `access_role`
    /// fields) is returned.
    ///
    /// # Errors
    /// * `InvalidInput` when `room_id` is unqualified, either list
    ///   is empty, or the room is not in the cache after refresh.
    /// * `BackendRejected` / `Network` from the HTTP layer.
    pub async fn set_room_access(
        &self,
        room_id: &str,
        access: &[String],
        access_role: &[String],
    ) -> Result<Arc<Room>, KupferdrahtError> {
        self.http
            .set_conversation_access(
                self.api_version,
                &self.current_access_token(),
                room_id,
                access,
                access_role,
            )
            .await?;
        self.sync_metadata().await?;
        self.room(room_id).await.ok_or_else(|| {
            KupferdrahtError::InvalidInput(format!(
                "set_room_access: {room_id} missing from cache after refresh"
            ))
        })
    }

    /// Crate-internal: fetch the live `(access, access_role)` arrays
    /// for `room_id` by re-running `POST /v{N}/conversations/list`
    /// with a single id and projecting the response's access fields
    /// into a typed [`RoomAccess`]. Step 14d — replaces the public
    /// `Client::fetch_room_access(...) -> Result<(Vec<String>,
    /// Vec<String>), _>`; the public surface is now [`Room::access`]
    /// which delegates here through the `Weak<Client>` back-ref.
    ///
    /// # Errors
    /// * `InvalidInput` when `room_id` is unqualified or Wire
    ///   doesn't return the room.
    /// * `BackendRejected` / `Network` / `Encoding` from the HTTP
    ///   layer.
    pub(crate) async fn list_room_access(
        &self,
        room_id: &str,
    ) -> Result<RoomAccess, KupferdrahtError> {
        let qid = parse_room_id(room_id).ok_or_else(|| {
            KupferdrahtError::InvalidInput(format!(
                "list_room_access: room_id `{room_id}` must be qualified <uuid>@<domain>"
            ))
        })?;
        let resp = self
            .http
            .list_conversations(self.api_version, &self.current_access_token(), &[qid])
            .await?;
        let conv = resp.found.into_iter().next().ok_or_else(|| {
            KupferdrahtError::InvalidInput(format!(
                "list_room_access: Wire did not return a conversation for {room_id}"
            ))
        })?;
        Ok(RoomAccess {
            modes: conv.access,
            roles: conv.access_role,
        })
    }

    /// Redeem a guest link and join the conversation it names. Step 8;
    /// Step 122 finds the room by the link instead of by a domain:
    ///
    /// 1. `GET /v{N}/conversations/join?key&code` — the conversation the
    ///    link names (its id). A redeem only ever reaches the own
    ///    backend's conversations (`open.md` §74).
    /// 2. `POST /v{N}/conversations/join` with `{key, code, password?}`
    ///    — Wire adds the caller to the conversation member list (`204`:
    ///    a member already).
    /// 3. `Client::sync_metadata` — the room list now holds the room.
    /// 4. `Client::join_room` (Step 3 external commit) — unless the room's
    ///    group is joined already.
    ///
    /// # Errors
    /// * `InvalidInput` if `code` or `key` is empty.
    /// * `BackendRejected` (`404 no-conversation-code` for a link the
    ///   backend does not know, `403 invalid-conversation-password`,
    ///   `409 guest-links-disabled`, …) from the lookup or the redeem.
    /// * `Generic` when the redeem succeeded but the refreshed room list
    ///   lacks the room.
    /// * Anything `sync_metadata` / `join_room` can return.
    pub async fn join_via_link(
        &self,
        code: String,
        key: String,
        password: Option<String>,
    ) -> Result<Arc<crate::rooms::Room>, KupferdrahtError> {
        if code.is_empty() || key.is_empty() {
            return Err(KupferdrahtError::InvalidInput(
                "join_via_link: code and key must be non-empty".into(),
            ));
        }
        let conversation = self.redeem_link(&code, &key, password.as_deref()).await?;
        self.sync_metadata().await?;
        let missing = || {
            KupferdrahtError::Generic(format!(
                "join_via_link: joined conversation {conversation}, but the room list lacks it"
            ))
        };
        let room = self
            .rooms()
            .await
            .into_iter()
            .find(|room| {
                room.id()
                    .split_once('@')
                    .is_some_and(|(id, _)| id == conversation)
            })
            .ok_or_else(missing)?;
        let room_id = room.id();
        if !self.room_group_joined(&room)? {
            self.join_room(&room_id).await?;
        }
        self.room(&room_id).await.ok_or_else(missing)
    }

    /// Steps 1 and 2 of [`Self::join_via_link`]: the conversation the link
    /// names (its unqualified id), then the redeem.
    async fn redeem_link(
        &self,
        code: &str,
        key: &str,
        password: Option<&str>,
    ) -> Result<String, KupferdrahtError> {
        let named = self
            .http
            .get_conversation_by_code(self.api_version, &self.current_access_token(), key, code)
            .await?;
        self.http
            .join_conversation_by_code(
                self.api_version,
                &self.current_access_token(),
                key,
                code,
                password,
            )
            .await?;
        Ok(named.id)
    }

    /// Whether `room`'s MLS group is here already — a room joined before
    /// is not joined again (an external commit would replace the own leaf
    /// for nothing). `false` without MLS state or a group id: the join
    /// reports those.
    fn room_group_joined(&self, room: &crate::rooms::Room) -> Result<bool, KupferdrahtError> {
        let (Some(mls), Some(group_id)) = (self.mls_client.as_ref(), room.group_id_bytes()) else {
            return Ok(false);
        };
        crate::encryption::lock_mls(mls).group_exists(&group_id)
    }

    /// Snapshot of the rooms cache populated by
    /// [`Self::sync_once`]. Empty before the first sync.
    ///
    /// Both this and [`Self::room`] are kept `async` so the existing
    /// public-API contract (returning a future) stays stable, even
    /// though the underlying `std::sync::RwLock` read is non-blocking
    /// — Step 14a switched the lock type so [`Self::dm_room`] can be
    /// sync (finding §41 verb taxonomy).
    ///
    /// # Panics
    /// Panics if the in-memory rooms-cache `RwLock` is poisoned.
    #[allow(clippy::unused_async)]
    // Contract-async: the signature is public API (matrix-rust-sdk
    // shape / FFI await-ability); 1.98's new lint name is allowed
    // alongside `unknown_lints` so older toolchains don't trip over
    // the unknown lint (deny(warnings)).
    #[allow(unknown_lints, clippy::unused_async_trait_impl)]
    #[allow(clippy::unused_async)]
    pub async fn rooms(&self) -> Vec<Arc<Room>> {
        self.rooms.read().expect("rooms lock poisoned").clone()
    }

    /// Look up a single room by qualified id (`<uuid>@<domain>`).
    /// `None` if the room is not in the current cache snapshot.
    ///
    /// # Panics
    /// Panics if the in-memory rooms-cache `RwLock` is poisoned.
    #[allow(clippy::unused_async)]
    // Contract-async: the signature is public API (matrix-rust-sdk
    // shape / FFI await-ability); 1.98's new lint name is allowed
    // alongside `unknown_lints` so older toolchains don't trip over
    // the unknown lint (deny(warnings)).
    #[allow(unknown_lints, clippy::unused_async_trait_impl)]
    #[allow(clippy::unused_async)]
    pub async fn room(&self, room_id: &str) -> Option<Arc<Room>> {
        self.rooms
            .read()
            .expect("rooms lock poisoned")
            .iter()
            .find(|r| r.id() == room_id)
            .cloned()
    }

    /// Step 14g — cache-first lookup with lazy store-hydrate
    /// fallback. Tries [`Self::room`] first; on `None`, calls
    /// [`Self::load_rooms_from_store`] and retries the lookup.
    /// Returns `None` on a true miss (neither cache nor store has
    /// the room) or when no store is attached / a store error
    /// occurs — the surface mirrors `Client::room`'s `Option`
    /// shape rather than introducing a `Result` cost on the happy
    /// path.
    ///
    /// Closes `v1-api-ergonomics-audit.md §1.1` — the cold-cache
    /// case `Client::room` returns `None` on after `resume()`. Use
    /// `room_or_load` for the "I want the room if it exists
    /// anywhere" pattern; keep `Client::room` for the non-blocking
    /// lookup where `None` carries the explicit "not in cache"
    /// signal.
    pub async fn room_or_load(&self, room_id: &str) -> Option<Arc<Room>> {
        if let Some(room) = self.room(room_id).await {
            return Some(room);
        }
        // Best-effort hydrate. A store error here (no store attached,
        // I/O failure, decrypt fault) is treated as "the room isn't
        // available" — returning `Option` keeps `Client::room`'s
        // shape and avoids cascading a `KupferdrahtError` through callers
        // that only wanted "give me the room if you have it".
        let _ = self.load_rooms_from_store().await;
        self.room(room_id).await
    }

    /// Step 14c — rooms the caller has joined. matrix-rust-sdk
    /// analog: `Client::joined_rooms()`.
    ///
    /// Kupferdraht's current data model only surfaces rooms the user is a
    /// member of through `sync_once`, so `joined_rooms()` is the
    /// same set as [`Self::rooms`] today. The two methods are
    /// distinguished for forward-compatibility: when Kupferdraht starts
    /// materializing pending invitations as cached `Room` objects,
    /// `rooms()` will broaden while `joined_rooms()` keeps the
    /// "joined-only" guarantee.
    ///
    /// # Panics
    /// Panics if the in-memory rooms-cache `RwLock` is poisoned.
    #[must_use]
    pub fn joined_rooms(&self) -> Vec<Arc<Room>> {
        // Kupferdraht's data model: every cached room is joined. Future
        // work (invitation materialization) will narrow `rooms()`
        // and keep `joined_rooms()` joined-only — both methods
        // already exist so the caller-facing contract is stable.
        self.rooms.read().expect("rooms lock poisoned").clone()
    }

    /// Step 14c — rooms the caller has been invited to but has not
    /// yet joined. matrix-rust-sdk analog: `Client::invited_rooms()`.
    ///
    /// Kupferdraht does not materialize pending invitations as `Room`
    /// objects today: `sync_once` only surfaces conversations the
    /// user is already a member of, and inbound
    /// `conversation.create` / `conversation.member-join` events
    /// are log-and-skipped. This method therefore returns an empty
    /// `Vec` in the current data model — a Kupferdraht-current
    /// limitation tracked separately from 14c. The matrix-rust-sdk-
    /// shaped surface is in place so callers can write
    /// invitation-handling code that compiles unchanged once the
    /// underlying machinery lands.
    ///
    /// # Panics
    /// Panics if the in-memory rooms-cache `RwLock` is poisoned.
    #[must_use]
    pub fn invited_rooms(&self) -> Vec<Arc<Room>> {
        // Kupferdraht does not yet materialize pending invitations as
        // cached Room objects — see the rustdoc above. The empty
        // Vec keeps the matrix-rust-sdk-shaped surface stable so
        // caller-side invitation handling compiles today and works
        // unchanged once the materialization lands.
        Vec::new()
    }

    /// Step 14c — filter the rooms cache by an arbitrary predicate.
    /// matrix-rust-sdk uses a typed `RoomListFilter`; Kupferdraht takes
    /// a closure to keep the surface UDL-friendly.
    ///
    /// Replaces ad-hoc filtering through [`Self::rooms`] for
    /// callers who want, say, all groups (`r.room_type() ==
    /// RoomType::Group`) or all DMs with a particular counterpart.
    ///
    /// # Panics
    /// Panics if the in-memory rooms-cache `RwLock` is poisoned.
    #[must_use]
    pub fn rooms_filtered<F>(&self, predicate: F) -> Vec<Arc<Room>>
    where
        F: Fn(&Room) -> bool,
    {
        self.rooms
            .read()
            .expect("rooms lock poisoned")
            .iter()
            .filter(|r| predicate(r.as_ref()))
            .cloned()
            .collect()
    }

    /// Create a new MLS conversation on Wire and the matching
    /// local MLS group. Returns the freshly-cached `Arc<Room>`.
    /// Step 6a — zero-invitee path only; with-invitees support
    /// (which requires uploading an initial commit-bundle that
    /// adds the listed users) lands in cycle 6b.
    ///
    /// Pipeline:
    /// 1. `POST /v14/conversations` with `protocol="mls"` and
    ///    `qualified_users=[]` — Wire registers the conversation
    ///    envelope and assigns both a `qualified_id` and an MLS
    ///    `group_id`.
    /// 2. `MlsClient::create_group` builds the local MLS group at
    ///    epoch 0 with that `group_id`, the standard
    ///    `[Basic, X509]` leaf capabilities, and the `external_senders`
    ///    extension carrying Wire's backend removal key.
    /// 3. `sync_metadata` re-snapshots the rooms list so the new
    ///    room appears in `Client::rooms` and the persistent
    ///    `rooms` table.
    ///
    /// # Errors
    /// * `KupferdrahtError::InvalidInput` if the client has no MLS
    ///   state, the server response omits `group_id`, or the
    ///   newly-created room can't be located in the post-sync
    ///   cache.
    /// * `KupferdrahtError::Encoding` if `group_id` is not valid base64 or empty.
    /// * Network / backend / openmls failures from any sub-call.
    pub async fn create_room(&self, name: &str) -> Result<Arc<Room>, KupferdrahtError> {
        if name.is_empty() {
            return Err(KupferdrahtError::InvalidInput(
                "create_room: name must be non-empty".into(),
            ));
        }
        let mls = self.mls_client_required("create_room")?;
        let request = crate::wire::dto::CreateConversationRequest {
            name,
            protocol: "mls",
            group_conv_type: "group_conversation",
            qualified_users: Vec::new(),
        };
        let conv = self
            .http
            .create_conversation(self.api_version, &self.current_access_token(), &request)
            .await?;
        let group_id_b64 = conv.group_id.as_deref().ok_or_else(|| {
            KupferdrahtError::InvalidInput(
                "create_room: server response omitted group_id; check protocol=\"mls\"".into(),
            )
        })?;
        let group_id = base64::engine::general_purpose::STANDARD
            .decode(group_id_b64)
            .map_err(|e| KupferdrahtError::Encoding(format!("create_room group_id b64: {e}")))?;
        // Step 95c: backend data — an empty group id is a malformed
        // response, as in `create_dm` / `establish_self_room`.
        if group_id.is_empty() {
            return Err(KupferdrahtError::Encoding(
                "create_room: server response carries an empty group_id".into(),
            ));
        }
        // Build the local MLS group at epoch 0, then establish it
        // (epoch 0 → 1) with a single commit so the room is sendable
        // right away — Wire rejects application messages at epoch 0.
        {
            let _gate = self.mls_write_gate().await;
            let guard = crate::encryption::lock_mls(mls);
            guard.create_group(&group_id, &self.backend_removal_key_ecdsa_p256)?;
        }
        self.establish_group(mls, &group_id, "create_room").await?;
        // Re-sync to put the new conversation into the cache and the
        // persistent `rooms` table at its established (epoch 1) state.
        self.sync_metadata().await?;
        let room_id = conv.qualified_id.to_string();
        self.room(&room_id).await.ok_or_else(|| {
            KupferdrahtError::InvalidInput(format!(
                "create_room: {room_id} not in cache after sync (Wire metadata race?)"
            ))
        })
    }

    /// Advance a freshly-created group from epoch 0 to epoch 1 with one
    /// commit so application messages are accepted (Wire rejects them at
    /// epoch 0). Claims the creator's own remaining devices' KeyPackages
    /// (the current device is skipped via `skip_own` inside
    /// [`Self::claim_key_packages_for_users`]) and, if any exist, Adds
    /// them; with no other device to add, issues a self-update commit.
    /// Uploads the bundle to `/v14/mls/commit-bundles` and merges on
    /// success. On rejection the pending commit is cleared and the error
    /// surfaces — the room is left at epoch 0 (the prior inert state).
    /// `op` labels the calling operation in error messages
    /// (`"create_room"`, `"establish_self_room"` — Step 86 reuses this
    /// path for the never-established self conversation).
    pub(super) async fn establish_group(
        &self,
        mls: &Arc<StdMutex<crate::encryption::MlsClient>>,
        group_id: &[u8],
        op: &'static str,
    ) -> Result<(), KupferdrahtError> {
        debug_assert!(!group_id.is_empty(), "establish_group: group_id non-empty");
        let own_devices_kps = self.claim_key_packages_for_users(&[]).await?;
        self.stage_and_upload_commit(mls, group_id, op, |guard| {
            if own_devices_kps.is_empty() {
                guard.self_update_commit(group_id)
            } else {
                guard.add_members_commit(group_id, &own_devices_kps)
            }
        })
        .await
    }

    /// Create — and **establish** — the Wire MLS 1:1 with the named
    /// peer. Steps 14a + 68 — matrix-rust-sdk's `Client::create_dm`
    /// analog over Wire's first-class 1:1 endpoint.
    ///
    /// Wire models 1:1s as a dedicated `ConvType::DirectMessage`
    /// conversation rather than a "group with `is_direct` flag"
    /// (Matrix's model). The endpoint is idempotent on the server
    /// side: calling `create_dm` against a peer with whom a 1:1
    /// already exists returns the existing conversation rather than
    /// erroring out. There is at most one DM per (self, peer) pair.
    ///
    /// The fetch only reserves the conversation — live against
    /// wire-server (2026-08, observations §66) an MLS 1:1 stays
    /// hidden from `list-ids` until a member uploads the establishing
    /// commit. Hence the three-way branch after the fetch:
    ///
    /// 1. Local group present → resume (returning caller).
    /// 2. `epoch == 0` → establish: claim the peer's key packages,
    ///    Add-commit, upload ([`Self::establish_dm`]; failure
    ///    discards the local group so a retry starts clean).
    /// 3. `epoch > 0` → the peer (or another own device) established
    ///    first: cache the now-listed room, then join via external
    ///    commit ([`Self::join_room`]).
    ///
    /// On success the local rooms cache is refreshed via
    /// `sync_metadata` so the returned `Arc<Room>` is the same handle
    /// `Client::room(id)` and `Client::dm_room(peer)` will yield
    /// from then on.
    ///
    /// `peer` is the qualified `<uuid>@<domain>` string.
    ///
    /// # Errors
    /// * `KupferdrahtError::InvalidInput` if `peer` is malformed, the
    ///   client has no MLS state, or the conversation is missing from
    ///   the cache after the establish + sync round-trip.
    /// * `KupferdrahtError::BackendRejected` if the peer's key-package
    ///   pool is exhausted or Wire rejects the commit bundle.
    /// * Network / encoding / openmls failures from the underlying
    ///   fetch, claim, commit-upload, and `sync_metadata` calls.
    pub async fn create_dm(&self, peer: &str) -> Result<Arc<Room>, KupferdrahtError> {
        let peer_qid = QualifiedId::from(&UserId::parse(peer).map_err(|_| {
            KupferdrahtError::InvalidInput(format!(
                "create_dm: peer must be qualified '<uuid>@<domain>', got `{peer}`"
            ))
        })?);
        let mls = self.mls_client_required("create_dm")?;
        // Wire's 1:1 endpoint is a single idempotent GET — the server
        // creates the conversation on the first invocation against
        // a fresh `(self, peer)` pair, and returns the existing one
        // on subsequent invocations. No request body. The swagger
        // also lists a POST path; live Wire v14 backends do not
        // deploy it (recorded in `docs/wire/api-notes.md`).
        let conv = self
            .http
            .fetch_one_to_one_conversation(
                self.api_version,
                &self.current_access_token(),
                &peer_qid.domain,
                &peer_qid.id,
            )
            .await?;
        let group_id_b64 = conv.group_id.as_deref().ok_or_else(|| {
            KupferdrahtError::InvalidInput(
                "create_dm: server response omitted group_id; check protocol=\"mls\"".into(),
            )
        })?;
        let group_id = base64::engine::general_purpose::STANDARD
            .decode(group_id_b64)
            .map_err(|e| KupferdrahtError::Encoding(format!("create_dm group_id b64: {e}")))?;
        // Server data must never trip an assert (recoverable-error
        // rule) — an empty group_id is a backend defect, not ours.
        if group_id.is_empty() {
            return Err(KupferdrahtError::InvalidInput(
                "create_dm: server returned an empty group_id".into(),
            ));
        }
        let room_id = conv.qualified_id.to_string();
        let locally_present = {
            let guard = crate::encryption::lock_mls(mls);
            guard.group_exists(&group_id)?
        };
        if !locally_present {
            if conv.epoch == 0 {
                self.establish_dm(&group_id, peer).await?;
            } else {
                // Established elsewhere → the server already lists the
                // conversation; sync so `join_room` finds it cached.
                self.sync_metadata().await?;
                self.join_room(&room_id).await?;
            }
        }
        // Refresh the rooms cache — after the establishing commit the
        // server lists the conversation.
        self.sync_metadata().await?;
        self.room(&room_id).await.ok_or_else(|| {
            KupferdrahtError::InvalidInput(format!(
                "create_dm: {room_id} not listed after establish — \
                 server still hides the conversation"
            ))
        })
    }

    /// Epoch-0 bootstrap for [`Self::create_dm`]: create the local
    /// group under the conversation's `group_id`, then commit the
    /// peer in via [`Self::commit_dm_peer`]. Any failure after the
    /// local create **discards the group** (Step-66 pattern) — an
    /// unestablished leftover would short-circuit the next
    /// `create_dm` into the resume branch and dead-end the DM.
    async fn establish_dm(&self, group_id: &[u8], peer: &str) -> Result<(), KupferdrahtError> {
        assert!(!group_id.is_empty(), "establish_dm: empty group_id");
        let mls = self.mls_client_required("create_dm")?;
        {
            let _gate = self.mls_write_gate().await;
            let guard = crate::encryption::lock_mls(mls);
            guard.create_group(group_id, &self.backend_removal_key_ecdsa_p256)?;
        }
        let result = self.commit_dm_peer(group_id, peer).await;
        if result.is_err() {
            // Best-effort discard: the commit error is what the
            // caller must see, not a secondary storage failure.
            let _gate = self.mls_write_gate().await;
            if let Err(e) = crate::encryption::lock_mls(mls).discard_group(group_id) {
                tracing::warn!(error = %e, "create_dm: discard after failed establish failed");
            }
        }
        result
    }

    /// Claim + Add-commit + upload leg of [`Self::establish_dm`]:
    /// `claim_key_packages_for_users([peer])` (claims the peer's and
    /// own other devices' key packages; errors on an exhausted pool)
    /// → `add_members_commit` → the staged
    /// [`Self::upload_and_merge_commit_bundle`].
    async fn commit_dm_peer(&self, group_id: &[u8], peer: &str) -> Result<(), KupferdrahtError> {
        debug_assert!(peer.contains('@'), "commit_dm_peer: unqualified peer");
        let kp_bytes_list = self
            .claim_key_packages_for_users(&[peer.to_string()])
            .await?;
        let mls = self.mls_client_required("create_dm")?;
        self.stage_and_upload_commit(mls, group_id, "create_dm", |guard| {
            guard.add_members_commit(group_id, &kp_bytes_list)
        })
        .await
    }

    /// Local-cache lookup for the 1:1 conversation with the named
    /// peer. Step 14a — matrix-rust-sdk's `Client::get_dm_room`
    /// analog (renamed to drop the `get_` prefix per Rust API
    /// Guidelines C-GETTER and CHANGELOG.md §41's verb taxonomy).
    /// Returns `None` when no `RoomType::DirectMessage` room
    /// with a counterpart matching `peer` is in the cache.
    /// Strictly local; never hits the network.
    ///
    /// The lookup filters by `room_type == DirectMessage` AND
    /// `counterpart.user_id == peer` — a group chat that happens
    /// to contain the peer as a member is not returned.
    ///
    /// `peer` is the qualified `<uuid>@<domain>` string. Unqualified
    /// values produce `None` (no DM can have an `@`-less counterpart).
    ///
    /// # Panics
    /// Panics if the in-memory rooms-cache `RwLock` is poisoned.
    #[must_use]
    pub fn dm_room(&self, peer: &str) -> Option<Arc<Room>> {
        let rooms = self.rooms.read().expect("rooms lock poisoned");
        rooms
            .iter()
            .find(|r| {
                r.room_type() == RoomType::DirectMessage
                    && r.counterpart().is_some_and(|m| m.user_id == peer)
            })
            .cloned()
    }

    /// "Look up first, create if missing" convenience for 1:1
    /// conversations. Step 14a Wire-extension — matrix-rust-sdk
    /// has no analog because Matrix' DM model is `is_direct`-flagged
    /// room state, not a first-class server endpoint.
    ///
    /// Calls [`Self::dm_room`] first; on `Some`, returns it without
    /// any HTTP traffic. On `None`, falls through to
    /// [`Self::create_dm`] exactly once. Concurrent invocations
    /// against the same `peer` de-duplicate: the second caller
    /// observes the in-flight POST and receives the same `Arc<Room>`,
    /// guaranteeing at most one server-side 1:1 creation per
    /// `(self, peer)` pair across the process lifetime.
    ///
    /// `peer` follows the same qualified `<uuid>@<domain>` form as
    /// [`Self::create_dm`] and [`Self::dm_room`].
    ///
    /// # Errors
    /// Mirrors [`Self::create_dm`]'s error surface — any failure on
    /// the create-path bubbles up to all waiters.
    ///
    /// # Panics
    /// Panics if the per-peer mutex map's `std::sync::Mutex` or the
    /// in-memory rooms-cache `RwLock` is poisoned.
    pub async fn dm_with(&self, peer: &str) -> Result<Arc<Room>, KupferdrahtError> {
        // Fast path: already cached.
        if let Some(room) = self.dm_room(peer) {
            return Ok(room);
        }
        // Slow path: serialise creates per peer so two concurrent
        // callers do not POST twice. The per-peer mutex is held
        // across the `create_dm.await`; the second caller waits,
        // then observes the freshly-cached room via `dm_room`.
        let peer_lock = {
            let mut map = self.dm_create_lock.lock().expect("dm_create_lock poisoned");
            Arc::clone(
                map.entry(peer.to_string())
                    .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(()))),
            )
        };
        let _guard = peer_lock.lock().await;
        if let Some(room) = self.dm_room(peer) {
            return Ok(room);
        }
        self.create_dm(peer).await
    }
}
