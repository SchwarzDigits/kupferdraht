// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Inbound `Text` dispatch — Step 6-II-a + 6-II-g (mentions).
//!
//! Step 89 splits the handler into the `Message` builder
//! ([`inbound_text_message`]) and the persist / notify / emit tail
//! ([`SyncService::dispatch_inbound_text_message`]) so the
//! `Ephemeral(Text)` handler can reuse the tail with an expiry stamp.

use std::sync::Arc;

use tracing::debug;

use crate::client::Client;
use crate::error::KupferdrahtError;
use crate::storage::{MentionRecord, TimelineEventRecord};
use crate::sync::event_handler::{Message, MessageBody};
use crate::sync::service::SyncService;
use crate::time::now_ms;
use crate::timeline::DeliveryStatus;

use super::{mentions_to_records, message_to_record};

/// Project an inbound `Text` into the consumer-facing `Message`.
/// `ttl_ms` is `Some` for an `Ephemeral(Text)` (Step 89): the
/// timer starts at dispatch, so `expires_at_ms = received_at_ms + ttl`.
pub(super) fn inbound_text_message(
    room_id: &str,
    sender_user_id: &str,
    sender_device_id: Option<&str>,
    message_id: &str,
    text: crate::wire::proto::Text,
    ttl_ms: Option<i64>,
) -> Message {
    let received_at_ms = now_ms();
    Message {
        room_id: room_id.to_string(),
        sender_user_id: sender_user_id.to_string(),
        sender_device_id: sender_device_id.map(str::to_string),
        body: MessageBody::Text(text.content),
        received_at_ms,
        message_id: message_id.to_string(),
        reply_to_id: text.quote.map(|q| q.quoted_message_id),
        expires_at_ms: ttl_ms.map(|ttl| received_at_ms.saturating_add(ttl)),
    }
}

impl SyncService {
    pub(crate) async fn dispatch_inbound_text(
        &self,
        client: &Arc<Client>,
        room_id: &str,
        sender_user_id: &str,
        sender_device_id: Option<&str>,
        message_id: &str,
        text: crate::wire::proto::Text,
    ) -> Result<(), KupferdrahtError> {
        let mention_records = mentions_to_records(room_id, message_id, &text.mentions);
        let message = inbound_text_message(
            room_id,
            sender_user_id,
            sender_device_id,
            message_id,
            text,
            None,
        );
        self.dispatch_inbound_text_message(client, message, mention_records)
            .await
    }

    /// Persist an inbound text `Message`, replace its mentions, notify
    /// the handlers, emit the `Append`, auto-ack delivery (1:1), and
    /// arm the expiry timer when the message is ephemeral. The echo of
    /// an own send (row already present) upgrades `Submitted →
    /// Received` instead. Shared by the plain-text and the
    /// `Ephemeral(Text)` handlers (Step 89).
    pub(super) async fn dispatch_inbound_text_message(
        &self,
        client: &Arc<Client>,
        message: Message,
        mention_records: Vec<MentionRecord>,
    ) -> Result<(), KupferdrahtError> {
        if self
            .persist_inbound_text(client, &message, &mention_records)
            .await?
        {
            self.surface_fresh_inbound_text(client, &message, &mention_records);
            Ok(())
        } else {
            upgrade_echoed_own_text(client, &message).await
        }
    }

    /// The fresh-insert half of the inbound text tail (the mentions were
    /// stored with the row, Step 105): `on_message`, the `Append` diff, the 1:1 delivery receipt, and
    /// (Step 89) the local expiry timer for an ephemeral row — armed on
    /// the fresh insert only; an echoed own send was armed at send time.
    fn surface_fresh_inbound_text(
        &self,
        client: &Arc<Client>,
        message: &Message,
        mention_records: &[MentionRecord],
    ) {
        let room_id = message.room_id.as_str();
        let message_id = message.message_id.as_str();
        // Step 65 (review S6): notify only for fresh rows — a
        // redelivered/echoed row already reached the handlers (or
        // was the app's own send); re-firing duplicates binding
        // callbacks. An other-device own message inserts fresh and
        // still notifies.
        self.notify_message(message);
        // 6-II-a: emit TimelineDiff::Append for inbound text. Self-sent
        // round-trips already inserted at send time and were skipped by
        // persist_inbound_text's UNIQUE-conflict guard; those don't fire here.
        // Step 65 (review S10): hydrate the mentions into the Append
        // item — subscribers used to see them only after a reload.
        client.dispatch_timeline_diff(&crate::timeline::TimelineDiff::Append {
            room_id: message.room_id.clone(),
            item: crate::timeline::TimelineItem::hydrated(
                message_to_record(message),
                mention_records,
                &[],
                &client.weak(),
            ),
        });
        // 25b: auto-acknowledge delivery (1:1 only, best-effort).
        self.queue_delivery_confirmation(room_id, message_id);
        if let Some(at) = message.expires_at_ms {
            client.arm_ephemeral_timer(at);
        }
    }
}

/// The already-present half of the inbound text tail. Step 65 (review
/// S12): the Wire echo of an own message upgrades `Submitted →
/// Received` ("awaiting the Wire echo" was the documented contract all
/// along) and surfaces as an `UpdateAt` so UIs drop the pending state.
async fn upgrade_echoed_own_text(
    client: &Arc<Client>,
    message: &Message,
) -> Result<(), KupferdrahtError> {
    let Some(store) = client.store_handle() else {
        return Ok(());
    };
    if store
        .mark_delivery_received(&message.room_id, &message.message_id)
        .await?
    {
        client
            .emit_message_update_at(&message.room_id, &message.message_id)
            .await?;
    }
    Ok(())
}

impl SyncService {
    /// Insert the inbound row into `timeline_events` together with its
    /// mentions — one write (Step 105). Returns `Ok(true)` when a fresh
    /// row was inserted, `Ok(false)` when the message already existed
    /// (self-sent round-trip, replay) — the latter must NOT fire a
    /// `TimelineDiff::Append` because the outbound send-path already did.
    async fn persist_inbound_text(
        &self,
        client: &Arc<Client>,
        message: &Message,
        mention_records: &[MentionRecord],
    ) -> Result<bool, KupferdrahtError> {
        let Some(store) = client.store_handle() else {
            return Ok(false);
        };
        let MessageBody::Text(body) = &message.body else {
            // Step 11g dispatch path handles Asset; this text-only
            // persist is unreachable for non-Text bodies.
            return Ok(false);
        };
        let event = TimelineEventRecord {
            message_id: message.message_id.clone(),
            room_id: message.room_id.clone(),
            sender_user_id: message.sender_user_id.clone(),
            sender_device_id: message.sender_device_id.clone().unwrap_or_default(),
            kind: "text".into(),
            body: body.clone(),
            sent_at_ms: message.received_at_ms,
            delivery_status: DeliveryStatus::Received.as_db_str().into(),
            reply_to_id: message.reply_to_id.clone(),
            edited_at_ms: None,
            redacted_at_ms: None,
            expires_at_ms: message.expires_at_ms,
            ephemeral_ttl_ms: None,
        };
        // A self-sent message round-trips through Wire and arrives
        // here too — the row already exists with status='submitted';
        // the store reports it and writes nothing.
        let fresh = store
            .insert_timeline_event_with_mentions(&event, mention_records)
            .await?;
        if !fresh {
            debug!(message_id = %event.message_id, "skipping duplicate timeline_events row");
        }
        Ok(fresh)
    }
}
