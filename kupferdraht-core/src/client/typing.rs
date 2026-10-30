// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `TypingTracker` — a `Client`'s ephemeral typing-indicator state
//! (Step 21).
//!
//! Two independent, in-memory halves — neither is ever persisted:
//!
//! - **Incoming** — a per-room set of the users currently typing,
//!   driven by inbound `conversation.typing` started/stopped events.
//!   Wire sends discrete signals; this set is the "who is typing now"
//!   view Kupferdraht derives client-side and hands to
//!   [`EventHandler::on_typing`](crate::sync::EventHandler::on_typing).
//! - **Outgoing** — a dedupe set of rooms with an in-flight typing
//!   notification. `begin_outgoing` guards against re-sending `started`
//!   while one is active; `end_outgoing` guarantees exactly one
//!   `stopped` is emitted whether it comes from the caller or the
//!   11-second auto-stop.
//!
//! The tracker is pure and synchronous. The `Client` wraps it with the
//! HTTP send, the auto-stop timer, and observer notification.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex as StdMutex;

use crate::error::KupferdrahtError;

use super::Client;

/// In-memory typing state for one `Client`. Cheap to default-construct;
/// every method takes `&self` and locks only briefly.
#[derive(Default)]
pub(crate) struct TypingTracker {
    incoming: StdMutex<HashMap<String, HashSet<String>>>,
    outgoing: StdMutex<HashSet<String>>,
}

impl TypingTracker {
    /// Apply an inbound started/stopped for `user_id` in `room_id` and
    /// return the room's resulting typing-user set (sorted, qualified
    /// ids). A room emptied by a `stopped` is pruned from the map so it
    /// does not leak memory.
    pub(crate) fn record_incoming(
        &self,
        room_id: &str,
        user_id: &str,
        started: bool,
    ) -> Vec<String> {
        debug_assert!(!room_id.is_empty(), "room_id must be non-empty");
        debug_assert!(!user_id.is_empty(), "user_id must be non-empty");
        let mut map = self
            .incoming
            .lock()
            .expect("typing incoming mutex poisoned");
        let set = map.entry(room_id.to_string()).or_default();
        if started {
            set.insert(user_id.to_string());
        } else {
            set.remove(user_id);
        }
        let mut users: Vec<String> = set.iter().cloned().collect();
        let empty = set.is_empty();
        if empty {
            map.remove(room_id);
        }
        users.sort();
        users
    }

    /// Drop all incoming typing state and return the rooms that had
    /// typists, so the caller can emit an empty set for each and clear
    /// stale indicators. Orphan safety net at the start of a sync
    /// session (covers a sender that vanished without a `stopped`).
    pub(crate) fn clear_incoming(&self) -> Vec<String> {
        let mut map = self
            .incoming
            .lock()
            .expect("typing incoming mutex poisoned");
        let rooms: Vec<String> = map.keys().cloned().collect();
        map.clear();
        rooms
    }

    /// Mark `room_id` as having an in-flight outgoing typing
    /// notification. Returns `true` when newly marked (the caller should
    /// send `started`), `false` when one was already active (dedupe —
    /// skip the send).
    pub(crate) fn begin_outgoing(&self, room_id: &str) -> bool {
        debug_assert!(!room_id.is_empty(), "room_id must be non-empty");
        self.outgoing
            .lock()
            .expect("typing outgoing mutex poisoned")
            .insert(room_id.to_string())
    }

    /// Clear `room_id`'s in-flight marker. Returns `true` when one was
    /// active (the caller should send `stopped`), `false` otherwise
    /// (no-op). This guarantees exactly one `stopped` across the
    /// explicit call and the auto-stop, whichever fires first.
    pub(crate) fn end_outgoing(&self, room_id: &str) -> bool {
        debug_assert!(!room_id.is_empty(), "room_id must be non-empty");
        self.outgoing
            .lock()
            .expect("typing outgoing mutex poisoned")
            .remove(room_id)
    }
}

#[cfg(test)]
mod tests {
    use super::TypingTracker;

    const ROOM: &str = "room-uuid@wire.com";
    const ALICE: &str = "alice-uuid@wire.com";
    const BOB: &str = "bob-uuid@wire.com";

    #[test]
    fn incoming_started_adds_and_stopped_removes() {
        let t = TypingTracker::default();
        assert_eq!(t.record_incoming(ROOM, ALICE, true), vec![ALICE]);
        assert_eq!(
            t.record_incoming(ROOM, BOB, true),
            vec![ALICE.to_string(), BOB.to_string()],
            "set is sorted",
        );
        assert_eq!(t.record_incoming(ROOM, ALICE, false), vec![BOB]);
        assert!(
            t.record_incoming(ROOM, BOB, false).is_empty(),
            "last typist leaving yields an empty set",
        );
    }

    #[test]
    fn incoming_started_is_idempotent() {
        let t = TypingTracker::default();
        t.record_incoming(ROOM, ALICE, true);
        assert_eq!(
            t.record_incoming(ROOM, ALICE, true),
            vec![ALICE],
            "a duplicate started does not double-count",
        );
    }

    #[test]
    fn incoming_rooms_are_independent() {
        let t = TypingTracker::default();
        t.record_incoming("room-a@wire.com", ALICE, true);
        assert_eq!(t.record_incoming("room-b@wire.com", BOB, true), vec![BOB]);
    }

    #[test]
    fn clear_incoming_returns_affected_rooms_and_empties_them() {
        let t = TypingTracker::default();
        t.record_incoming(ROOM, ALICE, true);
        assert_eq!(
            t.clear_incoming(),
            vec![ROOM],
            "clear reports the rooms that had typists",
        );
        // A fresh started after the clear sees an empty room again.
        assert_eq!(t.record_incoming(ROOM, BOB, true), vec![BOB]);
    }

    #[test]
    fn outgoing_dedupes_started_until_stopped() {
        let t = TypingTracker::default();
        assert!(t.begin_outgoing(ROOM), "first started marks in-flight");
        assert!(
            !t.begin_outgoing(ROOM),
            "second started while in-flight is deduped",
        );
        assert!(t.end_outgoing(ROOM), "stopped clears the in-flight marker");
        assert!(
            !t.end_outgoing(ROOM),
            "a second stopped is a no-op (exactly-once stopped)",
        );
        assert!(
            t.begin_outgoing(ROOM),
            "after stopping, a new started marks in-flight again",
        );
    }
}

/// Auto-stop window for outbound typing: after sending `started`,
/// Kupferdraht emits `stopped` this long later unless the caller already
/// did. Step 21.
// Used by the native auto-stop spawn (gated out on wasm) + the tests.
#[cfg(any(not(target_arch = "wasm32"), test))]
const TYPING_AUTOSTOP: std::time::Duration = std::time::Duration::from_secs(11);

impl Client {
    /// Send a typing `started` for `room_id` and auto-enqueue a
    /// `stopped` after [`TYPING_AUTOSTOP`]. Deduped: a second call while
    /// one is in flight is a no-op. The app/UI decides when to call
    /// this; Kupferdraht guarantees a `stopped` follows within the window
    /// (Step 21). `room_id` is the qualified `<uuid>@<domain>` form.
    ///
    /// # Errors
    /// * `InvalidInput` if `room_id` is unqualified.
    /// * `BackendRejected` / `Network` from the typing POST.
    pub async fn send_typing_started(&self, room_id: &str) -> Result<(), KupferdrahtError> {
        crate::client::input::require_non_empty("send_typing_started", &[("room_id", room_id)])?;
        // Hold the send lock across the marker check + POST so a
        // concurrent `stopped` (explicit or auto) cannot reorder ahead of
        // this `started` on the wire. The auto-stop is spawned only after
        // the guard is released.
        {
            let _send = self.typing_send_lock.lock().await;
            if !self.typing.begin_outgoing(room_id) {
                return Ok(());
            }
            if let Err(e) = self.http_send_typing(room_id, "started").await {
                self.typing.end_outgoing(room_id);
                return Err(e);
            }
        }
        // The ~11 s typing auto-stop runs on a spawned background task. On wasm
        // there is no `tokio::spawn`/multi-thread runtime; the auto-stop is
        // skipped until the single-thread `spawn_local` sync work lands (a later
        // step). The explicit `send_typing_stopped` path still works. docs/plan/44-*.
        #[cfg(not(target_arch = "wasm32"))]
        {
            let weak = self.weak();
            let room = room_id.to_string();
            tokio::spawn(async move {
                tokio::time::sleep(TYPING_AUTOSTOP).await;
                if let Some(client) = weak.upgrade()
                    && let Err(e) = client.send_typing_stopped(&room).await
                {
                    tracing::debug!(error = %e, room, "typing auto-stop send failed");
                }
            });
        }
        Ok(())
    }

    /// Send a typing `stopped` for `room_id`, but only if one was
    /// outstanding — a no-op otherwise. Together with the auto-stop this
    /// guarantees exactly one `stopped` per `started`. Step 21.
    ///
    /// # Errors
    /// * `InvalidInput` if `room_id` is unqualified.
    /// * `BackendRejected` / `Network` from the typing POST.
    pub async fn send_typing_stopped(&self, room_id: &str) -> Result<(), KupferdrahtError> {
        crate::client::input::require_non_empty("send_typing_stopped", &[("room_id", room_id)])?;
        // Same send-lock-across-POST ordering guarantee as
        // `send_typing_started`.
        let _send = self.typing_send_lock.lock().await;
        if !self.typing.end_outgoing(room_id) {
            return Ok(());
        }
        self.http_send_typing(room_id, "stopped").await
    }

    async fn http_send_typing(&self, room_id: &str, status: &str) -> Result<(), KupferdrahtError> {
        self.http
            .send_typing(
                self.api_version,
                &self.current_access_token(),
                room_id,
                status,
            )
            .await
    }

    /// Record an inbound `conversation.typing` started/stopped and
    /// return the room's current typing-user set. Crate-private —
    /// driven by `SyncService::dispatch_inbound_typing`. Step 21.
    pub(crate) fn record_incoming_typing(
        &self,
        room_id: &str,
        user_id: &str,
        started: bool,
    ) -> Vec<String> {
        self.typing.record_incoming(room_id, user_id, started)
    }

    /// Snapshot the registered event handlers and fire `on_typing` with
    /// `event`. Same snapshot-and-dispatch shape as
    /// `SyncService::notify_message`. Step 21.
    pub(crate) fn notify_typing(&self, event: &crate::sync::TypingEvent) {
        let snapshot = self
            .event_handlers
            .lock()
            .expect("event_handlers mutex poisoned")
            .clone();
        for h in &snapshot {
            h.on_typing(event.clone());
        }
    }

    /// Orphan safety net: at the start of a sync session, drop all
    /// incoming typing state and emit an empty set for each affected
    /// room so observers clear stale indicators — covers a sender that
    /// vanished without a `stopped`. Step 21.
    pub(crate) fn reset_typing_on_sync_start(&self) {
        for room_id in self.typing.clear_incoming() {
            self.notify_typing(&crate::sync::TypingEvent {
                room_id,
                typing_users: Vec::new(),
            });
        }
    }
}

#[cfg(test)]
mod typing_autostop_tests {
    use super::TYPING_AUTOSTOP;
    use std::time::Duration;

    #[test]
    fn autostop_window_is_eleven_seconds() {
        // Guards the auto-stop window against an accidental change.
        assert_eq!(TYPING_AUTOSTOP, Duration::from_secs(11));
    }
}
