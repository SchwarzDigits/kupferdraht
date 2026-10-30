// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `Client` — the self conversation (Step 86).
//!
//! Wire's per-user notes-to-self room is an ordinary MLS group whose
//! only members are the user's own devices. It carries the private
//! cross-device channel later features ride (personal task list,
//! `LastRead` read position, draft sync). Wire lists it in the rooms
//! snapshot from day one (`RoomType::SelfRoom`, `epoch: 0`, a
//! `group_id`) but serves **no `GroupInfo`** until a member has
//! established the group — live-confirmed against a self-hosted instance,
//! `GET …/groupinfo` answers `404 mls-missing-group-info`
//! (observations §82). An external commit is therefore impossible at
//! epoch 0: the first device establishes the group itself, every
//! later device joins by external commit like any other room.
//! `Client::join_room` branches on exactly that (`membership.rs`).

use std::sync::Arc;

use tracing::{info, warn};

use crate::error::KupferdrahtError;
use crate::rooms::{Room, RoomType};

use super::Client;

impl Client {
    /// Local-cache lookup of the caller's self conversation
    /// (`RoomType::SelfRoom`). Strictly local; never hits the network.
    /// `None` before the first `sync_once` / `load_rooms_from_store`
    /// or when the snapshot carries no self room. Wire has exactly one
    /// per user; the room may still be at epoch 0 (not yet
    /// established) — `join_room` / `join_all_rooms` bring it online.
    /// matrix-rust-sdk has no analog (Matrix has no self room).
    #[must_use]
    pub fn self_room(&self) -> Option<Arc<Room>> {
        self.rooms_filtered(|r| r.room_type() == RoomType::SelfRoom)
            .into_iter()
            .next()
    }

    /// Establish the never-established self conversation (`epoch == 0`
    /// in the snapshot): build the local group under the snapshot's
    /// `group_id`, then run the Step-31 establishing commit
    /// ([`Self::establish_group`] — own other devices Added, their
    /// Welcome riding the bundle for Step 69 to absorb, or a
    /// self-update when this is the only device) and record the
    /// resulting epoch on the cached + persisted room.
    ///
    /// Any failure after the local create **discards the group**
    /// (Step-66 pattern): a leftover epoch-0 group would make
    /// `join_all_rooms` report `already_joined` on a room Wire never
    /// accepted. That covers the stale-snapshot race too — another
    /// own device established first, Wire rejects this bundle, the
    /// next sync shows `epoch > 0` and the retry external-joins.
    ///
    /// # Errors
    /// * `KupferdrahtError::InvalidInput` if the room has no `group_id`
    ///   or the client has no MLS state.
    /// * `KupferdrahtError::BackendRejected` / `Network` from the claim
    ///   and bundle upload; `Generic` / `Storage` from openmls.
    pub(super) async fn establish_self_room(&self, room: &Room) -> Result<(), KupferdrahtError> {
        debug_assert!(
            room.room_type() == RoomType::SelfRoom,
            "establish_self_room: only the self room is self-established"
        );
        let room_id = room.id();
        let group_id = room.group_id_bytes().ok_or_else(|| {
            KupferdrahtError::InvalidInput(format!(
                "establish_self_room: {room_id} has no group_id — run sync first"
            ))
        })?;
        // Server data must never trip an assert (recoverable-error rule).
        if group_id.is_empty() {
            return Err(KupferdrahtError::InvalidInput(format!(
                "establish_self_room: {room_id} carries an empty group_id"
            )));
        }
        let mls = self.mls_client_required("establish_self_room")?;
        {
            let _gate = self.mls_write_gate().await;
            let guard = crate::encryption::lock_mls(mls);
            guard.create_group(&group_id, &self.backend_removal_key_ecdsa_p256)?;
        }
        if let Err(establish_err) = self
            .establish_group(mls, &group_id, "establish_self_room")
            .await
        {
            // Best-effort discard: the establish error is what the
            // caller must see, not a secondary storage failure.
            let _gate = self.mls_write_gate().await;
            if let Err(e) = crate::encryption::lock_mls(mls).discard_group(&group_id) {
                warn!(error = %e, "establish_self_room: discard after failed establish failed");
            }
            return Err(establish_err);
        }
        let new_epoch = {
            let guard = crate::encryption::lock_mls(mls);
            guard.group_epoch(&group_id)?
        };
        // Code invariant: the merged establishing commit advanced the
        // fresh group past epoch 0 (openmls, not server data).
        assert!(
            new_epoch > 0,
            "establish_self_room: merged establishing commit left epoch 0"
        );
        info!(%room_id, epoch = new_epoch, "self room established");
        self.bump_room_epoch(&room_id, new_epoch).await
    }
}
