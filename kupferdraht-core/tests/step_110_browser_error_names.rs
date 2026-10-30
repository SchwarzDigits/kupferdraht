// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 110 — the browser binding throws errors named after their
//! `KupferdrahtError` variant (ADR-0035), so an embedder branches on
//! `error.name` as Kotlin and Swift do on the typed error. TDD red.
//! Runs in a **dedicated worker** (`scripts/wasm-test.sh`).
#![cfg(target_arch = "wasm32")]

use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

wasm_bindgen_test_configure!(run_in_dedicated_worker);

fn property(value: &JsValue, key: &str) -> Option<String> {
    js_sys::Reflect::get(value, &JsValue::from_str(key))
        .ok()
        .and_then(|v| v.as_string())
}

#[wasm_bindgen_test]
async fn a_thrown_error_is_named_after_its_variant() {
    let kupferdraht = kupferdraht::wasm::Kupferdraht::open(JsValue::UNDEFINED, None)
        .await
        .expect("Kupferdraht.open in a dedicated worker");

    let Err(error) = kupferdraht
        .resume("nobody@example.test".to_owned(), JsValue::UNDEFINED)
        .await
    else {
        panic!("resuming an unknown account fails");
    };

    assert!(error.is_instance_of::<js_sys::Error>(), "a JS Error");
    assert_eq!(property(&error, "name").as_deref(), Some("InvalidInput"));
    assert!(
        property(&error, "message").is_some_and(|m| m.contains("unknown account")),
        "the message stays"
    );
}
