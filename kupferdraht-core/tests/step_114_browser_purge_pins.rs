// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 114 — native pins for the browser's `logout` purge options
//! (ADR-0035). TDD red. The purge itself is tested in the crate
//! (`manager::purge_tests`); the server mode's deletion is the smoke's.
//! These hold the shape of the binding, the tabs layer's declaration and
//! what the embedder guide says.

use std::path::Path;

fn workspace_file(rel: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

#[test]
fn the_binding_takes_the_purge_options() {
    let binding = workspace_file("kupferdraht-core/src/wasm/manager.rs");
    for needle in ["purge_local: bool", "purge_device: bool", "logout_client("] {
        assert!(binding.contains(needle), "wasm/manager.rs: {needle}");
    }
}

#[test]
fn the_tabs_declaration_carries_purge_local() {
    let dts = workspace_file("bindings/web/kupferdraht-tabs/index.d.ts");
    let needle = "logout(options?: { purgeDevice?: boolean; password?: string; purgeLocal?: boolean }): Promise<void>;";
    assert!(dts.contains(needle), "index.d.ts: {needle}");
}

#[test]
fn the_embedder_guide_describes_leaving_a_browser() {
    let bindings = workspace_file("docs/bindings.md");
    for needle in [
        "logout({ purgeDevice, password, purgeLocal })",
        "leave a browser for good",
        "NothingStored",
        "the client is closed",
    ] {
        assert!(bindings.contains(needle), "docs/bindings.md: {needle}");
    }
}
