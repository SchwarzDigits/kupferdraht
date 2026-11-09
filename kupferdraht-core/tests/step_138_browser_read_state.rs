// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 138 — the read state as a browser sees it: the three fields of a
//! `rooms()` row and the `onRoomsChanged` payload, built from the store's
//! records in the browser and read back as JS values. A room never read
//! crosses with `lastReadAtMs === null` (the hand-written types say
//! `number | null`, so a thin tab must not meet `undefined`). `rooms()`
//! itself needs a logged-in client the browser suite has not; its join of
//! the two store queries is pinned natively (`step_138_read_state`).
//!
//! Run: `scripts/wasm-test.sh` (headless Firefox).
#![cfg(target_arch = "wasm32")]

use kupferdraht::storage::ReadStateRecord;
use kupferdraht::wasm::js_view::to_js;
use kupferdraht::wasm::read_state::{read_state_change, room_view};
use wasm_bindgen::JsValue;
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

wasm_bindgen_test_configure!(run_in_browser);

fn property(value: &JsValue, key: &str) -> JsValue {
    js_sys::Reflect::get(value, &JsValue::from_str(key)).expect("property read")
}

#[wasm_bindgen_test]
fn a_room_never_read_crosses_with_a_null_marker_and_no_flag() {
    let row = to_js(&room_view("room-1@example.test", "Fixture room", 2, None)).expect("row");

    assert_eq!(
        property(&row, "roomId").as_string().as_deref(),
        Some("room-1@example.test")
    );
    assert_eq!(
        property(&row, "name").as_string().as_deref(),
        Some("Fixture room")
    );
    assert_eq!(property(&row, "unreadCount").as_f64(), Some(2.0));
    assert!(
        property(&row, "lastReadAtMs").is_null(),
        "a missing marker is null, not undefined"
    );
    assert_eq!(property(&row, "markedUnread").as_bool(), Some(false));
}

#[wasm_bindgen_test]
fn a_flagged_room_carries_its_marker_and_its_flag() {
    let state = ReadStateRecord {
        last_read_at_ms: Some(1_500),
        marked_unread: true,
    };
    let row = to_js(&room_view(
        "room-1@example.test",
        "Fixture room",
        0,
        Some(&state),
    ))
    .expect("row");

    assert_eq!(property(&row, "unreadCount").as_f64(), Some(0.0));
    assert_eq!(property(&row, "lastReadAtMs").as_f64(), Some(1_500.0));
    assert_eq!(property(&row, "markedUnread").as_bool(), Some(true));
}

#[wasm_bindgen_test]
fn the_change_payload_is_the_rooms_row_without_the_name() {
    let state = ReadStateRecord {
        last_read_at_ms: None,
        marked_unread: true,
    };
    let change = to_js(&read_state_change("room-1@example.test", 1, Some(&state))).expect("change");

    assert_eq!(
        property(&change, "roomId").as_string().as_deref(),
        Some("room-1@example.test")
    );
    assert_eq!(property(&change, "unreadCount").as_f64(), Some(1.0));
    assert!(property(&change, "lastReadAtMs").is_null());
    assert_eq!(property(&change, "markedUnread").as_bool(), Some(true));
    assert!(property(&change, "name").is_undefined());
}
