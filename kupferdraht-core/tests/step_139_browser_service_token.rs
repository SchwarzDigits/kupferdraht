// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 139 — service tokens as the browser sees them: the exchange's answer
//! crosses as `{ token, expiresIn }` and nothing else, and the local mode
//! has no exchange to buy from (`InvalidInput`). `serviceToken` itself needs
//! a logged-in client the browser suite has not: the purchase, the one
//! renewal and the cache per audience are pinned natively
//! (`token_exchange`, `wire_session`, `service_tokens`), the thin-tab path
//! by the Step 94 driver.
//!
//! Run: `scripts/wasm-test.sh` (headless Firefox).
#![cfg(target_arch = "wasm32")]

use kupferdraht::KupferdrahtError;
use kupferdraht::wasm::js_view::to_js;
use kupferdraht::wasm::service_token::{ServiceTokenView, exchange};
use wasm_bindgen::JsValue;
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

wasm_bindgen_test_configure!(run_in_browser);

fn property(value: &JsValue, key: &str) -> JsValue {
    js_sys::Reflect::get(value, &JsValue::from_str(key)).expect("property read")
}

#[wasm_bindgen_test]
fn the_answer_crosses_as_token_and_expires_in() {
    let view = ServiceTokenView {
        token: "cases.jwt".into(),
        expires_in: 900,
    };
    let answer = to_js(&view).expect("answer");

    assert_eq!(
        property(&answer, "token").as_string().as_deref(),
        Some("cases.jwt")
    );
    assert_eq!(
        property(&answer, "expiresIn").as_f64(),
        Some(900.0),
        "seconds, as a number"
    );
    let keys = js_sys::Object::keys(&js_sys::Object::from(answer));
    assert_eq!(keys.length(), 2, "the exchange's answer, nothing else");
}

#[wasm_bindgen_test]
fn the_local_mode_has_no_exchange() {
    let Err(KupferdrahtError::InvalidInput(message)) = exchange() else {
        panic!("the local mode must refuse with InvalidInput");
    };
    assert!(
        message.contains("no token exchange configured"),
        "{message}"
    );
}
