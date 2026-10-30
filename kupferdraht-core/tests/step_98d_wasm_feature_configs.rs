// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 98d — the wasm surface must expose the server-administered config.
//!
//! `feature_configs` and `file_sharing` tell a client what the team allows.
//! Without them a browser cannot say *why* a send was refused — it can only
//! report a generic failure — and cannot grey out an attachment button it
//! already knows the team has disabled.
//!
//! Source-level pins — see `wasm_surface/mod.rs`.

mod wasm_surface;

use wasm_surface::{assert_bound, surface};

#[test]
fn client_binds_server_administered_config() {
    let s = surface();
    assert_bound(&s, "Client", "featureConfigs");
    assert_bound(&s, "Client", "fileSharing");
}

/// The view is an object with a `policy` string (`"disabled" | "enabled"`),
/// not a bare boolean, so a later field can join without changing the shape.
#[test]
fn file_sharing_view_is_an_object_with_a_policy_string() {
    let src = wasm_surface::manager_source();
    assert!(
        src.contains("struct FileSharingView"),
        "manager.rs projects FileSharingPolicy into a view struct"
    );
    assert!(
        src.contains("policy: &'static str"),
        "FileSharingView carries `policy`"
    );
    assert!(
        !src.contains("allowed_extensions"),
        "the allow-list left with the former EnabledSome variant"
    );
}
