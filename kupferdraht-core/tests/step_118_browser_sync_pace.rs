// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 118 — the browser's sync loop waits for the bell while its socket
//! lives, and polls only without one.
//!
//! Before: the loop ran a full pass every 2.5 s whether or not the socket
//! was live. Now a live socket is waited on for up to a minute, then
//! renewed (a browser cannot ping, so a silently dead socket is invisible
//! otherwise); without a socket — push unavailable — the loop polls every
//! 10 s; and a socket that closes at once cannot make it connect more often
//! than that. The socket and its close are covered by the step's live smoke.
//!
//! Run: `RUSTUP_TOOLCHAIN=1.98.0 bash scripts/wasm-test.sh`
#![cfg(target_arch = "wasm32")]

use std::time::Duration;

use kupferdraht::wasm::sync_pace::{NO_SOCKET_POLL, PUSH_RENEWAL, connect_due, idle_wait};
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

wasm_bindgen_test_configure!(run_in_browser);

const NOW_MS: i64 = 1_800_000_000_000;

#[wasm_bindgen_test]
fn with_a_live_socket_the_loop_waits_for_the_renewal() {
    assert_eq!(PUSH_RENEWAL, Duration::from_secs(60));
    assert_eq!(idle_wait(true), PUSH_RENEWAL);
}

#[wasm_bindgen_test]
fn without_a_socket_the_loop_polls_every_ten_seconds() {
    assert_eq!(NO_SOCKET_POLL, Duration::from_secs(10));
    assert_eq!(idle_wait(false), NO_SOCKET_POLL);
}

#[wasm_bindgen_test]
fn the_first_connect_is_due_at_once() {
    assert!(connect_due(None, NOW_MS));
}

#[wasm_bindgen_test]
fn a_reconnect_after_a_long_lived_socket_is_due_at_once() {
    assert!(connect_due(Some(NOW_MS - 60_000), NOW_MS));
}

#[wasm_bindgen_test]
fn no_second_connect_within_ten_seconds() {
    assert!(!connect_due(Some(NOW_MS - 1_000), NOW_MS));
    assert!(!connect_due(Some(NOW_MS - 9_999), NOW_MS));
    assert!(connect_due(Some(NOW_MS - 10_000), NOW_MS));
}
