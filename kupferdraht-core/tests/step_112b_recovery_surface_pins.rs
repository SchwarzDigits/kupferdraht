// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 112b — native pins for the browser's recovery surface (ADR-0035).
//! TDD red. The recovery itself is tested in the crate
//! (`manager::recovery_tests`); the chain against the real PIN service is
//! the smoke. These hold the shape of the binding and the tabs layer's
//! declarations, and what the embedder guide says.

use std::path::Path;

fn workspace_file(rel: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

#[test]
fn the_binding_offers_recovery() {
    let binding = workspace_file("kupferdraht-core/src/wasm/manager.rs");
    for needle in [
        "js_name = recover",
        "js_name = recovery",
        "pub struct Recovery",
        "js_name = enable",
        "js_name = changePin",
        "js_name = verifyPin",
        "js_name = validateNewPin",
        "pin_service_url",
    ] {
        assert!(binding.contains(needle), "wasm/manager.rs: {needle}");
    }
}

#[test]
fn the_tabs_declarations_carry_recovery() {
    let dts = workspace_file("bindings/web/kupferdraht-tabs/index.d.ts");
    for needle in [
        "recover(data: Uint8Array, pin: string, options?: { takeOver?: boolean }): Promise<ClientProxy>;",
        "recovery(): Promise<RecoveryProxy>;",
        "validateNewPin(pin: string): Promise<'ok' | 'tooShort' | 'tooCommon'>;",
        "export class RecoveryProxy",
    ] {
        assert!(dts.contains(needle), "index.d.ts: {needle}");
    }
}

#[test]
fn the_embedder_guide_describes_recovery() {
    let bindings = workspace_file("docs/bindings.md");
    for needle in [
        "recover(data, pin, { takeOver })",
        "validateNewPin",
        "changePin",
        "verifyPin",
        "pinServiceUrl",
        "WrongPin",
        "TooManyAttempts",
        "InvalidRecoveryData",
    ] {
        assert!(bindings.contains(needle), "docs/bindings.md: {needle}");
    }
}
