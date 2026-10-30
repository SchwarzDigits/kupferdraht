// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Network follow-ups of a dispatch cycle (Step 97d, ADR-0031).
//!
//! Dispatch runs under the account's drain lock and does local work
//! only. What needs the network — healing a room, a 1:1 delivery
//! receipt, re-fetching a config the event only named — is recorded
//! here and run by `end_dispatch_cycle`, after the lock is released
//! (and before the Step-74 removal flush). Every follow-up is best
//! effort: a failure warns and reaches `on_sync_error`, the cycle goes on.

use std::sync::Arc;

use tracing::warn;

use crate::client::Client;
use crate::sync::service::SyncService;

/// One piece of network work a dispatch cycle left for after the lock.
pub(crate) enum FollowUp {
    /// Resync a room whose local group is missing (Step 64).
    HealRoom(String),
    /// Resync a room whose inbound message came from a later epoch — only
    /// when the backend confirms the gap (Step 106, ADR-0034 d.1).
    HealRoomIfBehind(String),
    /// Acknowledge an inbound 1:1 message as delivered (Step 25b).
    ConfirmDelivery { room_id: String, message_id: String },
    /// Re-fetch the team feature configs (Step 10d).
    RefetchFeatureConfigs,
    /// Re-fetch one user property (Step 10d).
    RefetchProperty(String),
}

impl SyncService {
    /// Record `follow_up` for the end of the current dispatch cycle.
    pub(crate) fn queue_follow_up(&self, follow_up: FollowUp) {
        self.follow_ups
            .lock()
            .expect("follow_ups lock")
            .push(follow_up);
    }

    /// Run and forget every recorded follow-up, in order.
    pub(super) async fn run_follow_ups(&self, client: &Arc<Client>) {
        let queued = std::mem::take(&mut *self.follow_ups.lock().expect("follow_ups lock"));
        for follow_up in queued {
            self.run_follow_up(client, follow_up).await;
        }
        debug_assert!(
            self.follow_ups.lock().expect("follow_ups lock").is_empty(),
            "a follow-up never queues another"
        );
    }

    async fn run_follow_up(&self, client: &Arc<Client>, follow_up: FollowUp) {
        let outcome = match follow_up {
            FollowUp::HealRoom(room_id) => {
                warn!(room_id, "drain: local group missing; resyncing room");
                client
                    .resync_room(&room_id)
                    .await
                    .map_err(|e| format!("drain: recovery resync failed: {e}"))
            }
            FollowUp::HealRoomIfBehind(room_id) => heal_if_backend_ahead(client, &room_id).await,
            FollowUp::ConfirmDelivery {
                room_id,
                message_id,
            } => {
                super::dispatch::maybe_send_delivery_confirmation(client, &room_id, &message_id)
                    .await;
                Ok(())
            }
            FollowUp::RefetchFeatureConfigs => super::dispatch::refetch_feature_configs(client)
                .await
                .map_err(|e| format!("drain: {e}")),
            FollowUp::RefetchProperty(key) => super::dispatch::refetch_user_property(client, &key)
                .await
                .map_err(|e| format!("drain: {e}")),
        };
        if let Err(description) = outcome {
            warn!(%description, "dispatch follow-up failed");
            self.notify_error(&description);
        }
    }
}

/// Heal `room_id` for a later-epoch message only when the backend's epoch
/// is ahead of the local group (ADR-0034 d.1); otherwise the message was
/// undecryptable noise — reported, not healed around.
async fn heal_if_backend_ahead(client: &Arc<Client>, room_id: &str) -> Result<(), String> {
    match client.backend_epoch_ahead(room_id).await {
        Ok(true) => {
            warn!(
                room_id,
                "drain: backend confirms a missed commit; resyncing room"
            );
            client
                .resync_room(room_id)
                .await
                .map_err(|e| format!("drain: recovery resync failed: {e}"))
        }
        Ok(false) => Err(format!(
            "drain: skipped an undecryptable message in {room_id} — its later epoch is not \
             the conversation's (the backend reports no missed commit)"
        )),
        Err(e) => Err(format!(
            "drain: could not confirm an epoch gap in {room_id}; not healed: {e}"
        )),
    }
}
