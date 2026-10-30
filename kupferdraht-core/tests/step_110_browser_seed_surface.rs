// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 110 — the browser surface of an account's seed (ADR-0035). TDD red.
//!
//! The behaviour lives in the core and is driven natively
//! (`manager::seed_login_tests`); these pins hold the browser entry point to
//! it: `Kupferdraht.open` opens a home whose account keys come from a seed, the
//! pending login and the client expose what the flow needs, errors carry
//! their `KupferdrahtError` variant as `name`, the first-setup callers ask for
//! it, and the embedder guide describes the flow. The tab module mirrors
//! every member (`step_94_tab_proxy_parity`).

use std::path::Path;

fn workspace_file(rel: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn wasm_manager() -> String {
    workspace_file("kupferdraht-core/src/wasm/manager.rs")
}

#[test]
fn kupferdraht_open_opens_a_home_whose_account_keys_come_from_a_seed() {
    assert!(
        wasm_manager().contains(".account_keys(AccountKeySource::Seed)"),
        "wasm::Kupferdraht::open builds its manager with the seed key source"
    );
}

#[test]
fn the_pending_login_exposes_keys_the_hint_and_cancel() {
    let src = wasm_manager();
    for needle in [
        "pub fn keys(&self) -> String",
        "js_name = recoveryExpected",
        "pub fn cancel(&self)",
        "pub async fn register_device(&self, options: JsValue)",
    ] {
        assert!(src.contains(needle), "wasm PendingLogin: {needle}");
    }
}

#[test]
fn the_client_reports_a_replaced_device_and_takes_logout_options() {
    let src = wasm_manager();
    for needle in [
        "js_name = deviceReplaced",
        "pub async fn logout(&self, options: JsValue)",
    ] {
        assert!(src.contains(needle), "wasm Client: {needle}");
    }
}

/// JavaScript branches on `error.name`, as Kotlin and Swift on the type.
#[test]
fn thrown_errors_carry_their_variant_as_name() {
    let src = wasm_manager();
    assert!(
        src.contains("fn to_js(e: &KupferdrahtError) -> JsValue"),
        "the binding throws a JS Error it can name"
    );
    assert!(src.contains("e.variant_name()"), "named after the variant");
    assert!(
        !src.contains("Result<Client, JsError>"),
        "no entry point throws an unnamed JsError any more"
    );
}

/// The smoke page drives a first setup in a fresh browser.
#[test]
fn the_smoke_page_asks_for_a_first_setup() {
    let src = workspace_file("bindings/web/opfs-smoke/worker.js");
    assert!(
        src.contains("newSetup: true"),
        "opfs-smoke: a first setup passes {{ newSetup: true }}"
    );
}

#[test]
fn the_embedder_guide_describes_the_flow() {
    let doc = workspace_file("docs/bindings.md");
    for needle in [
        "recoveryExpected",
        "newSetup",
        "RecoveryDataRequired",
        "deviceReplaced",
    ] {
        assert!(doc.contains(needle), "docs/bindings.md names {needle}");
    }
}
