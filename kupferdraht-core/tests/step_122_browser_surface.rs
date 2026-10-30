// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 122 — small gaps of the browser surface: a rate limit's
//! `retryAfter` crosses the boundary, and an account's push state reads
//! `"connected"` / `"polling"` / `"stopped"`. The loop's own transitions,
//! the send that wakes it, `email` and `storageEngine().location` need a
//! backend and a storage server: the step's live smoke.
//!
//! Run: `RUSTUP_TOOLCHAIN=1.98.0 bash scripts/wasm-test.sh`
#![cfg(target_arch = "wasm32")]

use kupferdraht::KupferdrahtError;
use kupferdraht::wasm::js_error;
use kupferdraht::wasm::push_state::{clear_push_state, push_state, set_push_state};
use wasm_bindgen::JsValue;
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

wasm_bindgen_test_configure!(run_in_browser);

fn property(value: &JsValue, key: &str) -> JsValue {
    js_sys::Reflect::get(value, &JsValue::from_str(key)).expect("property read")
}

#[wasm_bindgen_test]
fn a_rate_limit_carries_its_retry_after() {
    let error = js_error(&KupferdrahtError::RateLimited {
        retry_after_secs: Some(240),
    });

    assert_eq!(
        property(&error, "name").as_string().as_deref(),
        Some("RateLimited")
    );
    assert_eq!(property(&error, "retryAfter").as_f64(), Some(240.0));
}

#[wasm_bindgen_test]
fn a_rate_limit_without_a_known_wait_carries_none() {
    let error = js_error(&KupferdrahtError::RateLimited {
        retry_after_secs: None,
    });

    assert!(property(&error, "retryAfter").is_undefined());
}

#[wasm_bindgen_test]
fn without_a_running_loop_push_is_stopped() {
    assert_eq!(push_state("nobody@example.test"), "stopped");
}

#[wasm_bindgen_test]
fn a_running_loop_reports_its_socket() {
    let account = "alice@example.test";

    set_push_state(account, true);
    assert_eq!(push_state(account), "connected");

    set_push_state(account, false);
    assert_eq!(push_state(account), "polling");

    clear_push_state(account);
    assert_eq!(push_state(account), "stopped");
}

#[wasm_bindgen_test]
fn push_state_is_per_account() {
    set_push_state("carol@example.test", true);

    assert_eq!(push_state("dave@example.test"), "stopped");
    clear_push_state("carol@example.test");
}
