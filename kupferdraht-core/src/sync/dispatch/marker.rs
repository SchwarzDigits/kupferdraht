// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Inbound task-list-marker dispatch (`TaskListMarker`,
//! `GenericMessage` field 262002) — Step 75, ADR-0020.
//!
//! The marker payload travels OUTSIDE the `content` oneof and carries
//! no text fallback. `dispatch_mls_message` checks field 262002
//! alongside the other Kupferdraht siblings, before the oneof. Every
//! marker row — any member may flip the flag — is persisted as the
//! audit trail; the fold happens on read (`Room::is_task_list`), so
//! no cache can go stale and a redaction needs no rebuild hook.

use std::sync::Arc;

use crate::client::Client;
use crate::error::KupferdrahtError;
use crate::sync::service::SyncService;

impl SyncService {
    /// Step 75: dispatch an inbound task-list-marker message. No
    /// authorization capture — the marker is any-member by decision
    /// (ADR-0020: an admin gate would inherit the §72 hole).
    pub(crate) async fn dispatch_inbound_task_list_marker(
        &self,
        client: &Arc<Client>,
        room_id: &str,
        sender_user_id: &str,
        sender_device_id: Option<&str>,
        message_id: &str,
        marked: bool,
    ) -> Result<(), KupferdrahtError> {
        debug_assert!(
            !message_id.is_empty(),
            "dispatch_inbound_task_list_marker: message_id must be non-empty"
        );
        self.dispatch_inbound_simple(
            client,
            room_id,
            sender_user_id,
            sender_device_id,
            message_id,
            crate::sync::event_handler::MessageBody::TaskListMarker { marked },
        )
        .await?;
        Ok(())
    }
}
