// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Ephemeral (self-deleting) text — Wire's `Ephemeral` wrapper
//! (`GenericMessage` field 18) around a `Text`. Step 89.
//!
//! Three responsibilities of one feature, kept together because they
//! share the timer contract:
//!
//! 1. **Send** — `Timeline::send_ephemeral_text` wraps the body with
//!    `expire_after_millis = ttl` and persists the own copy with
//!    `expires_at_ms = sent_at_ms + ttl`. The stamp is set only when
//!    the upload reached the backend (an official sender starts its
//!    countdown on the backend ack; a `Failed` row keeps its body
//!    and gets no timer).
//! 2. **Expiry** — the sweep [`Client::expire_due_ephemeral_messages`]
//!    tombstones every row whose stamp passed (body cleared,
//!    `redacted_at_ms` set — the `MessageDelete` tombstone, so reply
//!    chains keep resolving) and emits one `TimelineDiff::UpdateAt`
//!    per row. For an INBOUND row it then broadcasts the expiry as a
//!    delete-for-everyone (`MessageDelete` into the room, best-effort,
//!    V1b): live-observed on wire-webapp and confirmed in Kalium's
//!    protocol behaviour (observations §74), the RECEIVER enforces
//!    expiry network-wide while the SENDER only marks its own copy and
//!    waits for exactly that delete to purge it. An own row therefore
//!    stays local-only. The one official-client step Kupferdraht cannot
//!    mirror yet is the `DeleteForMe` into the self-conversation for
//!    the receiver's other devices (Step 86).
//! 3. **Timers** — a native `tokio` task per stamp
//!    ([`Client::arm_ephemeral_timer`]) fires the sweep on time for
//!    long-running consumers (push transport has no periodic drain);
//!    every dispatch cycle and every resume
//!    ([`Client::restore_ephemeral_timers`]) sweep as well, so a
//!    restart or a lost task never leaves an expired body readable.
//!    On wasm32 there is no task spawner — only the cycle sweeps run.
//!
//! **Timer start = dispatch/send time.** Live-verified against
//! wire-webapp's behaviour (observations §74): official
//! receivers start the countdown when the message scrolls into the
//! viewport, i.e. when it is actually viewed. A headless client has
//! no viewport, so Kupferdraht starts at dispatch — never LATER than an
//! official client would, which is the safe direction (Kupferdraht may
//! delete sooner, never shows a message longer). Documented in
//! `docs/wire/message-types.md` and `docs/library.md`.

use std::time::Duration;

use tracing::{debug, warn};

use crate::client::Client;
use crate::error::KupferdrahtError;
use crate::time::now_ms;
use crate::timeline::{DeliveryStatus, MessageHandle, Timeline};
use crate::wire::proto::build_ephemeral_text_generic_message;

/// Smallest TTL Kupferdraht accepts on the send side. Wire's official
/// clients bound the conversation timer to the same range
/// (1 s … 1 year, observations §74); a zero TTL would be a message
/// deleted before it can be read.
pub(crate) const EPHEMERAL_TTL_MIN: Duration = Duration::from_secs(1);
/// Largest TTL Kupferdraht accepts on the send side (365 days).
pub(crate) const EPHEMERAL_TTL_MAX: Duration = Duration::from_hours(365 * 24);
/// [`EPHEMERAL_TTL_MAX`] in milliseconds — the receive-side clamp for a
/// peer-chosen `expire_after_millis` (Step 95c).
pub(crate) const EPHEMERAL_TTL_MAX_MS: i64 = 365 * 24 * 60 * 60 * 1_000;

/// Range-check a caller-supplied TTL and convert it to Wire's
/// `expire_after_millis`.
///
/// # Errors
/// `KupferdrahtError::InvalidInput` outside [`EPHEMERAL_TTL_MIN`] ..=
/// [`EPHEMERAL_TTL_MAX`].
fn validate_ephemeral_ttl(ttl: Duration) -> Result<i64, KupferdrahtError> {
    if ttl < EPHEMERAL_TTL_MIN || ttl > EPHEMERAL_TTL_MAX {
        return Err(KupferdrahtError::InvalidInput(format!(
            "send_ephemeral_text: ttl must be between {}s and {} days, got {ttl:?}",
            EPHEMERAL_TTL_MIN.as_secs(),
            EPHEMERAL_TTL_MAX.as_secs() / 86_400
        )));
    }
    i64::try_from(ttl.as_millis()).map_err(|_| {
        KupferdrahtError::InvalidInput("send_ephemeral_text: ttl does not fit i64 millis".into())
    })
}

impl Timeline {
    /// Send a self-deleting text: the body travels inside Wire's
    /// `Ephemeral` wrapper with `ttl` as `expire_after_millis`, so
    /// official clients render a countdown and delete their copy
    /// locally; Kupferdraht's own copy self-redacts after `ttl` as well.
    /// `send_text` stays untouched — a plain text never expires.
    /// Step 89.
    ///
    /// The wrapper carries no mentions and no quote.
    ///
    /// # Errors
    /// * `KupferdrahtError::InvalidInput` — `ttl` outside 1 s … 365 days.
    /// * Otherwise the same downstream errors as [`Self::send_text`]
    ///   (group resolve, encode, MLS encrypt, transport, storage). On
    ///   an upload failure the row is persisted as `Failed` WITHOUT
    ///   an expiry stamp (the timer starts on the backend ack only).
    pub async fn send_ephemeral_text(
        &self,
        body: String,
        ttl: Duration,
    ) -> Result<MessageHandle, KupferdrahtError> {
        self.client
            .send_ephemeral_text(&self.room_id, body, ttl)
            .await
    }
}

impl Client {
    /// See [`Timeline::send_ephemeral_text`] for the caller-facing
    /// contract. Step 89.
    pub(crate) async fn send_ephemeral_text(
        &self,
        room_id: &str,
        body: String,
        ttl: Duration,
    ) -> Result<MessageHandle, KupferdrahtError> {
        debug_assert!(
            !room_id.is_empty(),
            "send_ephemeral_text: room_id must be non-empty"
        );
        let ttl_ms = validate_ephemeral_ttl(ttl)?;
        let group_id = self
            .resolve_room_group_id(room_id, "send_ephemeral_text")
            .await?;
        let (message_id, plaintext) = build_ephemeral_text_generic_message(&body, ttl_ms)?;
        // Step 97a: the row is written as `Sending` before the upload; the
        // expiry stamp and the timer ride only a delivered send (see the
        // module doc).
        let _in_flight = self.begin_send(&message_id);
        let sent_at_ms = now_ms();
        let persisted = self
            .persist_and_emit_outgoing_ephemeral_text(
                &message_id,
                room_id,
                &body,
                sent_at_ms,
                ttl_ms,
            )
            .await?;
        let upload = self
            .encrypt_and_upload(&group_id, "send_ephemeral_text", &plaintext)
            .await;
        // The timer counts from the send (`sent_at_ms`, Step 89's
        // contract: expires_at = sent_at + ttl); only a delivered send
        // gets it.
        let expires_at_ms = upload.is_ok().then(|| sent_at_ms.saturating_add(ttl_ms));
        self.finish_send(room_id, &message_id, upload.is_ok(), expires_at_ms)
            .await;
        if persisted && let Some(at) = expires_at_ms {
            self.arm_ephemeral_timer(at);
        }
        upload?;
        Ok(MessageHandle {
            message_id,
            sent_at_ms,
        })
    }

    /// Persist the own copy of an ephemeral text as `Sending`
    /// (`kind = 'text'`, the chosen TTL, no expiry stamp yet — the timer
    /// starts on the backend ack; the persisted TTL lets
    /// `Timeline::resend` re-wrap the row later, Step 79) and emit its
    /// `TimelineDiff::Append`. `false` without a store (metadata-only
    /// resume mode). Step 97a.
    async fn persist_and_emit_outgoing_ephemeral_text(
        &self,
        message_id: &str,
        room_id: &str,
        body: &str,
        sent_at_ms: i64,
        ttl_ms: i64,
    ) -> Result<bool, KupferdrahtError> {
        let Some(store) = self.store_handle() else {
            return Ok(false);
        };
        let mut event = self.outgoing_record(
            message_id,
            room_id,
            "text",
            body,
            sent_at_ms,
            DeliveryStatus::Sending,
        );
        event.ephemeral_ttl_ms = Some(ttl_ms);
        store.insert_timeline_event(&event).await?;
        self.emit_outgoing_text_append(event, &[]);
        Ok(true)
    }

    /// Tombstone every ephemeral row whose timer elapsed, surface each
    /// as a `TimelineDiff::UpdateAt` carrying the tombstone, and — for
    /// inbound rows — broadcast the expiry as a delete-for-everyone
    /// (V1b, best-effort). Idempotent and cheap (one indexed query;
    /// a row is retired exactly once, so the broadcast fires once) —
    /// safe to run at the end of every dispatch cycle, from every timer
    /// task, and on resume. Returns the number of rows retired.
    /// Step 89.
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` on the sweep or the re-read. A failed
    /// broadcast is logged, never propagated (the local tombstone is
    /// the contract, the network delete the courtesy toward the sender).
    pub(crate) async fn expire_due_ephemeral_messages(&self) -> Result<u32, KupferdrahtError> {
        let Some(store) = self.store_handle() else {
            return Ok(0);
        };
        let now = now_ms();
        // Step 95c: the device clock is environment data — a clock
        // before the epoch skips the sweep instead of asserting.
        if now <= 0 {
            tracing::warn!("ephemeral sweep skipped: the wall clock is before the epoch");
            return Ok(0);
        }
        let retired = store.expire_ephemeral_messages(now).await?;
        let own_user_id = self.own_qualified_user_id();
        for row in &retired {
            debug!(room_id = %row.room_id, message_id = %row.message_id, "ephemeral expired");
            self.emit_message_update_at(&row.room_id, &row.message_id)
                .await?;
            if row.sender_user_id != own_user_id {
                self.broadcast_expiry_delete(&row.room_id, &row.message_id)
                    .await;
            }
        }
        Ok(u32::try_from(retired.len()).unwrap_or(u32::MAX))
    }

    /// V1b: tell the sender (and everyone else in the room) that this
    /// inbound ephemeral message expired here — the `MessageDelete`
    /// official senders wait for before purging their own copy. Same
    /// envelope as `Timeline::redact`, minus the local apply (the row
    /// is already tombstoned). Best-effort like the Step-25b delivery
    /// receipt: a rejected upload is logged and not retried; the stale
    /// -epoch case self-heals once inside `encrypt_and_upload`.
    async fn broadcast_expiry_delete(&self, room_id: &str, message_id: &str) {
        debug_assert!(
            !message_id.is_empty(),
            "broadcast_expiry_delete: message_id must be non-empty"
        );
        match self.upload_message_delete(room_id, message_id).await {
            Ok((delete_event_id, _)) => {
                debug!(
                    room_id,
                    message_id, delete_event_id, "expiry delete-for-everyone sent"
                );
            }
            Err(e) => {
                warn!(
                    room_id,
                    message_id,
                    error = %e,
                    "expiry delete-for-everyone failed (local tombstone kept)"
                );
            }
        }
    }

    /// Resume-time half of the timer contract: sweep whatever expired
    /// while the process was down, then arm one timer per pending
    /// expiry instant so a long-running consumer without a periodic
    /// drain (push transport) still redacts on time. Called from
    /// `Client::load_rooms_from_store`. Step 89.
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` on the sweep or the pending scan.
    pub(crate) async fn restore_ephemeral_timers(&self) -> Result<(), KupferdrahtError> {
        let Some(store) = self.store_handle() else {
            return Ok(());
        };
        self.expire_due_ephemeral_messages().await?;
        let pending = store.pending_ephemeral_expiries(now_ms()).await?;
        for at in pending {
            self.arm_ephemeral_timer(at);
        }
        Ok(())
    }

    /// Spawn a task that sleeps until `expires_at_ms` and then runs
    /// the sweep. Each stamp gets its own task (they are tiny and the
    /// sweep is idempotent, so duplicates are harmless). Without a
    /// tokio runtime on the current thread the timer is skipped — the
    /// dispatch-cycle and resume sweeps remain. Step 89.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn arm_ephemeral_timer(&self, expires_at_ms: i64) {
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            debug!(
                expires_at_ms,
                "no tokio runtime — ephemeral timer not armed"
            );
            return;
        };
        let delay = Duration::from_millis(u64::try_from(expires_at_ms - now_ms()).unwrap_or(0));
        let weak = self.weak();
        handle.spawn(async move {
            tokio::time::sleep(delay).await;
            let Some(client) = weak.upgrade() else {
                return;
            };
            if let Err(e) = client.expire_due_ephemeral_messages().await {
                warn!(error = %e, "ephemeral expiry sweep failed");
            }
        });
    }

    /// wasm32 has no task spawner (`tokio::spawn` is cfg'd out, W4):
    /// only the dispatch-cycle and resume sweeps expire rows there.
    // `&self` mirrors the native twin's signature (call sites are identical).
    #[cfg(target_arch = "wasm32")]
    #[allow(clippy::unused_self)]
    pub(crate) fn arm_ephemeral_timer(&self, expires_at_ms: i64) {
        debug!(expires_at_ms, "ephemeral timers are sweep-only on wasm32");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_ephemeral_ttl_accepts_the_wire_range_inclusive() {
        assert_eq!(
            validate_ephemeral_ttl(EPHEMERAL_TTL_MIN).expect("min"),
            1_000
        );
        assert_eq!(
            validate_ephemeral_ttl(EPHEMERAL_TTL_MAX).expect("max"),
            365 * 24 * 60 * 60 * 1_000
        );
        assert_eq!(
            validate_ephemeral_ttl(Duration::from_secs(300)).expect("5 min"),
            300_000
        );
    }

    #[test]
    fn the_millisecond_bound_matches_the_duration_bound() {
        assert_eq!(
            u128::try_from(EPHEMERAL_TTL_MAX_MS).expect("positive"),
            EPHEMERAL_TTL_MAX.as_millis()
        );
    }

    #[test]
    fn validate_ephemeral_ttl_rejects_outside_the_range() {
        for ttl in [
            Duration::ZERO,
            Duration::from_millis(999),
            EPHEMERAL_TTL_MAX + Duration::from_secs(1),
        ] {
            let err = validate_ephemeral_ttl(ttl).expect_err("out of range");
            assert!(
                matches!(&err, KupferdrahtError::InvalidInput(msg) if msg.contains("ttl")),
                "{ttl:?}: {err:?}"
            );
        }
    }
}
