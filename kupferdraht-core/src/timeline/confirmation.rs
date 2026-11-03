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

/// Whether a read is confirmed to the room (Step 138). matrix-rust-sdk
/// analog: `ReceiptType::Read` / `ReceiptType::ReadPrivate`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ReceiptType {
    /// Move the device-local read marker and send
    /// `Confirmation { type: READ }` — unless the account's receipt mode
    /// is off (ADR-0012).
    Read,
    /// Move the device-local read marker only; the room learns nothing.
    ReadPrivate,
}

impl Timeline {
    /// Mark a message as read (Steps 6-II-f, 138): the device-local read
    /// position moves to `item_id` (forward only — Step 67) and the
    /// room's hand-set unread flag clears, whatever `receipt` says. With
    /// [`ReceiptType::Read`] a `Confirmation { type: READ,
    /// first_message_id: item_id }` leaves for the room (one id per
    /// call); receivers stamp `(message_id, reader_user_id)` into
    /// `read_receipts` and Wire's apps show a "read" indicator. With
    /// [`ReceiptType::ReadPrivate`] nothing leaves: the room learns
    /// nothing, and the result is always `None`.
    ///
    /// **Step 10f gate**: with the account's `WIRE_RECEIPT_MODE` off a
    /// `Read` sends no `Confirmation` either and returns `Ok(None)` — the
    /// other Wire clients respect the same setting, so peers expect no
    /// receipt (ADR-0012 §Decision, `Client::receipt_mode`).
    ///
    /// An item unknown locally anchors no position: the marker stays,
    /// the flag still clears — the caller's "this room is read" holds.
    /// The position and the flag are device-local; the cross-device read
    /// position is a later step.
    ///
    /// matrix-rust-sdk analog: `Timeline::send_single_receipt`.
    ///
    /// # Errors
    /// Same as [`Self::send_text`]; additionally `InvalidInput` when
    /// `item_id` is empty.
    pub async fn mark_as_read(
        &self,
        item_id: String,
        receipt: ReceiptType,
    ) -> Result<Option<MessageHandle>, KupferdrahtError> {
        if item_id.is_empty() {
            return Err(KupferdrahtError::InvalidInput(
                "mark_as_read: item_id must be non-empty".into(),
            ));
        }
        // The local read position first — the receipt gate below governs
        // the outbound wire signal, not the position (Step 67).
        self.advance_read_position(&item_id).await?;
        if receipt == ReceiptType::ReadPrivate
            || matches!(
                self.client.receipt_mode().await?,
                crate::client::ReceiptMode::Off
            )
        {
            return Ok(None);
        }
        self.client
            .send_read_confirmation(&self.room_id, &[item_id])
            .await
            .map(Some)
    }

    /// Move the device-local marker to `item_id`'s position when the item
    /// is known locally (the store's upsert keeps it forward-only and
    /// clears the flag), else clear the flag alone (Step 138).
    async fn advance_read_position(&self, item_id: &str) -> Result<(), KupferdrahtError> {
        let Some(store) = self.client.store_handle() else {
            return Ok(());
        };
        match self
            .client
            .lookup_timeline_event(&self.room_id, item_id)
            .await?
        {
            Some(record) => {
                store
                    .set_read_marker(&self.room_id, record.sent_at_ms)
                    .await
            }
            None => store.set_unread_flag(&self.room_id, false).await,
        }
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
