// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Inbound case dispatch (`CasePayload`, `GenericMessage` field
//! 262001) — Step 72b, ADR-0018.
//!
//! The case payload travels OUTSIDE the `content` oneof and carries
//! no text fallback. `dispatch_mls_message` checks field 262001
//! BEFORE the task field and the oneof. Every case row — admin or
//! not — is persisted (audit trail) with the receive-time
//! authorization flag in the body; the fold happens on read
//! (`Room::case`), so no cache can go stale and a redaction needs
//! no rebuild hook.

use std::sync::Arc;

use crate::client::Client;
use crate::error::KupferdrahtError;
use crate::rooms::RoomMemberRole;
use crate::sync::service::SyncService;

/// Step 95c: whether an inbound `CasePayload` names a case — proto2
/// `required case_ref` is not enforced by prost, and a snapshot without
/// one can neither be folded nor re-broadcast. An invalid payload is
/// dropped (cases carry no fallback).
pub(crate) fn case_payload_is_valid(case: &crate::wire::proto::CasePayload) -> bool {
    let valid = !case.case_ref.trim().is_empty();
    if !valid {
        tracing::warn!("inbound case payload without a case_ref dropped");
    }
    valid
}

impl SyncService {
    /// Step 72b: dispatch an inbound case message. Captures whether
    /// the sender is a conversation administrator **now** (the
    /// Step-72a metadata roles — ADR-0018's receive-time
    /// authorization) into the persisted row, so later role changes
    /// never re-evaluate history. A sender the metadata does not
    /// know is unauthorized.
    pub(crate) async fn dispatch_inbound_case(
        &self,
        client: &Arc<Client>,
        room_id: &str,
        sender_user_id: &str,
        sender_device_id: Option<&str>,
        message_id: &str,
        payload: crate::wire::proto::CasePayload,
    ) -> Result<(), KupferdrahtError> {
        debug_assert!(
            !message_id.is_empty(),
            "dispatch_inbound_case: message_id must be non-empty"
        );
        let snapshot = crate::timeline::case::snapshot_from_payload(&payload);
        let sender_authorized =
            match client.room(room_id).await {
                Some(room) => room.members().await?.iter().any(|m| {
                    m.user_id == sender_user_id && m.role == RoomMemberRole::Administrator
                }),
                None => false,
            };
        self.dispatch_inbound_simple(
            client,
            room_id,
            sender_user_id,
            sender_device_id,
            message_id,
            crate::sync::event_handler::MessageBody::Case {
                snapshot: Box::new(snapshot),
                sender_authorized,
            },
        )
        .await?;
        Ok(())
    }
}
