// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 98a — the wasm surface must carry session and device admin.
//!
//! These capabilities exist in `kupferdraht-core` and are wrapped in `ffi/`, but
//! the hand-written `#[wasm_bindgen]` layer never bound them, so a browser
//! embedder cannot change a profile, list the account's devices, or recover
//! from Wire's seven-device cap during login.
//!
//! Source-level pins rather than browser tests — see `wasm_surface/mod.rs`
//! for why, and for the shared enumerator these sub-steps share.

mod wasm_surface;

use wasm_surface::{assert_bound, surface};

#[test]
fn client_binds_session_admin() {
    let s = surface();
    assert_bound(&s, "Client", "setDisplayName");
    assert_bound(&s, "Client", "setHandle");
    assert_bound(&s, "Client", "updateEmail");
}

#[test]
fn client_binds_device_management() {
    let s = surface();
    assert_bound(&s, "Client", "devices");
    assert_bound(&s, "Client", "deleteDevice");
}

/// The device-limit recovery path: on `TooManyDevices` the `PendingLogin`
/// stays valid, so the caller frees a slot and calls `registerDevice` again
/// on the same handle. Without these two the browser has no way back.
#[test]
fn pending_login_binds_device_limit_recovery() {
    let s = surface();
    assert_bound(&s, "PendingLogin", "devices");
    assert_bound(&s, "PendingLogin", "deleteDevice");
}

/// Device payloads cross to JS as owned camelCase objects, like every other
/// view struct on this surface — never as opaque handles.
///
/// The field set is `core::DeviceInfo`'s, not an invention of the binding:
/// a browser consumer must see the same record Kotlin and Swift see.
#[test]
fn device_payload_mirrors_core_device_info() {
    let src = wasm_surface::manager_source();
    assert!(
        src.contains("struct DeviceInfoView"),
        "manager.rs projects DeviceInfo into a view struct"
    );
    for field in [
        "device_id",
        "class",
        "label",
        "model",
        "registered_at",
        "last_active",
        "is_self",
    ] {
        assert!(
            src.contains(field),
            "DeviceInfoView carries core DeviceInfo's `{field}`"
        );
    }
    assert!(
        src.contains("#[serde(rename_all = \"camelCase\")]"),
        "view structs are camelCase on the JS side"
    );
}
