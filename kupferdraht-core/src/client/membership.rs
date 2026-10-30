// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `Client` — MLS-membership lifecycle operations.
//!
//! The public membership surface (`invite_members`, `kick_members`,
//! `leave_room`, `join_room`, `join_all_rooms`, `resync_existing_rooms`,
//! `resync_room`, `sync_members`) plus the private commit-bundle /
//! key-package / member-resolution helpers that back it. Split out of
//! [`super`] in Step 29; the `Client` struct and the shared
//! accessors these methods call live in [`super`]. Public API surface
//! mirrors `matrix_sdk::Client` — see
//! `docs/api-mapping-matrix-rust-sdk.md`.

#![allow(clippy::doc_markdown)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};

// `MlsClient` is referenced only by the `[`MlsClient::…`]` intra-doc
// links in the moved docs below (the code reaches it via fully-qualified
// path), so the import is doc-only — kept to preserve link resolution,
// matching the in-scope state these docs had in `runtime.rs`.
#[allow(unused_imports)]
use crate::encryption::MlsClient;
use crate::error::KupferdrahtError;
use crate::identity::UserId;
use crate::rooms::{
    Room, RoomMember, RoomType, with_epoch as room_with_epoch, with_members as room_with_members,
};
use crate::storage::RoomMemberRecord;
use crate::wire::dto::{QualifiedId, UserProfile};

use super::{Client, JoinSummary, ResyncSummary};

impl Client {
    /// Invite one or more Wire users into the MLS group of an
    /// existing cached `Room`. Step 6b — the with-invitees flow that
    /// complements [`Self::create_room`]'s zero-invitee bootstrap.
    ///
    /// Pipeline:
    /// 1. Resolve the room's `group_id` from the cache.
    /// 2. For each invitee, claim `KeyPackage`s via
    ///    `POST /v14/mls/key-packages/claim/{domain}/{uuid}` — Wire
    ///    returns one entry per device the user has registered.
    /// 3. Build an Add-commit through
    ///    [`crate::encryption::MlsClient::add_members_commit`] —
    ///    locally staged but **not** merged.
    /// 4. Upload the bundle via `POST /v14/mls/commit-bundles`.
    /// 5. On success: merge the pending commit so the local epoch
    ///    matches the server. On failure: clear the pending commit
    ///    and surface the upload error.
    /// 6. Re-sync metadata to pick up the new epoch and member list.
    /// 7. Step 14e: return the freshly-cached `Arc<Room>` (now
    ///    carrying the new epoch + member list). Same handle
    ///    `Client::room(room_id)` yields — verified by `Arc::ptr_eq`.
    ///
    /// `user_ids` are qualified `<uuid>@<domain>` strings — the same
    /// shape `Room::members().user_id` exposes.
    ///
    /// **Invite from a fresh fold.** After the commit merges, the
    /// re-broadcast (ADR-0018 / ADR-0020 MUST) hands the invitees
    /// this client's *current* fold of case, marker, and tasks —
    /// MLS has no history, so state this client has not yet drained
    /// is permanently missing for the joiner. Long-running consumers
    /// on a push loop are always fresh; a one-shot process must
    /// `sync_once` before inviting (the CLI does).
    ///
    /// # Errors
    /// * `KupferdrahtError::InvalidInput` if `user_ids` is empty, an
    ///   entry is malformed, the room is not in the cache, the room
    ///   has no `group_id`, the client has no MLS state, or the
    ///   refreshed cache no longer contains the room (defensive).
    /// * `KupferdrahtError::BackendRejected` if Wire's claim returns an
    ///   empty pool or `/mls/commit-bundles` rejects the bundle.
    /// * `KupferdrahtError::Encoding` for malformed claim payloads.
    /// * Network / openmls failures from any sub-call.
    pub async fn invite_members(
        &self,
        room_id: &str,
        user_ids: &[String],
    ) -> Result<Arc<Room>, KupferdrahtError> {
        if user_ids.is_empty() {
            return Err(KupferdrahtError::InvalidInput(
                "invite_members: at least one user_id is required".into(),
            ));
        }
        // Step 95c: caller input — each id must be `<uuid>@<domain>`
        // before it is split into the key-package claim path.
        for user_id in user_ids {
            crate::identity::UserId::parse(user_id)?;
        }
        let group_id = self
            .resolve_room_group_id(room_id, "invite_members")
            .await?;
        let mls = self.mls_client_required("invite_members")?;
        let kp_bytes_list = self.claim_key_packages_for_users(user_ids).await?;
        self.stage_and_upload_commit(mls, &group_id, "invite_members", |guard| {
            guard.add_members_commit(&group_id, &kp_bytes_list)
        })
        .await?;
        self.note_room_devices(room_id).await;
        // Step 72c + Step 75 (ADR-0018 / ADR-0020 MUST): re-broadcast
        // the room's current state AFTER the Add-commit merged — the
        // invitees decrypt only post-commit epochs, so these are the
        // messages that close the MLS no-history late-join gap.
        self.rebroadcast_room_state_after_invite(room_id).await?;
        self.sync_metadata().await?;
        self.room(room_id).await.ok_or_else(|| {
            KupferdrahtError::InvalidInput(format!(
                "invite_members: {room_id} missing from cache after refresh"
            ))
        })
    }

    /// The invite re-broadcast (72c MUST + Step 75 union, ADR-0020
    /// decision 2): re-send whatever anchors the room currently has —
    /// case snapshot (`Room::case()` `Some`) and/or task-list marker
    /// (`Room::is_task_list()`) — plus, when EITHER anchor is
    /// present, every task's current folded state ONCE (a room that
    /// is both a case and a marked task list sends case, marker,
    /// and N tasks — never 2N). Re-broadcasts repeat state, never
    /// update it. Best-effort throughout: membership succeeded; a
    /// failed re-send warns and self-heals on later room activity.
    /// Unanchored rooms send nothing (ADR-0014 empty-on-join).
    async fn rebroadcast_room_state_after_invite(
        &self,
        room_id: &str,
    ) -> Result<(), KupferdrahtError> {
        debug_assert!(
            !room_id.is_empty(),
            "rebroadcast_room_state_after_invite: room_id must be non-empty"
        );
        let case = self.folded_case(room_id).await?;
        let marked = self.folded_task_list_marker(room_id).await?;
        if let Some(snapshot) = &case
            && let Err(e) = self.send_case_snapshot(room_id, snapshot).await
        {
            tracing::warn!(room_id, error = %e, "invite: case snapshot re-broadcast failed");
        }
        if marked && let Err(e) = self.send_task_list_marker_state(room_id, true).await {
            tracing::warn!(room_id, error = %e, "invite: task-list marker re-broadcast failed");
        }
        if case.is_some() || marked {
            self.resend_folded_task_states(room_id).await?;
        }
        Ok(())
    }

    /// Remove every leaf belonging to one of `user_ids` from the
    /// MLS group backing `room_id`. Step 6d.
    ///
    /// Flow mirrors [`Self::invite_members`] in the inverse
    /// direction:
    ///
    /// 1. Locate the room + its `group_id` in the cache.
    /// 2. Build a Remove-commit through
    ///    [`crate::encryption::MlsClient::remove_members_commit`] —
    ///    every leaf whose credential identity decodes to a
    ///    `user_id` in the list is dropped in one commit.
    /// 3. Upload via `POST /v14/mls/commit-bundles`.
    /// 4. On success: merge. On failure: clear pending and surface
    ///    the rejection.
    /// 5. Re-sync metadata to pick up the new epoch and member list.
    /// 6. Step 14e: return the freshly-cached `Arc<Room>` (now
    ///    minus the kicked leaves).
    ///
    /// Wire-side, the Remove emits `conversation.mls-message-add`
    /// events to the kicked devices' notification streams plus a
    /// `member-leave` to the rest of the room — consumers handle
    /// both via their existing event handlers.
    ///
    /// **Self-kick is intentionally not supported** in Step 6d.
    /// Removing your own last device from a room collapses your
    /// own MLS state in a way that needs the proper logout /
    /// device-revocation lifecycle (a future Step's scope).
    ///
    /// # Errors
    /// * `KupferdrahtError::InvalidInput` if `user_ids` is empty, the
    ///   caller is in the list, the room is not cached / has no
    ///   `group_id`, no leaf matches any listed user, or the
    ///   refreshed cache no longer contains the room (defensive).
    /// * `KupferdrahtError::BackendRejected` if Wire rejects the bundle.
    /// * Network / openmls failures from any sub-call.
    pub async fn kick_members(
        &self,
        room_id: &str,
        user_ids: &[String],
    ) -> Result<Arc<Room>, KupferdrahtError> {
        if user_ids.is_empty() {
            return Err(KupferdrahtError::InvalidInput(
                "kick_members: at least one user_id is required".into(),
            ));
        }
        let group_id = self.resolve_room_group_id(room_id, "kick_members").await?;
        let mls = self.mls_client_required("kick_members")?;
        self.stage_and_upload_commit(mls, &group_id, "kick_members", |guard| {
            guard.remove_members_commit(&group_id, user_ids)
        })
        .await?;
        self.note_room_devices(room_id).await;
        self.sync_metadata().await?;
        self.room(room_id).await.ok_or_else(|| {
            KupferdrahtError::InvalidInput(format!(
                "kick_members: {room_id} missing from cache after refresh"
            ))
        })
    }

    /// Stage an own commit with `stage`, upload it, merge on success or
    /// clear on rejection — the whole flow under the group's commit-flow
    /// lock, so this process's drain waits for the merge instead of
    /// healing around the next epoch (Step 97d, ADR-0031).
    pub(crate) async fn stage_and_upload_commit(
        &self,
        mls: &Arc<StdMutex<crate::encryption::MlsClient>>,
        group_id: &[u8],
        op: &'static str,
        stage: impl FnOnce(&crate::encryption::MlsClient) -> Result<Vec<u8>, KupferdrahtError> + Send,
    ) -> Result<(), KupferdrahtError> {
        let _flow = self.lock_commit_flow(group_id).await;
        let bundle = {
            let _gate = self.mls_write_gate().await;
            stage(&crate::encryption::lock_mls(mls))?
        };
        self.upload_and_merge_commit_bundle(mls, group_id, &bundle, op)
            .await
    }

    /// Upload a commit bundle to `/v14/mls/commit-bundles`. On
    /// success, merge the pending commit so the local epoch advances
    /// in lockstep with the server. On failure, clear the pending
    /// commit so a follow-up retry rebuilds against the unchanged
    /// epoch. Used by `invite_members` and `kick_members`.
    pub(crate) async fn upload_and_merge_commit_bundle(
        &self,
        mls: &Arc<StdMutex<crate::encryption::MlsClient>>,
        group_id: &[u8],
        bundle: &[u8],
        op: &'static str,
    ) -> Result<(), KupferdrahtError> {
        debug_assert!(
            self.commit_flow_in_flight(group_id),
            "an own commit is uploaded under lock_commit_flow (ADR-0031)"
        );
        match self
            .http
            .upload_commit_bundle(self.api_version, &self.current_access_token(), bundle)
            .await
        {
            Ok(()) => {
                tracing::debug!(op, "commit bundle accepted; merging the pending commit");
                let _gate = self.mls_write_gate().await;
                crate::encryption::lock_mls(mls).merge_pending_commit(group_id)
            }
            Err(upload_err) => {
                tracing::debug!(op, "commit bundle rejected; clearing the pending commit");
                let _gate = self.mls_write_gate().await;
                let _ = crate::encryption::lock_mls(mls).clear_pending_commit(group_id);
                Err(upload_err)
            }
        }
    }

    /// Leave the MLS group for `room_id`. Step 6e-1; matrix-rust-sdk
    /// equivalent: `Room::leave`.
    ///
    /// **Wire's MLS leave flow is server-driven** (observations.md §31,
    /// live-validated against Wire's backend). The leaving client
    /// does NOT build a self-Remove commit — openmls explicitly
    /// rejects that with `CreateCommitError::CannotRemoveSelf`. The
    /// observed flow on the wire: client calls
    /// `DELETE /v14/conversations/{d}/{id}/members/{self_d}/{self_uuid}`,
    /// Wire's external-sender produces the MLS Remove commit on the
    /// leaver's behalf, the remaining members receive that commit via
    /// their `/v14/notifications` stream.
    ///
    /// Pipeline:
    /// 1. Locate the room + its `group_id` in the local cache so we
    ///    know which openmls group to discard locally on success.
    /// 2. `DELETE /v14/conversations/{d}/{id}/members/{self_d}/{self_uuid}`.
    /// 3. On success: discard the persisted MLS group state via
    ///    `MlsClient::discard_group` (the local mirror of "no longer
    ///    a member"). Wire's commit will arrive on the next sync but
    ///    we've already cleared the local state.
    /// 4. Re-sync metadata so the in-memory rooms cache + the
    ///    persisted `rooms` table both drop the now-left room.
    ///
    /// # Errors
    /// * `KupferdrahtError::InvalidInput` if the room is not cached.
    /// * `KupferdrahtError::BackendRejected` if Wire rejects the request
    ///   (e.g. `403 invalid-op`, `404 no-conversation`).
    /// * Network / openmls / storage failures from any sub-call.
    pub async fn leave_room(&self, room_id: &str) -> Result<(), KupferdrahtError> {
        let room = self.room(room_id).await.ok_or_else(|| {
            KupferdrahtError::InvalidInput(format!("leave_room: {room_id} not in cache"))
        })?;
        let group_id = room.group_id_bytes();
        let self_qualified = self.user_id.as_str().to_string();
        self.http
            .delete_conversation_member(
                self.api_version,
                &self.current_access_token(),
                room_id,
                &self_qualified,
            )
            .await?;
        if let (Some(gid), Some(mls)) = (group_id, self.mls_client.as_ref()) {
            // Best-effort local cleanup: discard the persisted MLS
            // group. If it isn't in storage (we never joined / already
            // discarded), tolerate the missing-state error rather than
            // failing the whole leave.
            let _gate = self.mls_write_gate().await;
            let _ = crate::encryption::lock_mls(mls).discard_group(&gid);
        }
        self.sync_metadata().await?;
        Ok(())
    }

    /// Helper for [`Self::invite_members`]: claim
    ///
    /// 1. the caller's own `KeyPackage`s (`skip_own` = current device)
    ///    so every other Wire client this user owns can be added in
    ///    the same commit, and
    /// 2. one batch of `KeyPackage`s per invitee.
    ///
    /// Then flatten into a single `Vec<Vec<u8>>` ready for
    /// `MlsClient::add_members_commit`.
    ///
    /// Why the self-claim is mandatory: Wire enforces that every
    /// device of every member-user has a corresponding leaf. After
    /// `create_room` the local group has only this device's leaf,
    /// while Wire's user record may list further clients (older
    /// Kupferdraht logins, Wire iOS / desktop apps). Without seeding
    /// those into the same commit, Wire rejects with
    /// `409 mls-client-mismatch: "A proposal of type Add or Remove
    /// does not apply to the full list of clients for a user"`
    /// (live-confirmed). The duplicate-leaf filter inside
    /// `MlsClient::add_members_commit` keeps subsequent invites
    /// idempotent: once the user's other devices are leaves they are
    /// silently dropped from any later self-claim batch.
    pub(crate) async fn claim_key_packages_for_users(
        &self,
        user_ids: &[String],
    ) -> Result<Vec<Vec<u8>>, KupferdrahtError> {
        let ciphersuite_decimal = u16::from(crate::encryption::MLS_CIPHERSUITE_FOR_CLAIM);
        let self_domain = self.account.domain();
        let mut out: Vec<Vec<u8>> = Vec::new();
        // Self-bootstrap: every Wire user-record may list further
        // clients (older Kupferdraht logins, iOS / desktop). Wire rejects
        // an Add commit unless every member-user's full client set
        // has a matching leaf — see observations.md §27.
        self.append_claimed_kps_for(
            &mut out,
            self_domain,
            self.user_id.localpart(),
            ciphersuite_decimal,
            Some(&self.device_id),
        )
        .await?;
        // Invitees. Wire's claim returns the user's full pool;
        // duplicate-leaf filter inside `add_members_commit` keeps
        // subsequent invites idempotent.
        for user_id in user_ids {
            let (uuid, domain) = user_id.split_once('@').ok_or_else(|| {
                KupferdrahtError::InvalidInput(format!(
                    "invite_members: malformed user_id (expected <uuid>@<domain>): {user_id}"
                ))
            })?;
            let len_before = out.len();
            self.append_claimed_kps_for(&mut out, domain, uuid, ciphersuite_decimal, None)
                .await?;
            if out.len() == len_before {
                return Err(KupferdrahtError::BackendRejected(format!(
                    "invite_members: Wire returned no KeyPackages for {user_id} \
                     (pool exhausted or user has no devices?)"
                )));
            }
        }
        Ok(out)
    }

    async fn append_claimed_kps_for(
        &self,
        out: &mut Vec<Vec<u8>>,
        domain: &str,
        uuid: &str,
        ciphersuite_decimal: u16,
        skip_own_client: Option<&str>,
    ) -> Result<(), KupferdrahtError> {
        let claim = self
            .http
            .claim_key_packages(
                self.api_version,
                &self.current_access_token(),
                domain,
                uuid,
                ciphersuite_decimal,
                skip_own_client,
            )
            .await?;
        for kp in claim.key_packages {
            out.push(kp.key_package_bytes()?);
        }
        Ok(())
    }

    /// Iterate every cached room that Kupferdraht is not yet locally a
    /// member of and run `Room::join` against each. Collects a
    /// [`JoinSummary`] with per-room success / failure info.
    ///
    /// A room is considered "already joined" when the local MLS group
    /// keyed by its `group_id` is present in openmls's storage. This
    /// is the canonical source — Wire's conversation metadata cannot
    /// distinguish "I am a member" from "I have been added but never
    /// rehydrated locally", which is exactly the state a fresh
    /// Kupferdraht install boots into (LEARNINGS §2.1).
    ///
    /// Errors from a single room's join are captured in
    /// `JoinSummary.failed` rather than bubbled up — the outer call
    /// only fails on infrastructural problems (missing MLS state).
    ///
    /// # Errors
    /// * `KupferdrahtError::InvalidInput` if the client has no MLS state.
    pub async fn join_all_rooms(&self) -> Result<JoinSummary, KupferdrahtError> {
        let mls = self.mls_client.as_ref().ok_or_else(|| {
            KupferdrahtError::InvalidInput(
                "join_all_rooms: Client has no MLS state; base_dir must be set".into(),
            )
        })?;
        let cached = self.rooms().await;
        let mut summary = JoinSummary::default();
        for room in cached {
            summary.attempted += 1;
            let Some(group_id) = room.group_id_bytes() else {
                summary
                    .failed
                    .push((room.id(), "missing group_id (run sync_metadata)".into()));
                continue;
            };
            // Cheap local probe — if openmls storage already knows
            // the group, we've joined before (in this process or a
            // previous one backed by the same mls.db).
            let already = {
                let guard = crate::encryption::lock_mls(mls);
                guard.group_exists(&group_id)?
            };
            if already {
                summary.already_joined += 1;
                continue;
            }
            match self.join_room(&room.id()).await {
                Ok(()) => summary.joined += 1,
                Err(e) => summary.failed.push((room.id(), e.to_string())),
            }
        }
        Ok(summary)
    }

    /// Walk every cached room and re-join any whose persisted MLS
    /// leaf belongs to a prior `device_id` of the same `user_id` —
    /// the failure mode that surfaced after an interactive
    /// re-`login` (CHANGELOG.md §29). Step 7e.
    ///
    /// Per-room flow:
    ///
    /// 1. Skip rooms with no `group_id` (un-`sync_metadata`-d) and
    ///    rooms whose `group_id` is unknown to the local openmls
    ///    storage (never joined by *any* device — out of scope for
    ///    resync; covered by [`Self::join_all_rooms`]).
    /// 2. Inspect the persisted group's `own_leaf` credential via
    ///    [`MlsClient::group_belongs_to_self`]. When the credential
    ///    identity matches the current device, count as
    ///    `already_current` and move on.
    /// 3. Otherwise: drop the stale group state via
    ///    [`MlsClient::discard_group`] and re-join via the unchanged
    ///    Step-3 external-commit path ([`Self::join_room`]). Wire's
    ///    `external_senders` invariants hold across discard/rejoin
    ///    because the new external commit re-fetches a live
    ///    `GroupInfo` each time.
    ///
    /// Per-room failures land in [`ResyncSummary::failed`] rather
    /// than aborting — one stuck room must not block recovery of
    /// the rest.
    ///
    /// # Errors
    /// * `KupferdrahtError::InvalidInput` if the client has no MLS state.
    pub async fn resync_existing_rooms(&self) -> Result<ResyncSummary, KupferdrahtError> {
        let mls = self.mls_client.as_ref().ok_or_else(|| {
            KupferdrahtError::InvalidInput(
                "resync_existing_rooms: Client has no MLS state; base_dir must be set".into(),
            )
        })?;
        let cached = self.rooms().await;
        let mut summary = ResyncSummary::default();
        for room in cached {
            summary.attempted += 1;
            let Some(group_id) = room.group_id_bytes() else {
                continue;
            };
            let (exists, belongs_to_self) = {
                let guard = crate::encryption::lock_mls(mls);
                if guard.group_exists(&group_id)? {
                    (true, guard.group_belongs_to_self(&group_id)?)
                } else {
                    (false, false)
                }
            };
            if !exists {
                continue;
            }
            if belongs_to_self {
                summary.already_current += 1;
                continue;
            }
            match self.resync_room(&room.id()).await {
                Ok(()) => summary.resynced += 1,
                Err(e) => summary.failed.push((room.id(), e.to_string())),
            }
        }
        Ok(summary)
    }

    /// Drop the stale local MLS state for `room_id` and re-join via
    /// external commit. Step 7e — the per-room recovery operation
    /// driven by [`Self::resync_existing_rooms`].
    ///
    /// # Errors
    /// * `KupferdrahtError::InvalidInput` if `room_id` is not in the
    ///   cache, has no `group_id`, or no local MLS group exists.
    /// * Network / openmls / Wire-rejection errors from
    ///   [`Self::join_room`].
    pub async fn resync_room(&self, room_id: &str) -> Result<(), KupferdrahtError> {
        let room = self.room(room_id).await.ok_or_else(|| {
            KupferdrahtError::InvalidInput(format!("resync_room: {room_id} not in cache"))
        })?;
        let group_id = room.group_id_bytes().ok_or_else(|| {
            KupferdrahtError::InvalidInput(format!(
                "resync_room: {room_id} has no group_id — run `kupferdraht-cli sync` first"
            ))
        })?;
        let mls = self.mls_client_required("resync_room")?;
        {
            let _gate = self.mls_write_gate().await;
            let guard = crate::encryption::lock_mls(mls);
            // A missing local group is exactly the state recovery exists
            // for (Step 64: an mls-message-add that failed because the
            // group is gone) — nothing to discard must not abort the
            // rejoin; the join either restores the state or fails loudly.
            if let Err(e) = guard.discard_group(&group_id) {
                tracing::debug!(error = %e, "resync_room: nothing to discard; rejoining");
            }
        }
        self.join_room(room_id).await
    }

    /// Join an existing MLS group — via external commit (RFC 9420
    /// §12.5) for every room Wire has a `GroupInfo` for, or, for the
    /// **never-established self conversation** (`RoomType::SelfRoom`
    /// at `epoch == 0`), by establishing it ourselves (Step 86,
    /// [`Self::establish_self_room`]): Wire serves no `GroupInfo`
    /// before a member's first commit (`404 mls-missing-group-info`,
    /// live-confirmed), so an external join is impossible there. A
    /// self room another own device already established (`epoch > 0`)
    /// takes the external-commit path like any other room.
    ///
    /// External-commit flow:
    ///
    /// 1. Fetch the group's current `GroupInfo` via
    ///    `GET /v14/conversations/{d}/{id}/groupinfo`.
    /// 2. Build the commit bundle through [`MlsClient::join_by_external_commit`]
    ///    — the local MLS state advances to `server_epoch + 1` the
    ///    moment the builder finalizes (openmls has no staged external
    ///    join — `open.md` gap-B.22).
    /// 3. Upload the bundle via `POST /v14/mls/commit-bundles`. Wire
    ///    advances every other member to the same epoch. On a non-2xx
    ///    response the just-merged local group is **discarded** (Step
    ///    66) so a later `join_all_rooms` retries instead of skipping
    ///    a group that is one epoch ahead of the server; the error
    ///    surfaces as `KupferdrahtError::BackendRejected`.
    /// 4. Update the in-memory and persisted room snapshots with the
    ///    epoch reported by the joined openmls group (Step 66 — not
    ///    cache+1; the cache may be stale).
    ///
    /// # Errors
    /// * `KupferdrahtError::InvalidInput` if `room_id` is not in the cache,
    ///   is malformed, or the client has no MLS state.
    /// * `KupferdrahtError::Network` / `BackendRejected` / `Encoding` from
    ///   either HTTP call.
    /// * `KupferdrahtError::Generic` / `Encoding` from the openmls
    ///   external-commit pipeline.
    pub async fn join_room(&self, room_id: &str) -> Result<(), KupferdrahtError> {
        let room = self.room(room_id).await.ok_or_else(|| {
            KupferdrahtError::InvalidInput(format!("join_room: {room_id} not in cache"))
        })?;
        let joined = if room.room_type() == RoomType::SelfRoom && room.epoch() == 0 {
            self.establish_self_room(&room).await
        } else {
            self.join_room_by_external_commit(&room).await
        };
        // Step 95d: a (re)join replaces the tree — diff it against the
        // persisted device set (a forced rejoin may bring new devices).
        if joined.is_ok() {
            self.note_room_devices(room_id).await;
        }
        joined
    }

    /// The external-commit leg of [`Self::join_room`] (steps 1–4 of
    /// its flow) for a cached `room`.
    async fn join_room_by_external_commit(&self, room: &Room) -> Result<(), KupferdrahtError> {
        let room_id = room.id();
        let room_id = room_id.as_str();
        let (room_uuid, room_domain) = room_id.split_once('@').ok_or_else(|| {
            KupferdrahtError::InvalidInput(format!("join_room: malformed room_id: {room_id}"))
        })?;
        let mls = self.mls_client.as_ref().ok_or_else(|| {
            KupferdrahtError::InvalidInput(
                "join_room: Client has no MLS state; set ClientBuilder::base_dir and \
                 re-login"
                    .into(),
            )
        })?;
        let gi_bytes = self
            .http
            .fetch_group_info(
                self.api_version,
                &self.current_access_token(),
                room_uuid,
                room_domain,
            )
            .await?;
        // Resolve the group id BEFORE the builder finalizes (review
        // re-check): failing here after the merge would leave a group
        // behind with no way to discard it — the exact orphan Step 66
        // exists to prevent.
        let group_id = room.group_id_bytes().ok_or_else(|| {
            KupferdrahtError::InvalidInput(format!(
                "join_room: {room_id} has no group_id — run sync first"
            ))
        })?;
        let bundle = {
            let _gate = self.mls_write_gate().await;
            let guard = crate::encryption::lock_mls(mls);
            guard.join_by_external_commit(&gi_bytes)?
        };
        if let Err(upload_err) = self
            .http
            .upload_commit_bundle(self.api_version, &self.current_access_token(), &bundle)
            .await
        {
            // Step 66 / Option A (open.md gap-B.22): openmls merged the
            // joined group at `finalize` — discard it so `group_exists`
            // is false again and `join_all_rooms` retries normally.
            // Without this the client sits one epoch ahead of the
            // server behind a group that masquerades as joined.
            // Best-effort: the upload error is what the caller must see.
            let _gate = self.mls_write_gate().await;
            if let Err(e) = crate::encryption::lock_mls(mls).discard_group(&group_id) {
                tracing::warn!(error = %e, "join_room: discard after failed upload failed");
            }
            return Err(upload_err);
        }
        // Step 66 (review S16): the authoritative post-join epoch comes
        // from the openmls group, not the possibly stale rooms cache.
        let new_epoch = {
            let guard = crate::encryption::lock_mls(mls);
            guard.group_epoch(&group_id)?
        };
        self.bump_room_epoch(room_id, new_epoch).await
    }

    /// Re-derive a room's member list from the local MLS group state
    /// and resolve display-name / handle via Wire's user directory.
    /// See [`Room::sync_members`] for caller-facing docs.
    ///
    /// Step 14b — renamed from `refresh_members` to retire the last
    /// `refresh_*` verb from the public API (finding §41 verb
    /// taxonomy). matrix-rust-sdk uses this exact name.
    ///
    /// # Errors
    /// * `KupferdrahtError::InvalidInput` if `room_id` is not cached, the
    ///   room has no persisted MLS group (`join_room` has not been
    ///   called), or the `Client` was built without MLS state.
    /// * `KupferdrahtError::Encoding` if a leaf credential is malformed.
    /// * Network / backend errors from `POST /v14/list-users`.
    /// * `KupferdrahtError::Storage` from the per-room persistence update.
    pub async fn sync_members(&self, room_id: &str) -> Result<Vec<RoomMember>, KupferdrahtError> {
        let room = self.room(room_id).await.ok_or_else(|| {
            KupferdrahtError::InvalidInput(format!("sync_members: {room_id} not in cache"))
        })?;
        let group_id = room.group_id_bytes().ok_or_else(|| {
            KupferdrahtError::InvalidInput(format!(
                "sync_members: {room_id} has no group_id — \
                 run `kupferdraht-cli sync` after join"
            ))
        })?;
        let mls = self.mls_client.as_ref().ok_or_else(|| {
            KupferdrahtError::InvalidInput(
                "sync_members: Client has no MLS state; base_dir must be set".into(),
            )
        })?;
        let (leaves, self_qualified_id) = {
            let guard = crate::encryption::lock_mls(mls);
            (guard.group_leaves(&group_id)?, guard.own_wire_user_id()?)
        };
        // Step 72a: the leaf rebuild must MERGE the conversation-level
        // roles from the cached metadata snapshot — leaves carry no
        // role, and resetting every member to `Member` would silently
        // strip admins (starving ADR-0018's case-fold gate).
        let prior_roles: HashMap<String, crate::rooms::RoomMemberRole> = room
            .members()
            .await?
            .into_iter()
            .map(|m| (m.user_id, m.role))
            .collect();
        let members = self
            .resolve_members(&leaves, &self_qualified_id, &prior_roles)
            .await?;
        self.persist_room_members(room_id, &members).await?;
        self.swap_room_members(room_id, members.clone());
        Ok(members)
    }

    /// Turn leaf identities into `RoomMember`s, tagging the self leaf
    /// and backfilling non-self display info via `POST /v14/list-users`.
    /// `self_user_id` is the qualified (`{uuid}@{domain}`) id of this
    /// device's own leaf, supplied by the caller who already locked
    /// `MlsClient`.
    async fn resolve_members(
        &self,
        leaves: &[crate::encryption::GroupLeaf],
        self_user_id: &str,
        prior_roles: &HashMap<String, crate::rooms::RoomMemberRole>,
    ) -> Result<Vec<RoomMember>, KupferdrahtError> {
        let non_self_qids: Vec<QualifiedId> = leaves
            .iter()
            .filter(|l| l.user_id != self_user_id)
            .filter_map(|l| {
                UserId::parse(&l.user_id)
                    .ok()
                    .map(|u| QualifiedId::from(&u))
            })
            .collect();
        let lookup: HashMap<String, UserProfile> = if non_self_qids.is_empty() {
            HashMap::new()
        } else {
            let resp = self
                .http
                .list_users(
                    self.api_version,
                    &self.current_access_token(),
                    &non_self_qids,
                )
                .await?;
            resp.found
                .into_iter()
                .map(|u| (u.qualified_id.to_string(), u))
                .collect()
        };
        let self_name = self.account.display_name().to_string();
        let self_handle = self.account.handle().map(str::to_string);
        Ok(leaves
            .iter()
            .map(|leaf| {
                let is_self = leaf.user_id == self_user_id;
                let (display_name, handle): (Option<String>, Option<String>) = if is_self {
                    (Some(self_name.clone()), self_handle.clone())
                } else if let Some(p) = lookup.get(&leaf.user_id) {
                    (Some(p.name.clone()), p.handle.clone())
                } else {
                    (None, None)
                };
                RoomMember {
                    user_id: leaf.user_id.clone(),
                    is_self,
                    display_name,
                    handle,
                    // Merged from the metadata snapshot; a leaf whose
                    // user the metadata does not know (yet) is a
                    // member until the next sync says otherwise.
                    role: prior_roles
                        .get(&leaf.user_id)
                        .copied()
                        .unwrap_or(crate::rooms::RoomMemberRole::Member),
                }
            })
            .collect())
    }

    /// Replace the `members` snapshot on the cached `Arc<Room>` for
    /// `room_id`. No-op when the room is not (or no longer) cached.
    fn swap_room_members(&self, room_id: &str, members: Vec<RoomMember>) {
        let mut rooms = self.rooms.write().expect("rooms lock poisoned");
        for slot in rooms.iter_mut() {
            if slot.id() == room_id {
                *slot = Arc::new(room_with_members((**slot).clone(), members));
                return;
            }
        }
    }

    /// Mirror a `sync_members` member list into the attached
    /// `SqliteStore`. No-op when no store is attached.
    async fn persist_room_members(
        &self,
        room_id: &str,
        members: &[RoomMember],
    ) -> Result<(), KupferdrahtError> {
        let Some(store) = self.store.as_ref() else {
            return Ok(());
        };
        let records: Vec<RoomMemberRecord> = members
            .iter()
            .map(|m| RoomMemberRecord {
                room_id: room_id.to_string(),
                user_id: m.user_id.clone(),
                is_self: m.is_self,
                display_name: m.display_name.clone(),
                handle: m.handle.clone(),
                role: m.role.as_db_str().to_string(),
            })
            .collect();
        store.replace_room_members(room_id, &records).await
    }

    /// Swap an `Arc<Room>` in the cache with a clone whose epoch is
    /// bumped to `new_epoch`. Mirrors the same bump through the
    /// attached `SqliteStore` when present so a process restart picks
    /// up the post-join epoch.
    pub(crate) async fn bump_room_epoch(
        &self,
        room_id: &str,
        new_epoch: u64,
    ) -> Result<(), KupferdrahtError> {
        {
            let mut rooms = self.rooms.write().expect("rooms lock poisoned");
            for slot in rooms.iter_mut() {
                if slot.id() == room_id {
                    let updated = room_with_epoch((**slot).clone(), new_epoch);
                    *slot = Arc::new(updated);
                    break;
                }
            }
        }
        if let Some(store) = self.store.as_ref() {
            let epoch = i64::try_from(new_epoch).unwrap_or(i64::MAX);
            store.update_room_epoch(room_id, epoch).await?;
        }
        Ok(())
    }
}
