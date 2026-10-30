// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Per-content-variant inbound dispatchers — Step 6-II + Step 10d.
//!
//! `SyncService` splits its receive pipeline across two files:
//!
//! - `sync/service.rs` carries the long-running drain loop and the
//!   per-event entry point (`dispatch_event` → `dispatch_mls_message`).
//! - `sync/dispatch.rs` (this file) carries the per-event handlers:
//!   the `GenericMessage`-variant ones from Step 6-II
//!   (`dispatch_inbound_text` / `_confirmation` / `_reaction` / `_delete`
//!   / `_edit`) plus the Step-10d server-administered-config arms
//!   (`dispatch_feature_config_update`, `dispatch_user_property_set`,
//!   `dispatch_user_property_delete`).
//!
//! Reviewers landing on a 6-II-c regression don't have to page
//! through the WebSocket-shaped drain loop, and vice-versa.

mod asset;
mod authorize;
pub(super) mod case;
mod config;
mod confirmation;
mod delete;
mod edit;
mod ephemeral;
mod knock;
mod location;
mod marker;
mod reaction;
pub(super) mod task;
mod text;

pub(super) use config::{refetch_feature_configs, refetch_user_property};

use std::sync::Arc;

use tracing::{debug, warn};

use crate::client::Client;
use crate::error::KupferdrahtError;
use crate::storage::{TaskRecord, TimelineEventRecord};
use crate::sync::event_handler::{Message, MessageBody};
use crate::sync::service::SyncService;
use crate::time::now_ms;
use crate::timeline::DeliveryStatus;

impl SyncService {
    /// Persist an inbound non-text content message (knock / location /
    /// case / marker / task) via [`message_to_record`] — a task together
    /// with its fold row, one write (Step 105). Returns whether a fresh
    /// row was inserted; a self-sent round-trip or a replay writes
    /// nothing. Step 26.
    async fn persist_inbound_message(
        &self,
        client: &Arc<Client>,
        message: &Message,
        task: Option<&TaskRecord>,
    ) -> Result<bool, KupferdrahtError> {
        let Some(store) = client.store_handle() else {
            return Ok(false);
        };
        let event = message_to_record(message);
        let fresh = match task {
            Some(task) => store.insert_timeline_event_with_task(&event, task).await?,
            None => {
                store
                    .insert_timeline_event_with_mentions(&event, &[])
                    .await?
            }
        };
        if !fresh {
            debug!(message_id = %event.message_id, "skipping duplicate timeline_events row");
        }
        Ok(fresh)
    }

    /// Dispatch a simple inbound content message (no mentions / attachments):
    /// persist, notify, emit `Append`, and auto-ack delivery on a fresh
    /// insert. Shared by knock, location, and task. Returns whether a
    /// fresh row was inserted (false on a redelivery) so callers can
    /// skip side effects for duplicates — the task fold cache must not
    /// regress to an older state (Step 60c). Step 26.
    async fn dispatch_inbound_simple(
        &self,
        client: &Arc<Client>,
        room_id: &str,
        sender_user_id: &str,
        sender_device_id: Option<&str>,
        message_id: &str,
        body: MessageBody,
    ) -> Result<bool, KupferdrahtError> {
        let message = inbound_message(room_id, sender_user_id, sender_device_id, message_id, body);
        self.surface_inbound_message(client, &message, None).await
    }

    /// Persist an inbound content message (with its task fold row when
    /// `task` is set) and, for a fresh row, notify, emit the `Append` and
    /// queue the delivery receipt. Returns whether the row was fresh.
    pub(super) async fn surface_inbound_message(
        &self,
        client: &Arc<Client>,
        message: &Message,
        task: Option<&TaskRecord>,
    ) -> Result<bool, KupferdrahtError> {
        let room_id = message.room_id.as_str();
        let message_id = message.message_id.as_str();
        let inserted = self.persist_inbound_message(client, message, task).await?;
        if inserted {
            // Step 65 residual (review re-check): same guard as the
            // text path — a redelivered row must not re-fire the
            // binding callbacks.
            self.notify_message(message);
            client.dispatch_timeline_diff(&crate::timeline::TimelineDiff::Append {
                room_id: message.room_id.clone(),
                item: crate::timeline::TimelineItem::from_record(message_to_record(message)),
            });
            self.queue_delivery_confirmation(room_id, message_id);
        }
        Ok(inserted)
    }
}

/// The consumer-facing `Message` of an inbound content event, received now.
pub(super) fn inbound_message(
    room_id: &str,
    sender_user_id: &str,
    sender_device_id: Option<&str>,
    message_id: &str,
    body: MessageBody,
) -> Message {
    Message {
        room_id: room_id.to_string(),
        sender_user_id: sender_user_id.to_string(),
        sender_device_id: sender_device_id.map(str::to_string),
        body,
        received_at_ms: now_ms(),
        message_id: message_id.to_string(),
        reply_to_id: None,
        expires_at_ms: None,
    }
}

impl SyncService {
    /// Record the 1:1 delivery receipt of a freshly stored inbound
    /// message; it is sent as a follow-up after the drain lock (Step 97d,
    /// ADR-0031) by [`maybe_send_delivery_confirmation`].
    pub(super) fn queue_delivery_confirmation(&self, room_id: &str, message_id: &str) {
        debug_assert!(
            !message_id.is_empty(),
            "delivery receipt needs a message id"
        );
        self.queue_follow_up(crate::sync::follow_ups::FollowUp::ConfirmDelivery {
            room_id: room_id.to_string(),
            message_id: message_id.to_string(),
        });
    }
}

/// Auto-acknowledge an inbound content message with a `DELIVERED`
/// confirmation back to the sender — only in 1:1 rooms (group delivery would
/// be N^2 traffic for little value) and **ungated** by the read-receipt
/// setting. Step 25b. Best-effort: a failed confirmation must not break
/// receiving the message. Sent per message; batching a sync burst is a
/// possible future optimisation.
pub(super) async fn maybe_send_delivery_confirmation(
    client: &Arc<Client>,
    room_id: &str,
    message_id: &str,
) {
    debug_assert!(
        !message_id.is_empty(),
        "maybe_send_delivery_confirmation: message_id must be non-empty"
    );
    let is_dm = client
        .room(room_id)
        .await
        .is_some_and(|r| r.room_type() == crate::rooms::RoomType::DirectMessage);
    if !is_dm {
        return;
    }
    if let Err(e) = client
        .send_delivered_confirmation(room_id, &[message_id.to_string()])
        .await
    {
        warn!(message_id, error = ?e, "delivery confirmation send failed (ignored)");
    }
}

/// Project Wire's protobuf `Mention` list into Kupferdraht's
/// `MentionRecord` shape, resolving each `qualified_user_id`
/// into its `<uuid>@<domain>` form. Mentions whose required
/// fields are missing (no qualified id, length 0) are dropped
/// silently — rather than rejecting the whole inbound message,
/// we accept the body and skip the malformed mention. Step 6-II-g.
fn mentions_to_records(
    room_id: &str,
    message_id: &str,
    mentions: &[crate::wire::proto::Mention],
) -> Vec<crate::storage::MentionRecord> {
    mentions
        .iter()
        .filter_map(|m| {
            let q = m.qualified_user_id.as_ref()?;
            if m.length <= 0 {
                return None;
            }
            // Step 95c: an unaddressable mention is dropped, the
            // message itself is kept.
            let user_id = crate::identity::UserId::from_parts(&q.id, &q.domain).ok()?;
            Some(crate::storage::MentionRecord {
                message_id: message_id.to_string(),
                room_id: room_id.to_string(),
                user_id: user_id.as_str().to_string(),
                start: m.start,
                length: m.length,
            })
        })
        .collect()
}

/// Project an in-flight inbound `Message` into the `TimelineEventRecord`
/// shape used by `TimelineItem::from_record`. Used by the
/// `TimelineDiff::Append` emitted on a fresh inbound text. Only
/// `MessageBody::Text` is in scope here — `MessageBody::Asset`
/// follows its own dispatch path (Step 11g) and does not pass
/// through this helper.
fn message_to_record(message: &Message) -> TimelineEventRecord {
    // Map the body to the persisted (kind, body) pair. Knock/Location
    // (Step 26) and Task (Step 60a) pack their small payload into the
    // `body` TEXT column. Asset has its own persist path and never
    // reaches here.
    let (kind, body) = match &message.body {
        MessageBody::Text(b) => ("text", b.clone()),
        MessageBody::Knock { hot_knock } => ("knock", hot_knock.to_string()),
        MessageBody::Location {
            latitude,
            longitude,
            name,
            zoom,
        } => (
            "location",
            crate::timeline::location_body(*latitude, *longitude, name.as_deref(), *zoom),
        ),
        MessageBody::Task(item) => ("task", crate::timeline::task::task_body(item)),
        MessageBody::TaskListMarker { marked } => (
            "task_list_marker",
            crate::timeline::task::marker_body(*marked),
        ),
        MessageBody::Case {
            snapshot,
            sender_authorized,
        } => (
            "case",
            crate::timeline::case::case_body(snapshot, *sender_authorized),
        ),
        MessageBody::Asset(_) => unreachable!("message_to_record called with Asset body"),
    };
    TimelineEventRecord {
        message_id: message.message_id.clone(),
        room_id: message.room_id.clone(),
        sender_user_id: message.sender_user_id.clone(),
        sender_device_id: message.sender_device_id.clone().unwrap_or_default(),
        kind: kind.into(),
        body,
        sent_at_ms: message.received_at_ms,
        delivery_status: DeliveryStatus::Received.as_db_str().into(),
        reply_to_id: message.reply_to_id.clone(),
        edited_at_ms: None,
        redacted_at_ms: None,
        expires_at_ms: message.expires_at_ms,
        ephemeral_ttl_ms: None,
    }
}
