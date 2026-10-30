// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Read / delivery confirmations (`Confirmation` `GenericMessage`) —
//! Steps 6-II-f (READ) and 25b (DELIVERED), split out of the former
//! monolithic `timeline/send.rs` by Step 30.
//!
//! READ confirmations are user-driven (`mark_as_read`) and locally
//! self-applied so the reader sees their own receipt immediately;
//! DELIVERED confirmations are the recipient's automatic
//! acknowledgement and carry no local self-apply (`delivered_to` is
//! the sender's view).

use crate::time::now_ms;

use crate::client::Client;
use crate::error::KupferdrahtError;
use crate::timeline::{MessageHandle, Timeline};

impl Timeline {
    /// Mark a message as read (Step 6-II-f). Wire-side emits a
    /// `Confirmation { type: READ, first_message_id: item_id }`
    /// carrying just the supplied id (Kupferdraht's CLI doesn't yet
    /// batch). Receivers stamp `(message_id, reader_user_id)`
    /// into `read_receipts`; Wire web's UI shows a "read" indicator
    /// once the recipient has confirmed.
    ///
    /// **Step 10f gate**: when the user's `WIRE_RECEIPT_MODE`
    /// preference is `Off`, this method short-circuits to `Ok(None)`
    /// without sending the `Confirmation`. The caller can use the
    /// `None` return as a signal that read receipts are off
    /// account-wide. Other Wire clients respect the same setting,
    /// so peers won't expect a receipt to arrive. See ADR-0012
    /// §Decision and `Client::receipt_mode` for the
    /// preference projection.
    ///
    /// matrix-rust-sdk analog: `Timeline::send_single_receipt`.
    ///
    /// # Errors
    /// Same as [`Self::send_text`]; additionally `InvalidInput` when
    /// `item_id` is empty.
    pub async fn mark_as_read(
        &self,
        item_id: String,
    ) -> Result<Option<MessageHandle>, KupferdrahtError> {
        if item_id.is_empty() {
            return Err(KupferdrahtError::InvalidInput(
                "mark_as_read: item_id must be non-empty".into(),
            ));
        }
        // Step 67: advance the device-local read marker BEFORE the
        // receipt gate below — the gate governs the outbound wire
        // signal, not the local read position. The marker only moves
        // forward (store-side MAX), so marking an older item cannot
        // resurrect read messages as unread. An item unknown locally
        // anchors no position and skips the marker.
        if let Some(record) = self
            .client
            .lookup_timeline_event(&self.room_id, &item_id)
            .await?
            && let Some(store) = self.client.store_handle()
        {
            store
                .set_read_marker(&self.room_id, record.sent_at_ms)
                .await?;
        }
        if matches!(
            self.client.receipt_mode().await?,
            crate::client::ReceiptMode::Off
        ) {
            return Ok(None);
        }
        self.client
            .send_read_confirmation(&self.room_id, &[item_id])
            .await
            .map(Some)
    }
}

impl Client {
    /// See [`Timeline::mark_as_read`] for the caller-facing
    /// contract. Step 6-II-f.
    pub(crate) async fn send_read_confirmation(
        &self,
        room_id: &str,
        message_ids: &[String],
    ) -> Result<MessageHandle, KupferdrahtError> {
        if message_ids.is_empty() {
            return Err(KupferdrahtError::InvalidInput(
                "send_read_confirmation: message_ids must be non-empty".into(),
            ));
        }
        let group_id = self.resolve_room_group_id(room_id, "mark_as_read").await?;
        let (event_id, plaintext) =
            crate::wire::proto::build_read_confirmation_generic_message(message_ids)?;
        self.encrypt_and_upload(&group_id, "mark_as_read", &plaintext)
            .await?;
        let confirmed_at_ms = now_ms();
        // Local mirror: we are the reader. Resolve canonical ids
        // and apply our own receipt so `read_by` shows ourselves
        // immediately.
        let self_user_id = self.own_qualified_user_id();
        if let Some(store) = self.store_handle() {
            let canonical_ids = self.resolve_canonical_ids(room_id, message_ids).await?;
            store
                .apply_read_receipts(room_id, &self_user_id, &canonical_ids, confirmed_at_ms)
                .await?;
            for canonical in &canonical_ids {
                self.emit_message_update_at(room_id, canonical).await?;
            }
        }
        Ok(MessageHandle {
            message_id: event_id,
            sent_at_ms: confirmed_at_ms,
        })
    }

    /// Send a `Confirmation { type: DELIVERED }` for `message_ids` — the
    /// auto-acknowledgement a recipient emits when a message arrives. Step
    /// 25b. Unlike [`Self::send_read_confirmation`] there is **no** local
    /// self-apply: `delivered_to` is the *sender's* view (who confirmed
    /// delivery), and the confirming device is not itself a row.
    ///
    /// # Errors
    /// `InvalidInput` if `message_ids` is empty; otherwise whatever the MLS
    /// encrypt / upload surfaces.
    pub(crate) async fn send_delivered_confirmation(
        &self,
        room_id: &str,
        message_ids: &[String],
    ) -> Result<MessageHandle, KupferdrahtError> {
        debug_assert!(
            !room_id.is_empty(),
            "send_delivered_confirmation: room_id must be non-empty"
        );
        if message_ids.is_empty() {
            return Err(KupferdrahtError::InvalidInput(
                "send_delivered_confirmation: message_ids must be non-empty".into(),
            ));
        }
        let group_id = self
            .resolve_room_group_id(room_id, "send_delivered_confirmation")
            .await?;
        let (event_id, plaintext) =
            crate::wire::proto::build_delivered_confirmation_generic_message(message_ids)?;
        self.encrypt_and_upload(&group_id, "send_delivered_confirmation", &plaintext)
            .await?;
        Ok(MessageHandle {
            message_id: event_id,
            sent_at_ms: now_ms(),
        })
    }
}
