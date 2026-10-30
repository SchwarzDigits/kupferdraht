// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Inbound `MessageDelete` dispatch — Step 6-II-d.

use std::sync::Arc;

use tracing::{info, warn};

use crate::client::Client;
use crate::error::KupferdrahtError;
use crate::sync::service::SyncService;
use crate::time::now_ms;

use super::authorize::{TargetOp, target_op_is_authorized};

impl SyncService {
    /// Apply an inbound `MessageDelete` to the parent row. Step 6-II-d.
    /// Idempotent via `Store::apply_message_delete`'s `redacted_at_ms`
    /// guard. The chain pointer covers deletes targeting an
    /// already-edited row. Only the original sender may delete — or, for
    /// an **ephemeral** target, the self user (another own device) or,
    /// when the target is the self user's own message, any member
    /// (Step 95b, ADR-0028, `authorize.rs`).
    pub(crate) async fn dispatch_inbound_delete(
        &self,
        client: &Arc<Client>,
        room_id: &str,
        sender_user_id: &str,
        delete_event_id: &str,
        delete: crate::wire::proto::MessageDelete,
    ) -> Result<(), KupferdrahtError> {
        info!(
            delete_event_id,
            target = delete.message_id,
            "MessageDelete received"
        );
        let Some(store) = client.store_handle() else {
            return Ok(());
        };
        if !target_op_is_authorized(
            client,
            room_id,
            &delete.message_id,
            sender_user_id,
            TargetOp::Delete,
        )
        .await?
        {
            return Ok(());
        }
        let applied = store
            .apply_message_delete(room_id, &delete.message_id, now_ms())
            .await?;
        if !applied {
            warn!(
                delete_event_id,
                target = delete.message_id,
                "MessageDelete had no effect (parent missing locally or older than persisted delete)"
            );
            return Ok(());
        }
        if let Some(canonical) = client
            .canonical_message_id(room_id, &delete.message_id)
            .await?
        {
            client.emit_message_update_at(room_id, &canonical).await?;
        }
        // Step 60c: a redacted task message drops out of the fold —
        // rebuild the room's materialized task cache (kind survives
        // the body wipe, so the check is cheap).
        crate::timeline::task::rebuild_tasks_if_redacted_task(store, room_id, &delete.message_id)
            .await?;
        Ok(())
    }
}
