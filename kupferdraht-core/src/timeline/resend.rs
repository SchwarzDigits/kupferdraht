// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Failed-send recovery — Step 79.
//!
//! A `Failed` row (Step 25a) is the wedged local echo of a send that
//! never reached the backend. `Timeline::resend` re-drives one such row
//! from its persisted state under the SAME `message_id` — the
//! matrix-rust-sdk `SendHandle::unwedge` shape: the transaction id
//! survives the retry, and if the first attempt did reach the backend
//! behind a 5xx proxy, receivers dedupe on the id instead of showing
//! the message twice. `Room::failed_sends` enumerates the rows
//! oldest-first; `Timeline::resend_failed` re-drives them in that order
//! and stops at the first hard error so a stuck message is never
//! overtaken (matrix's strict-order rule); rows behind the stop stay
//! `Failed` and the next pass picks them up.
//!
//! No scheduler lives in the core — the consumer calls these when its
//! sync loop comes back (docs/library.md "Reconnect contract"). The
//! per-kind payload reconstruction lives in `super::resend_payload`.

use tracing::{debug, warn};

use crate::client::Client;
use crate::error::KupferdrahtError;
use crate::storage::TimelineEventRecord;
use crate::time::now_ms;
use crate::timeline::items::DeliveryStatus;
use crate::timeline::resend_payload::snapshot_recovery_hint;
use crate::timeline::{MessageHandle, MessagesOptions, Timeline, TimelineItem};

/// Outcome of one [`Timeline::resend_failed`] pass. Step 79.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResendSummary {
    /// Rows re-driven successfully, oldest first.
    pub resent: Vec<MessageHandle>,
    /// Failed rows `resend` does not cover (task / case / marker
    /// snapshots — re-send the current state instead); left `Failed`.
    pub skipped: Vec<String>,
    /// `(message_id, error)` of the hard error the pass stopped at;
    /// `None` when every attempted row went through.
    pub stopped_at: Option<(String, String)>,
    /// Failed rows behind `stopped_at` that were not attempted — still
    /// `Failed`, the next pass picks them up (snapshot rows included;
    /// that pass classifies them).
    pub remaining: u32,
}

impl Timeline {
    /// Re-drive one failed send from its persisted row under the same
    /// `message_id`: text (reply and mentions included), ephemeral text
    /// (re-wrapped with the original TTL, timer armed on success),
    /// knock, location, and asset (the blob already sits on the
    /// backend — only the MLS reference is re-sent). Re-encrypts at the
    /// current epoch, uploads once (the stale-epoch self-heal applies),
    /// flips the row to `Submitted`, re-stamps `sent_at_ms`, and emits
    /// `TimelineDiff::UpdateAt`. matrix-rust-sdk analog:
    /// `SendHandle::unwedge`. Step 79.
    ///
    /// # Errors
    /// * `InvalidInput` — unknown id, the row is not a failed send, was
    ///   deleted, or is a task / case / marker snapshot (re-send the
    ///   current state via `update_task` / `set_case` / `set_task_list`
    ///   instead); a reply whose parent is no longer stored locally.
    /// * Transport / MLS / storage errors propagate; the row then stays
    ///   `Failed` for the next attempt.
    pub async fn resend(&self, message_id: String) -> Result<MessageHandle, KupferdrahtError> {
        if message_id.is_empty() {
            return Err(KupferdrahtError::InvalidInput(
                "resend: message_id must be non-empty".into(),
            ));
        }
        self.client.resend_message(&self.room_id, &message_id).await
    }

    /// Re-drive every failed send of this room, oldest first, stopping
    /// at the first hard error (a stuck message must not be overtaken);
    /// snapshot rows are skipped, rows behind the stop stay `Failed`.
    /// Never errors as a whole once the rows could be read — the
    /// outcome is the [`ResendSummary`]. matrix-rust-sdk analog:
    /// `SendQueue::respawn_tasks_for_rooms_with_unsent_requests`,
    /// scoped to one room. Step 79.
    ///
    /// # Errors
    /// `InvalidInput` without a store; `Storage` when the failed rows
    /// cannot be enumerated.
    pub async fn resend_failed(&self) -> Result<ResendSummary, KupferdrahtError> {
        self.client.resend_failed_messages(&self.room_id).await
    }
}

impl Client {
    /// The `Failed` rows of `room_id` as fully-overlaid items, oldest
    /// first — what `Room::failed_sends` returns. Step 79.
    pub(crate) async fn failed_sends(
        &self,
        room_id: &str,
    ) -> Result<Vec<TimelineItem>, KupferdrahtError> {
        debug_assert!(
            !room_id.is_empty(),
            "failed_sends: room_id must be non-empty"
        );
        let items = self.messages(room_id, &MessagesOptions::all()).await?;
        // Step 107: a row being resent right now is not a failed send —
        // listing it would invite a second, concurrent resend.
        Ok(items
            .into_iter()
            .filter(|item| {
                item.delivery_status == DeliveryStatus::Failed
                    && !item.redacted
                    && !self.is_in_flight(&item.item_id)
            })
            .collect())
    }

    /// See [`Timeline::resend`] for the caller-facing contract.
    pub(crate) async fn resend_message(
        &self,
        room_id: &str,
        message_id: &str,
    ) -> Result<MessageHandle, KupferdrahtError> {
        let row = self.load_failed_row(room_id, message_id).await?;
        self.resend_row(&row).await
    }

    /// See [`Timeline::resend_failed`] for the caller-facing contract.
    pub(crate) async fn resend_failed_messages(
        &self,
        room_id: &str,
    ) -> Result<ResendSummary, KupferdrahtError> {
        let rows = self.failed_rows(room_id).await?;
        let total = rows.len();
        let mut summary = ResendSummary::default();
        for (index, row) in rows.iter().enumerate() {
            if snapshot_recovery_hint(&row.kind).is_some() {
                summary.skipped.push(row.message_id.clone());
                continue;
            }
            match self.resend_row(row).await {
                Ok(handle) => summary.resent.push(handle),
                Err(error) => {
                    warn!(room_id, message_id = %row.message_id, %error, "resend pass stopped");
                    summary.stopped_at = Some((row.message_id.clone(), error.to_string()));
                    summary.remaining = u32::try_from(total - index - 1).unwrap_or(u32::MAX);
                    break;
                }
            }
        }
        debug_assert!(
            summary.resent.len()
                + summary.skipped.len()
                + usize::from(summary.stopped_at.is_some())
                + summary.remaining as usize
                == total,
            "resend_failed: every failed row is accounted for exactly once"
        );
        Ok(summary)
    }

    /// The raw `failed` rows of `room_id`, oldest first (no overlays —
    /// the pass only needs kind, body, and ids).
    async fn failed_rows(
        &self,
        room_id: &str,
    ) -> Result<Vec<TimelineEventRecord>, KupferdrahtError> {
        let store = self.store_handle().ok_or_else(|| {
            KupferdrahtError::InvalidInput(
                "resend_failed: no Store attached — set ClientBuilder::base_dir(...) first".into(),
            )
        })?;
        let rows = store.load_timeline_events(room_id).await?;
        Ok(rows
            .into_iter()
            .filter(|row| self.is_resendable(row))
            .collect())
    }

    /// Load the row `message_id` names and require it to be a live
    /// failed send. Every other state is the caller's mistake.
    async fn load_failed_row(
        &self,
        room_id: &str,
        message_id: &str,
    ) -> Result<TimelineEventRecord, KupferdrahtError> {
        let row = self
            .lookup_timeline_event(room_id, message_id)
            .await?
            .ok_or_else(|| {
                KupferdrahtError::InvalidInput(format!(
                    "resend: {message_id} not found in {room_id}"
                ))
            })?;
        if row.redacted_at_ms.is_some() {
            return Err(KupferdrahtError::InvalidInput(format!(
                "resend: {message_id} was deleted — nothing to resend"
            )));
        }
        if !self.is_resendable(&row) {
            return Err(KupferdrahtError::InvalidInput(format!(
                "resend: {message_id} is not a failed send (delivery_status = {})",
                row.delivery_status
            )));
        }
        Ok(row)
    }

    /// The resend spine: rebuild the plaintext under the persisted id,
    /// encrypt + upload once, flip the row, re-arm an ephemeral timer,
    /// tell subscribers. A failed upload leaves the row untouched.
    async fn resend_row(
        &self,
        row: &TimelineEventRecord,
    ) -> Result<MessageHandle, KupferdrahtError> {
        debug_assert!(
            self.is_resendable(row),
            "resend_row: caller checked the row"
        );
        let room_id = row.room_id.as_str();
        let message_id = row.message_id.as_str();
        // Step 107: in flight for the whole resend — `failed_sends` leaves
        // the row out, and a concurrent resend of it is refused.
        let _in_flight = self.try_begin_send(message_id).ok_or_else(|| {
            KupferdrahtError::InvalidInput(format!("resend: {message_id} is already being sent"))
        })?;
        let payload = self.rebuild_failed_payload(row).await?;
        let group_id = self.resolve_room_group_id(room_id, "resend").await?;
        self.encrypt_and_upload(&group_id, "resend", &payload.plaintext)
            .await?;
        let sent_at_ms = now_ms();
        let expires_at_ms = payload.ttl_ms.map(|ttl| sent_at_ms + ttl);
        self.flip_resent_row(room_id, message_id, sent_at_ms, expires_at_ms)
            .await?;
        if let Some(at) = expires_at_ms {
            self.arm_ephemeral_timer(at);
        }
        self.emit_message_update_at(room_id, message_id).await?;
        debug!(room_id, message_id, kind = %row.kind, "failed send re-driven");
        Ok(MessageHandle {
            message_id: message_id.to_string(),
            sent_at_ms,
        })
    }

    /// Flip the uploaded row from `failed` to `submitted`. The upload
    /// already happened, so a store that does not flip the row is a
    /// storage defect surfaced loudly — never a silent "sent" that the
    /// next pass would upload again.
    async fn flip_resent_row(
        &self,
        room_id: &str,
        message_id: &str,
        sent_at_ms: i64,
        expires_at_ms: Option<i64>,
    ) -> Result<(), KupferdrahtError> {
        let store = self.store_handle().ok_or_else(|| {
            KupferdrahtError::Storage(format!(
                "resend: {message_id} uploaded but no Store is attached"
            ))
        })?;
        let flipped = store
            .mark_message_resent(room_id, message_id, sent_at_ms, expires_at_ms)
            .await?;
        if !flipped {
            return Err(KupferdrahtError::Storage(format!(
                "resend: {message_id} uploaded but the Store did not flip the row \
                 (Store::mark_message_resent must be implemented)"
            )));
        }
        Ok(())
    }
}
