// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Inbound `MessageEdit` dispatch — Step 6-II-c.

use std::sync::Arc;

use tracing::{info, warn};

use crate::client::Client;
use crate::error::KupferdrahtError;
use crate::storage::MessageEditRecord;
use crate::sync::service::SyncService;
use crate::time::now_ms;

use super::authorize::{TargetOp, target_op_is_authorized};
use super::mentions_to_records;

impl SyncService {
    /// Apply an inbound `MessageEdit` to the parent row. Step 6-II-c.
    /// Idempotent: re-applying an older or equal edit is a no-op via
    /// `Store::apply_message_edit`'s `edited_at_ms` guard. Only the
    /// parent's original sender may edit it (Step 95b, ADR-0028).
    pub(crate) async fn dispatch_inbound_edit(
        &self,
        client: &Arc<Client>,
        room_id: &str,
        sender_user_id: &str,
        edit_message_id: &str,
        edit: crate::wire::proto::MessageEdit,
    ) -> Result<(), KupferdrahtError> {
        // Out-of-scope MessageEdit variants (Composite, Multipart) are
        // logged + skipped — matches the GenericMessage policy.
        let Some(crate::wire::proto::message_edit::Content::Text(text)) = edit.content else {
            warn!(
                edit_message_id,
                replacing = edit.replacing_message_id,
                "MessageEdit with non-Text content — skip dispatch"
            );
            return Ok(());
        };
        info!(
            edit_message_id,
            replacing = edit.replacing_message_id,
            new_body_len = text.content.len(),
            "MessageEdit received"
        );
        if !target_op_is_authorized(
            client,
            room_id,
            &edit.replacing_message_id,
            sender_user_id,
            TargetOp::Edit,
        )
        .await?
        {
            return Ok(());
        }
        let canonical = persist_inbound_edit(
            client,
            room_id,
            edit_message_id,
            &edit.replacing_message_id,
            &text,
        )
        .await?;
        match canonical {
            Some(canonical) => {
                client.emit_message_update_at(room_id, &canonical).await?;
            }
            None => {
                warn!(
                    edit_message_id,
                    replacing = edit.replacing_message_id,
                    "MessageEdit had no effect (parent missing locally or older than persisted edit)"
                );
            }
        }
        Ok(())
    }
}

/// Apply an inbound `MessageEdit` to `timeline_events` and the
/// associated mentions row-set. Returns the canonical (stable PK)
/// message id when the edit was applied, or `None` when no `Store`
/// is attached or the edit was idempotent-skipped (older or equal
/// `edited_at_ms` than what's persisted, or parent missing).
async fn persist_inbound_edit(
    client: &Arc<Client>,
    room_id: &str,
    edit_event_id: &str,
    replacing_message_id: &str,
    text: &crate::wire::proto::Text,
) -> Result<Option<String>, KupferdrahtError> {
    let Some(store) = client.store_handle() else {
        return Ok(None);
    };
    // Resolve the stable PK *before* `apply_message_edit` advances
    // `current_edit_event_id`; otherwise the prior chain link is no
    // longer reachable via `canonical_message_id`.
    let stable_message_id = client
        .canonical_message_id(room_id, replacing_message_id)
        .await?
        .unwrap_or_else(|| replacing_message_id.to_string());
    // Step 105: the edit and its mentions are one write.
    let mention_records = mentions_to_records(room_id, &stable_message_id, &text.mentions);
    let edit = MessageEditRecord {
        room_id: room_id.to_string(),
        message_id: stable_message_id,
        replacing_message_id: replacing_message_id.to_string(),
        edit_event_id: edit_event_id.to_string(),
        new_body: text.content.clone(),
        edited_at_ms: now_ms(),
    };
    let applied = store
        .apply_message_edit_with_mentions(&edit, &mention_records)
        .await?;
    Ok(applied.then_some(edit.message_id))
}
