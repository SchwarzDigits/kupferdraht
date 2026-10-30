// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 118 — the pace every sync loop shares, the browser's and the native
//! ones: how long a silent socket lives, and how often a loop without a
//! socket drains (and a push loop tries again).

use std::time::Duration;

/// How long a push socket may stay without a frame before its loop renews
/// it and drains once. A browser cannot ping, and a ping only finds a dead
/// connection when a write fails: a socket that died silently, or one a
/// proxy holds frames on, looks alive to both loops.
pub(crate) const PUSH_RENEWAL: Duration = Duration::from_mins(1);

/// How often a loop without a socket drains — the polling loop, and a push
/// loop while its socket is down, which also tries to connect at that
/// pace. One value on every target; `SyncSettings::poll_interval`
/// overrides it natively.
pub(crate) const NO_SOCKET_POLL: Duration = Duration::from_secs(10);

/// Whether a socket that has been without a frame for `silent_for` is due
/// for renewal.
#[must_use]
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn socket_silent(silent_for: Duration) -> bool {
    silent_for >= PUSH_RENEWAL
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn a_silent_socket_is_renewed_after_a_minute() {
        assert_eq!(PUSH_RENEWAL, Duration::from_mins(1));
        assert!(!socket_silent(Duration::from_secs(59)));
        assert!(socket_silent(Duration::from_secs(60)));
    }

    #[test]
    fn without_a_socket_a_loop_drains_every_ten_seconds() {
        assert_eq!(NO_SOCKET_POLL, Duration::from_secs(10));
    }
}
