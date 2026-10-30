// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `SyncSettings` + `SyncSummary` — the public configuration and
//! result types of the Step-14b sync-surface refactor.
//!
//! `SyncSettings` mirrors matrix-rust-sdk's `SyncSettings` shape but
//! drops `timeout` (Wire's `/v14/notifications` is not a server-side
//! long-poll — finding §40) in favour of `poll_interval`, which paces
//! the client-side wait between empty drains in the loop variants
//! (`sync`, `sync_with_callback`, `sync_stream`). `sync_once` ignores
//! the field entirely — one iteration never sleeps.
//!
//! `SyncSummary` is a per-iteration aggregate returned by `sync_once`
//! (and reported per-iteration to `sync_with_callback` / streamed by
//! `sync_stream`). Two counts: how many rooms the metadata phase
//! refreshed, how many events the notifications phase dispatched.
//! Kupferdraht intentionally keeps the shape compact; consumers receive
//! per-event detail through the existing `EventHandler` trait, not
//! through the summary.

use std::time::Duration;

/// Step-14b `Client::sync*` configuration. Mirrors matrix-rust-sdk's
/// `SyncSettings` shape: the client-side pacing knob plus, since
/// Step 62, the receive-transport choice.
#[derive(Debug, Clone, Default, PartialEq, Eq, uniffi::Record)]
pub struct SyncSettings {
    /// Pause between consecutive empty `/v14/notifications` drains
    /// in the loop variants (FFI `start_sync`: after every iteration).
    /// `None` means the 10 s default (Step 103's default, 10 s on every
    /// target since Step 118); `Some(Duration::ZERO)` means no pause at
    /// all — for tests. Ignored by `sync_once` — one-shot iterations
    /// never sleep. In [`SyncTransport::WebSocket`] mode it paces the
    /// push loop the same way — at most one connect per pause, and one
    /// drain per pause without a socket (Step 118). After a `429`
    /// ([`crate::KupferdrahtError::RateLimited`]) every loop waits the
    /// backend's `Retry-After` (else this pause), bounded to 1–60 s.
    pub poll_interval: Option<Duration>,
    /// Receive transport for the loop variants (`sync`,
    /// `sync_with_callback`, `sync_stream`). Ignored by `sync_once` —
    /// a one-shot iteration is always a REST drain. Step 62; push by
    /// default on native targets since Step 118 (ADR-0033).
    pub transport: SyncTransport,
}

/// Bounds of the wait after a `429` (Step 103).
const RATE_LIMIT_WAIT_MIN: Duration = Duration::from_secs(1);
const RATE_LIMIT_WAIT_MAX: Duration = Duration::from_secs(60);

impl SyncSettings {
    /// The pause between polls — of the polling loop, and of the push
    /// loop without a socket, which also connects at most once per pause:
    /// `poll_interval`, else [`crate::sync::pace::NO_SOCKET_POLL`] (10 s
    /// on every target since Step 118; Step 103 introduced the default).
    pub(crate) fn idle_pause(&self) -> Duration {
        self.poll_interval
            .unwrap_or(crate::sync::pace::NO_SOCKET_POLL)
    }

    /// The wait after a `429`: the backend's `Retry-After`, else the idle
    /// pause, bounded to 1–60 s. Step 103.
    pub(crate) fn rate_limit_pause(&self, retry_after_secs: Option<u64>) -> Duration {
        let pause = retry_after_secs
            .map_or_else(|| self.idle_pause(), Duration::from_secs)
            .clamp(RATE_LIMIT_WAIT_MIN, RATE_LIMIT_WAIT_MAX);
        debug_assert!((RATE_LIMIT_WAIT_MIN..=RATE_LIMIT_WAIT_MAX).contains(&pause));
        pause
    }
}

/// Receive transport driving the `sync*` loop variants (Step 62).
/// The default is [`SyncTransport::WebSocket`] on native targets and
/// [`SyncTransport::Polling`] in the browser build (ADR-0033, Step 118).
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum SyncTransport {
    /// Drain `GET /v14/notifications` in a paced loop (Step 5/14b) —
    /// works on every target and against every backend; the explicit
    /// choice where push is unwanted.
    Polling,
    /// Long-lived push WebSocket, auto-negotiated per connection
    /// (Step 73, ADR-0019): servers with ConsumableNotifications
    /// enabled get `GET /v14/events` (per-event acks, backend-spec
    /// §11.2); everything else — every current wire-server — gets
    /// the legacy cannon `/await` socket (passive — every frame only
    /// triggers a REST drain, ADR-0030; client pings hold the idle
    /// timeout). Without a socket the loop drains anyway, and it
    /// connects at most once per `poll_interval` (10 s when unset); a
    /// socket without a frame for a minute is renewed (Step 118). The
    /// native default. On wasm the loop variants do not support it —
    /// the browser's live sync has its own socket.
    WebSocket,
}

/// ADR-0033: push wherever the push module compiles.
#[cfg(not(target_arch = "wasm32"))]
impl Default for SyncTransport {
    fn default() -> Self {
        Self::WebSocket
    }
}

/// ADR-0033: the browser build has no push transport for the loop
/// variants (its live sync has its own socket).
#[cfg(target_arch = "wasm32")]
impl Default for SyncTransport {
    fn default() -> Self {
        Self::Polling
    }
}

/// Per-iteration outcome of a `sync_once` (or one tick of a `sync*`
/// loop). Counts are advisory — they aggregate the work the
/// iteration did so callers can log or react. Detailed per-event
/// callbacks still fire through the registered
/// [`crate::sync::EventHandler`]s.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, uniffi::Record)]
pub struct SyncSummary {
    /// Number of rooms the conversations-list refresh touched. `0`
    /// when the cached list was already up to date (subject to
    /// `/v14/conversations/list-ids` reporting `has_more = false`
    /// on the first page).
    pub rooms_updated: u32,
    /// Number of events the notifications-drain dispatched through
    /// the registered `EventHandler`s during this iteration. `0`
    /// when the Wire queue was empty for this device's cursor.
    pub events_dispatched: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ADR-0033: push wherever the push module compiles.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn push_is_the_native_default() {
        assert_eq!(SyncTransport::default(), SyncTransport::WebSocket);
        assert_eq!(SyncSettings::default().transport, SyncTransport::WebSocket);
    }

    /// Step 118: one value without a socket — polling, and push while its
    /// socket is down — on every target.
    #[test]
    fn the_poll_interval_is_ten_seconds_unless_set() {
        assert_eq!(
            SyncSettings::default().idle_pause(),
            Duration::from_secs(10)
        );
        let set = SyncSettings {
            poll_interval: Some(Duration::from_millis(200)),
            ..SyncSettings::default()
        };
        assert_eq!(set.idle_pause(), Duration::from_millis(200));
    }
}
