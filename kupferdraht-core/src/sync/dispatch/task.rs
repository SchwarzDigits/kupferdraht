// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Inbound task dispatch (`TaskPayload`, `GenericMessage` field 262000)
//! — Step 60a, ADR-0014.
//!
//! The task payload travels OUTSIDE the `content` oneof so it can
//! ride alongside a `Text` fallback (mode c). `dispatch_mls_message`
//! checks field 262000 BEFORE the oneof — a task message with fallback
//! must become a Task item, never a Text item.

use std::sync::Arc;

use crate::client::Client;
use crate::error::KupferdrahtError;
use crate::sync::service::SyncService;

/// Step 95c: whether an inbound `TaskPayload` can be folded at all —
/// proto2 `required task_id` is not enforced by prost, and an empty id
/// addresses no task. An invalid payload is ignored (the message then
/// renders its `Text` fallback, as on a plain Wire client).
pub(crate) fn task_payload_is_valid(task: &crate::wire::proto::TaskPayload) -> bool {
    let valid = !task.task_id.trim().is_empty();
    if !valid {
        tracing::warn!("inbound task payload without a task_id ignored");
    }
    valid
}

impl SyncService {
    /// Step 60a: dispatch an inbound task message. The payload is the
    /// full state of one task (create == update, ADR-0014); folding /
    /// upsert semantics land in Step 60b — 60a persists the item.
    pub(crate) async fn dispatch_inbound_task(
        &self,
        client: &Arc<Client>,
        room_id: &str,
        sender_user_id: &str,
        sender_device_id: Option<&str>,
        message_id: &str,
        task: crate::wire::proto::TaskPayload,
    ) -> Result<(), KupferdrahtError> {
        debug_assert!(
            !message_id.is_empty(),
            "dispatch_inbound_task: message_id must be non-empty"
        );
        let item = crate::timeline::task::item_from_payload(&task);
        let cached = crate::timeline::FoldedTask {
            item: item.clone(),
            creator_user_id: sender_user_id.to_string(),
            created_at_ms: crate::time::now_ms(),
            updated_at_ms: crate::time::now_ms(),
            latest_message_id: message_id.to_string(),
        };
        // Step 105: the task row and its fold row are one write — only a
        // fresh row updates the fold (a replayed old message must not
        // regress it; LWW is arrival order, Step 60c).
        let message = super::inbound_message(
            room_id,
            sender_user_id,
            sender_device_id,
            message_id,
            crate::sync::event_handler::MessageBody::Task(Box::new(item)),
        );
        let fold = crate::timeline::task::task_record_from_folded(room_id, &cached);
        self.surface_inbound_message(client, &message, Some(&fold))
            .await?;
        Ok(())
    }
}
