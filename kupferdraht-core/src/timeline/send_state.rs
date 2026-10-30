// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 97a — the lifecycle of an own send's row (review blocker B7).
//!
//! A row-producing send persists its row as `DeliveryStatus::Sending`
//! **before** the upload and settles it afterwards (`submitted` /
//! `failed`). While the upload runs, the message id sits in the
//! `Client`'s in-flight set; the [`InFlightSend`] guard removes it when
//! the send ends — also when its future is dropped (a Kotlin coroutine
//! cancel). A `sending` row whose id is not in the set was abandoned
//! (cancelled, or the process restarted): it reads as `Failed` and is
//! resendable under the same message id (Step 79). Nothing is written
//! at startup and no other process's row is flipped.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, PoisonError};

use tracing::warn;

use crate::client::Client;
use crate::error::KupferdrahtError;
use crate::storage::TimelineEventRecord;
use crate::timeline::{DeliveryStatus, TimelineItem};

/// The message ids of the own sends whose upload is in flight in this
/// process.
#[derive(Default, Clone)]
pub(crate) struct InFlightSends(Arc<Mutex<HashSet<String>>>);

impl InFlightSends {
    /// Mark `message_id` in flight; `false` when it already was.
    fn insert(&self, message_id: &str) -> bool {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(message_id.to_string())
    }

    fn remove(&self, message_id: &str) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(message_id);
    }

    fn contains(&self, message_id: &str) -> bool {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(message_id)
    }
}

/// One non-text content send for [`Client::send_content_row`]: the wire
/// message and the row it is persisted as.
pub(crate) struct ContentRow<'a> {
    pub(crate) message_id: &'a str,
    pub(crate) plaintext: &'a [u8],
    /// `timeline_events.kind` (`knock`, `location`, `case`, …).
    pub(crate) kind: &'a str,
    /// The packed body `TimelineItem::from_record` decodes.
    pub(crate) body: &'a str,
}

/// Marks one send as in flight until dropped — at the end of the send,
/// or when its future is cancelled. `Drop` does no async work, so it is
/// safe on every runtime (wasm included).
pub(crate) struct InFlightSend {
    sends: InFlightSends,
    message_id: String,
}

impl Drop for InFlightSend {
    fn drop(&mut self) {
        self.sends.remove(&self.message_id);
    }
}

impl Client {
    /// Mark `message_id` as in flight until the returned guard drops.
    /// Taken before the row is written, so a reader never sees the
    /// fresh `sending` row as abandoned.
    pub(crate) fn begin_send(&self, message_id: &str) -> InFlightSend {
        debug_assert!(!message_id.is_empty(), "begin_send: message_id must be set");
        let sends = self.in_flight_sends().clone();
        let fresh = sends.insert(message_id);
        debug_assert!(fresh, "begin_send: a fresh message id is not in flight yet");
        InFlightSend {
            sends,
            message_id: message_id.to_string(),
        }
    }

    /// [`Self::begin_send`] for a row that may already be in flight — a
    /// resend (Step 107): `None` when this `Client` is sending it already.
    pub(crate) fn try_begin_send(&self, message_id: &str) -> Option<InFlightSend> {
        debug_assert!(
            !message_id.is_empty(),
            "try_begin_send: message_id must be set"
        );
        let sends = self.in_flight_sends().clone();
        sends.insert(message_id).then(|| InFlightSend {
            sends,
            message_id: message_id.to_string(),
        })
    }

    /// The status a reader sees: a `Sending` row whose send is no longer
    /// in flight on this `Client` reads as `Failed`. The in-flight set is
    /// per `Client` — another `Client` on the same store sees a live send
    /// as `Failed` (documented in `library.md`, "Send lifecycle").
    pub(crate) fn settled_status(
        &self,
        status: DeliveryStatus,
        message_id: &str,
    ) -> DeliveryStatus {
        if status == DeliveryStatus::Sending && !self.is_in_flight(message_id) {
            DeliveryStatus::Failed
        } else {
            status
        }
    }

    /// Whether this `Client` is sending (or resending) `message_id`.
    pub(crate) fn is_in_flight(&self, message_id: &str) -> bool {
        self.in_flight_sends().contains(message_id)
    }

    /// Apply [`Self::settled_status`] to every item of a read.
    pub(crate) fn settle_items(&self, items: &mut [TimelineItem]) {
        for item in items {
            item.delivery_status = self.settled_status(item.delivery_status, &item.item_id);
        }
    }

    /// Whether `row` is a failed send `resend` may re-drive (see
    /// [`is_resendable_row`]).
    pub(crate) fn is_resendable(&self, row: &TimelineEventRecord) -> bool {
        is_resendable_row(row, self.in_flight_sends().contains(&row.message_id))
    }

    /// The persist-first send of a non-text content row (knock,
    /// location, case snapshot, task-list marker): write the row as
    /// `Sending` and emit it, upload under `op`, settle. Returns the
    /// row's `sent_at_ms` on a delivered send; the upload's error
    /// otherwise (the row then reads as `Failed`).
    ///
    /// # Errors
    /// Storage errors writing the row (nothing was uploaded), or the
    /// upload's error.
    pub(crate) async fn send_content_row(
        &self,
        room_id: &str,
        group_id: &[u8],
        op: &'static str,
        row: ContentRow<'_>,
    ) -> Result<i64, KupferdrahtError> {
        let _in_flight = self.begin_send(row.message_id);
        let sent_at_ms = crate::time::now_ms();
        self.persist_and_emit_outgoing_content(
            row.message_id,
            room_id,
            row.kind,
            row.body,
            sent_at_ms,
            DeliveryStatus::Sending,
        )
        .await?;
        let upload = self.encrypt_and_upload(group_id, op, row.plaintext).await;
        self.finish_send(room_id, row.message_id, upload.is_ok(), None)
            .await;
        upload.map(|()| sent_at_ms)
    }

    /// Settle a send after its upload: `sending` → `submitted`
    /// (`delivered`, with `expires_at_ms` for a delivered ephemeral) or
    /// `failed`, then tell subscribers. Never an error: the upload's
    /// outcome is what the caller must see — a failed status write
    /// leaves the row `sending`, which reads as `Failed` once the send is
    /// over, and a resend reuses the id (receivers dedupe on it).
    pub(crate) async fn finish_send(
        &self,
        room_id: &str,
        message_id: &str,
        delivered: bool,
        expires_at_ms: Option<i64>,
    ) {
        debug_assert!(
            !room_id.is_empty() && !message_id.is_empty(),
            "finish_send: room_id and message_id must be set"
        );
        let Some(store) = self.store_handle() else {
            return;
        };
        match store
            .mark_send_outcome(room_id, message_id, delivered, expires_at_ms)
            .await
        {
            Ok(true) => {}
            Ok(false) => warn_if_unsettled(store.as_ref(), room_id, message_id, delivered).await,
            Err(e) => {
                warn!(room_id, message_id, delivered, error = %e,
                    "send settled on the wire but its status could not be written; the row reads as failed");
                return;
            }
        }
        if let Err(e) = self.emit_message_update_at(room_id, message_id).await {
            warn!(room_id, message_id, error = %e, "send settled but its update could not be emitted");
        }
    }
}

/// `mark_send_outcome` changed no row. Fine when the echo already made it
/// `received`; otherwise the store did not record the outcome — a custom
/// store without the method, or a lost row — and the send reads as failed
/// (Step 107, after the 97a review).
async fn warn_if_unsettled(
    store: &dyn crate::storage::Store,
    room_id: &str,
    message_id: &str,
    delivered: bool,
) {
    let status = match store.load_timeline_event(message_id).await {
        Ok(Some(row)) => row.delivery_status,
        Ok(None) => "missing".to_string(),
        Err(e) => format!("unreadable: {e}"),
    };
    if status != DeliveryStatus::Received.as_db_str() {
        warn!(room_id, message_id, delivered, %status,
            "send settled on the wire but the store recorded no outcome; the row reads as failed");
    }
}

/// A failed send `resend` may re-drive: a live row that is `failed`, or
/// `sending` while its send is not in flight (abandoned). Unknown
/// statuses are never resent.
fn is_resendable_row(row: &TimelineEventRecord, in_flight: bool) -> bool {
    // A row whose send is in flight — a first send or a resend (Step
    // 107) — is being sent, never a failed send.
    !in_flight
        && row.redacted_at_ms.is_none()
        && match DeliveryStatus::from_db_str(&row.delivery_status) {
            Some(DeliveryStatus::Failed) => true,
            Some(DeliveryStatus::Sending) => !in_flight,
            _ => false,
        }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(status: &str, redacted: bool) -> TimelineEventRecord {
        TimelineEventRecord {
            message_id: "m".into(),
            room_id: "r".into(),
            delivery_status: status.into(),
            redacted_at_ms: redacted.then_some(1),
            ..TimelineEventRecord::default()
        }
    }

    #[test]
    fn only_live_failed_or_abandoned_rows_are_resendable() {
        assert!(is_resendable_row(&row("failed", false), false));
        assert!(
            is_resendable_row(&row("sending", false), false),
            "an abandoned sending row is a failed send"
        );
        assert!(
            !is_resendable_row(&row("sending", false), true),
            "a send in flight is not resent"
        );
        assert!(
            !is_resendable_row(&row("failed", false), true),
            "a resend in flight is not resent again"
        );
        assert!(
            !is_resendable_row(&row("failed", true), false),
            "a deleted row is gone"
        );
        assert!(!is_resendable_row(&row("submitted", false), false));
        assert!(!is_resendable_row(&row("received", false), false));
        assert!(
            !is_resendable_row(&row("garbage", false), false),
            "unknown status is never resent"
        );
    }
}
