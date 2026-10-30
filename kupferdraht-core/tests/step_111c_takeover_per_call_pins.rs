// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 111c — native pins for the takeover per call and the tabs layer in
//! the server mode (ADR-0035, ADR-0026). TDD red. The login's deferral is
//! unit-tested (`manager::seed_login_tests`), the tabs layer in the browser
//! (`step_94_leader_tab`); these hold the option shapes of the binding and
//! the `.d.ts`, and what the embedder guides say.

use std::path::Path;

fn workspace_file(rel: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// The takeover of another browser's databases is the opening call's:
/// `resume` takes options, `registerDevice`'s options carry `takeOver`.
#[test]
fn resume_and_register_device_take_the_takeover() {
    let binding = workspace_file("kupferdraht-core/src/wasm/manager.rs");
    assert!(
        binding.contains("pub async fn resume(&self, account_id: String, options: JsValue)"),
        "Kupferdraht.resume(accountId, options)"
    );
    // Step 112b: `recover` takes the same `{ takeOver }` options.
    for needle in ["struct TakeOverOptions", "take_over: bool"] {
        assert!(binding.contains(needle), "wasm/manager.rs: {needle}");
    }
}

/// `Kupferdraht.open`'s `takeOver` is the tab hand-off's: a server database
/// registers with the takeover of the call that opens the account.
#[test]
fn the_server_registration_does_not_take_over_with_the_tab_lease() {
    let server = workspace_file("kupferdraht-core/src/storage/remote/server.rs");
    assert!(
        !server.contains("storage_take_over"),
        "the server mode takes no takeover from Kupferdraht.open"
    );
}

#[test]
fn the_tabs_declarations_carry_the_new_options() {
    let dts = workspace_file("bindings/web/kupferdraht-tabs/index.d.ts");
    for needle in [
        "resume(accountId: string, options?: { takeOver?: boolean }): Promise<ClientProxy>;",
        "registerDevice(options?: { newSetup?: boolean; takeOver?: boolean }): Promise<ClientProxy>;",
        // Step 112b added the optional `pinServiceUrl`.
        "storage?: { mode: 'local' } | { mode: 'server'; url: string; tokenExchangeUrl: string; pinServiceUrl?: string };",
        "'evicted': CustomEvent<{ epoch: number; reason: 'tab' | 'device' | 'access' }>;",
    ] {
        assert!(dts.contains(needle), "index.d.ts: {needle}");
    }
}

#[test]
fn the_embedder_guides_describe_the_takeover_per_call() {
    let bindings = workspace_file("docs/bindings.md");
    for needle in [
        "resume(accountId, { takeOver })",
        "registerDevice({ newSetup, takeOver })",
        "never another browser's databases",
    ] {
        assert!(bindings.contains(needle), "docs/bindings.md: {needle}");
    }
    let tabs = workspace_file("bindings/web/kupferdraht-tabs/README.md");
    for needle in ["storage", "reason"] {
        assert!(
            tabs.contains(needle),
            "kupferdraht-tabs/README.md: {needle}"
        );
    }
}
