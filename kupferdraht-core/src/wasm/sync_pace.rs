// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 118 — the pace of the browser's sync loop: how long it waits for
//! the doorbell, and when it may connect the socket again.
//!
//! With a live socket the loop waits for the bell. A browser cannot ping,
//! so a socket that died silently (a sleeping laptop, a dropped NAT
//! mapping) looks open; after [`PUSH_RENEWAL`] without a ring the loop
//! renews it and drains once. Without a socket — push unavailable — it
//! polls every [`NO_SOCKET_POLL`]. Public for the browser suite only.

use std::time::Duration;

/// How long a live socket may stay silent before the loop renews it —
/// shared with the native push loop (`sync::pace`).
pub const PUSH_RENEWAL: Duration = crate::sync::pace::PUSH_RENEWAL;

/// How often the loop polls, and tries the socket again, without one —
/// shared with the native push loop (`sync::pace`).
pub const NO_SOCKET_POLL: Duration = crate::sync::pace::NO_SOCKET_POLL;

/// How long the loop waits for the bell: [`PUSH_RENEWAL`] with a live
/// socket, [`NO_SOCKET_POLL`] without one.
#[must_use]
pub fn idle_wait(socket_open: bool) -> Duration {
    if socket_open {
        PUSH_RENEWAL
    } else {
        NO_SOCKET_POLL
    }
}

/// Whether the loop may connect the socket now: at once for the first
/// connect, then at most once per [`NO_SOCKET_POLL`] — a server that
/// accepts and closes at once cannot spin the loop.
#[must_use]
pub fn connect_due(last_connect_ms: Option<i64>, now_ms: i64) -> bool {
    let gap_ms = i64::try_from(NO_SOCKET_POLL.as_millis()).unwrap_or(i64::MAX);
    last_connect_ms.is_none_or(|last| now_ms.saturating_sub(last) >= gap_ms)
}
