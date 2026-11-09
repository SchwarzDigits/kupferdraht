// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 130b — `resendFailed`'s summary as a browser sees it, built from
//! a core `ResendSummary` in the browser and read back as a JS value: the
//! re-driven handles, the skipped ids, the remaining count, and the stop —
//! `null`, not `undefined`, when the pass went through (the hand-written
//! types say `ResendStop | null`), else the row's id with the stopping
//! error's name and message. The pass itself needs a logged-in client the
//! browser suite has not; it is pinned natively (`step_79_failed_send_recovery`).
//!
//! Run: `scripts/wasm-test.sh` (headless Firefox).
#![cfg(target_arch = "wasm32")]

use kupferdraht::wasm::js_view::to_js;
use kupferdraht::wasm::resend::summary_view;
use kupferdraht::{MessageHandle, ResendSummary};
use wasm_bindgen::JsValue;
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

wasm_bindgen_test_configure!(run_in_browser);

fn property(value: &JsValue, key: &str) -> JsValue {
    js_sys::Reflect::get(value, &JsValue::from_str(key)).expect("property read")
}

fn array(value: &JsValue, key: &str) -> js_sys::Array {
    let field = property(value, key);
    assert!(js_sys::Array::is_array(&field), "{key} is an array");
    js_sys::Array::from(&field)
}

#[wasm_bindgen_test]
fn a_pass_that_went_through_crosses_with_a_null_stop() {
    let summary = ResendSummary {
        resent: vec![MessageHandle {
            message_id: "msg-1".into(),
            sent_at_ms: 1_000,
        }],
        skipped: vec!["task-1".into()],
        stopped_at: None,
        remaining: 0,
    };
    let js = to_js(&summary_view(&summary, None)).expect("summary");

    let resent = array(&js, "resent");
    assert_eq!(resent.length(), 1);
    let handle = resent.get(0);
    assert_eq!(
        property(&handle, "messageId").as_string().as_deref(),
        Some("msg-1")
    );
    assert_eq!(property(&handle, "sentAtMs").as_f64(), Some(1_000.0));
    let skipped = array(&js, "skipped");
    assert_eq!(skipped.get(0).as_string().as_deref(), Some("task-1"));
    assert!(
        property(&js, "stoppedAt").is_null(),
        "no stop is null, not undefined"
    );
    assert_eq!(property(&js, "remaining").as_f64(), Some(0.0));
}

#[wasm_bindgen_test]
fn a_stopped_pass_names_the_error_and_counts_the_rows_behind_it() {
    let summary = ResendSummary {
        resent: Vec::new(),
        skipped: Vec::new(),
        stopped_at: Some(("msg-2".into(), "backend rejected: 500".into())),
        remaining: 2,
    };
    let js = to_js(&summary_view(&summary, Some("BackendRejected"))).expect("summary");

    assert_eq!(
        array(&js, "resent").length(),
        0,
        "an empty list, not absent"
    );
    assert_eq!(array(&js, "skipped").length(), 0);
    let stop = property(&js, "stoppedAt");
    assert_eq!(
        property(&stop, "messageId").as_string().as_deref(),
        Some("msg-2")
    );
    assert_eq!(
        property(&stop, "name").as_string().as_deref(),
        Some("BackendRejected"),
        "the JS error name a caller branches on"
    );
    assert_eq!(
        property(&stop, "message").as_string().as_deref(),
        Some("backend rejected: 500")
    );
    assert_eq!(property(&js, "remaining").as_f64(), Some(2.0));
}
