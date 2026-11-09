// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 130c — a browser view's missing value as a browser sees it: `null`,
//! not `undefined`, because the hand-written types say `| null` and a
//! consumer comparing `=== null` must not miss it. The views themselves are
//! private to their modules, so a fixture of the same shapes crosses through
//! the one serializer they all use; the native pins
//! (`step_130c_null_views_pins`) hold that they do.
//!
//! Run: `scripts/wasm-test.sh` (headless Firefox).
#![cfg(target_arch = "wasm32")]

use kupferdraht::wasm::js_view::to_js;
use serde::Serialize;
use wasm_bindgen::JsValue;
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

wasm_bindgen_test_configure!(run_in_browser);

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AssetFixture {
    file_name: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ItemFixture {
    body: Option<String>,
    expires_at_ms: Option<i64>,
    asset: Option<AssetFixture>,
    attachments: Vec<AssetFixture>,
    #[serde(skip_serializing_if = "Option::is_none")]
    exported_at: Option<i64>,
}

fn empty_item() -> ItemFixture {
    ItemFixture {
        body: None,
        expires_at_ms: None,
        asset: None,
        attachments: Vec::new(),
        exported_at: None,
    }
}

fn property(value: &JsValue, key: &str) -> JsValue {
    js_sys::Reflect::get(value, &JsValue::from_str(key)).expect("property read")
}

#[wasm_bindgen_test]
fn a_missing_field_crosses_as_null() {
    let item = to_js(&empty_item()).expect("item crosses");

    assert!(property(&item, "body").is_null(), "body is null");
    assert!(
        property(&item, "expiresAtMs").is_null(),
        "expiresAtMs is null"
    );
    assert!(property(&item, "asset").is_null(), "asset is null");
}

#[wasm_bindgen_test]
fn a_missing_field_inside_a_present_object_or_a_list_crosses_as_null() {
    let item = to_js(&ItemFixture {
        asset: Some(AssetFixture { file_name: None }),
        attachments: vec![AssetFixture { file_name: None }],
        ..empty_item()
    })
    .expect("item crosses");

    assert!(
        property(&property(&item, "asset"), "fileName").is_null(),
        "a nested field is null"
    );
    let first = js_sys::Reflect::get_u32(&property(&item, "attachments"), 0).expect("list row");
    assert!(
        property(&first, "fileName").is_null(),
        "a field in a list row is null"
    );
}

#[wasm_bindgen_test]
fn a_missing_view_crosses_as_null() {
    let none: Option<AssetFixture> = None;

    assert!(to_js(&none).expect("no view crosses").is_null());
}

#[wasm_bindgen_test]
fn a_present_value_crosses_unchanged() {
    let item = to_js(&ItemFixture {
        body: Some("hello".to_owned()),
        expires_at_ms: Some(1_500),
        asset: Some(AssetFixture {
            file_name: Some("a.txt".to_owned()),
        }),
        ..empty_item()
    })
    .expect("item crosses");

    assert_eq!(
        property(&item, "body").as_string().as_deref(),
        Some("hello")
    );
    assert_eq!(property(&item, "expiresAtMs").as_f64(), Some(1_500.0));
    assert_eq!(
        property(&property(&item, "asset"), "fileName")
            .as_string()
            .as_deref(),
        Some("a.txt")
    );
}

#[wasm_bindgen_test]
fn a_skipped_field_stays_absent() {
    let item = to_js(&empty_item()).expect("item crosses");

    assert!(
        !js_sys::Reflect::has(&item, &JsValue::from_str("exportedAt")).expect("has"),
        "a field the view skips is no property at all"
    );
}
