// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Delete-for-everyone (`MessageDelete` `GenericMessage`) —
//! Step 6-II-d, split out of the former monolithic
//! `timeline/send.rs` by Step 30.
//!
//! Receivers tombstone the row: the body is hidden but the row stays
//! so reply chains remain renderable as "(quoted message deleted)".
//! A file goes entirely: its key's row, its cached copy and — for an own
//! delete — its upload at the server (Step 84).

use tracing::warn;

use crate::time::now_ms;

use crate::client::Client;
use crate::error::KupferdrahtError;
use crate::storage::AttachmentRecord;
use crate::timeline::{MessageHandle, Timeline};

impl Timeline {
    /// Delete a previously-sent message ("delete-for-everyone",
    /// Step 6-II-d). `item_id` is the original `message_id` (UUID)
    /// of the message being retired. Wire-side emits a
    /// `MessageDelete { message_id: item_id }` inside a fresh
    /// `GenericMessage`. Receivers tombstone the row; the body is
    /// hidden but the row stays so reply chains remain renderable
    /// as "(quoted message deleted)". A file goes entirely: its key,
    /// its cached copy and its upload at the server, the last best
    /// effort (Step 84).
    ///
    /// matrix-rust-sdk analog: `Timeline::redact`.
    ///
    /// # Errors
    /// Same as [`Self::send_text`]; additionally `InvalidInput` when
    /// `item_id` is empty.
    pub async fn redact(&self, item_id: &str) -> Result<MessageHandle, KupferdrahtError> {
        if item_id.is_empty() {
            return Err(KupferdrahtError::InvalidInput(
                "redact: item_id must be non-empty".into(),
            ));
        }
        self.client.redact_message(&self.room_id, item_id).await
    }
}

impl Client {
    /// See [`Timeline::redact`] for the caller-facing contract.
    pub(crate) async fn redact_message(
        &self,
        room_id: &str,
        target_message_id: &str,
    ) -> Result<MessageHandle, KupferdrahtError> {
        // Step 95b (ADR-0028): delete-for-everyone of another user's
        // message is rejected by every receiver — refuse it here. The
        // receiver-side disappearing expiry calls `upload_message_delete`
        // directly and is not affected.
        self.ensure_own_target(room_id, target_message_id, "delete")
            .await?;
        let (delete_event_id, redacted_at_ms) = self
            .upload_message_delete(room_id, target_message_id)
            .await?;
        // Step 84: the file's locator is read before the delete takes its row.
        let upload = self.attachment_of(room_id, target_message_id).await?;
        if self
            .apply_redaction(room_id, target_message_id, redacted_at_ms)
            .await?
            && let Some(upload) = upload
        {
            self.delete_own_upload(&upload).await;
        }
        Ok(MessageHandle {
            message_id: delete_event_id,
            sent_at_ms: redacted_at_ms,
        })
    }

    /// The local half of a delete for everyone, own or received: tombstone
    /// the message and forget its file — the row in the same transaction,
    /// the cached copy after it (Step 84) — then show the change. `false`
    /// when nothing applied (no store, no such message, a later delete).
    pub(crate) async fn apply_redaction(
        &self,
        room_id: &str,
        target_message_id: &str,
        redacted_at_ms: i64,
    ) -> Result<bool, KupferdrahtError> {
        let Some(store) = self.store_handle() else {
            return Ok(false);
        };
        if !store
            .apply_message_delete(room_id, target_message_id, redacted_at_ms)
            .await?
        {
            return Ok(false);
        }
        if let Some(canonical) = self
            .canonical_message_id(room_id, target_message_id)
            .await?
        {
            #[cfg(not(target_arch = "wasm32"))]
            self.remove_cached_file(&canonical);
            self.emit_message_update_at(room_id, &canonical).await?;
        }
        // Step 60c: a redacted task message drops out of the fold —
        // rebuild the room's materialized task cache.
        crate::timeline::task::rebuild_tasks_if_redacted_task(store, room_id, target_message_id)
            .await?;
        Ok(true)
    }

    /// The attachment row of the message `target_message_id` names — `None`
    /// for a message without a file.
    async fn attachment_of(
        &self,
        room_id: &str,
        target_message_id: &str,
    ) -> Result<Option<AttachmentRecord>, KupferdrahtError> {
        let Some(store) = self.store_handle() else {
            return Ok(None);
        };
        let Some(canonical) = self
            .canonical_message_id(room_id, target_message_id)
            .await?
        else {
            return Ok(None);
        };
        store.get_attachment(&canonical, room_id).await
    }

    /// Remove an own upload at the server once its message was deleted for
    /// everyone, as Wire's Android client does (`kalium-readings.md` #29).
    /// Best effort: a failure is logged and the delete stands (Step 84).
    async fn delete_own_upload(&self, upload: &AttachmentRecord) {
        let Some(access_token) = self.access_token() else {
            warn!(
                attachment_id = upload.attachment_id,
                "own upload kept: no access token"
            );
            return;
        };
        if let Err(e) = self
            .http_delete_attachment(
                &access_token,
                &upload.attachment_domain,
                &upload.attachment_id,
            )
            .await
        {
            warn!(attachment_id = upload.attachment_id, error = %e, "own upload not deleted at the server");
        }
    }

    /// The network half of a delete-for-everyone: build the
    /// `MessageDelete { message_id: target }` envelope, encrypt it under
    /// the room's group, and upload it. Returns the delete event's own
    /// id and the wall-clock of the upload. No local mutation — shared
    /// by [`Self::redact_message`] (which then tombstones locally) and
    /// the Step-89 expiry broadcast (whose row is already tombstoned).
    pub(crate) async fn upload_message_delete(
        &self,
        room_id: &str,
        target_message_id: &str,
    ) -> Result<(String, i64), KupferdrahtError> {
        debug_assert!(
            !target_message_id.is_empty(),
            "upload_message_delete: target must be non-empty"
        );
        let group_id = self.resolve_room_group_id(room_id, "redact").await?;
        let (delete_event_id, plaintext) =
            crate::wire::proto::build_message_delete_generic_message(target_message_id)?;
        self.encrypt_and_upload(&group_id, "redact", &plaintext)
            .await?;
        Ok((delete_event_id, now_ms()))
    }
}
