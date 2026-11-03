// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 139 — native pins for the browser's service tokens (ADR-0036
//! revision 1.1). The purchase, the renewal and the cache are tested in the
//! crate (`token_exchange`, `wire_session`, `service_tokens`); the browser
//! suite pins the answer's shape and the local mode; the Step 94 driver
//! the thin-tab path. These hold the shape of the binding, the tabs
//! layer's mirror, and what the doc homes and the decision record say.

use std::path::Path;

fn workspace_file(rel: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

#[test]
fn the_binding_offers_service_tokens() {
    let binding = workspace_file("kupferdraht-core/src/wasm/manager.rs");
    assert!(
        binding.contains("js_name = serviceToken"),
        "wasm/manager.rs: Client.serviceToken"
    );
    assert!(
        workspace_file("kupferdraht-core/src/wasm/mod.rs").contains("pub mod service_token;"),
        "wasm/mod.rs: the service_token module"
    );
}

#[test]
fn the_tabs_layer_mirrors_service_tokens() {
    let protocol = workspace_file("bindings/web/kupferdraht-tabs/protocol.js");
    assert!(
        protocol.contains(r#"["Client", "serviceToken", "idempotent", null]"#),
        "protocol.js: serviceToken is idempotent — a re-issue to a new leader buys again"
    );
    let proxy = workspace_file("bindings/web/kupferdraht-tabs/proxy.js");
    assert!(
        proxy.contains("serviceToken(audience) { return this.#call('serviceToken', [audience]); }"),
        "proxy.js: serviceToken"
    );
    let dts = workspace_file("bindings/web/kupferdraht-tabs/index.d.ts");
    for needle in [
        "serviceToken(audience: string): Promise<ServiceToken>;",
        "export interface ServiceToken { token: string; expiresIn: number }",
    ] {
        assert!(dts.contains(needle), "index.d.ts: {needle}");
    }
    assert!(
        workspace_file("bindings/web/kupferdraht-tabs/README.md").contains("serviceToken"),
        "kupferdraht-tabs/README.md: serviceToken"
    );
}

#[test]
fn the_embedder_guide_describes_service_tokens() {
    let bindings = workspace_file("docs/bindings.md");
    for needle in [
        "serviceToken(audience)",
        "`{ token, expiresIn }`",
        "reserved",
        "`cnf`",
        "any audience",
    ] {
        assert!(bindings.contains(needle), "docs/bindings.md: {needle}");
    }
    assert!(
        workspace_file("docs/library.md").contains("any audience"),
        "docs/library.md: the StorageAccessDenied row names the exchange's admission for any audience"
    );
    assert!(
        workspace_file("docs/api-mapping-matrix-rust-sdk.md").contains("serviceToken"),
        "api-mapping: the serviceToken row"
    );
    assert!(
        workspace_file("CHANGELOG.md").contains("serviceToken"),
        "CHANGELOG: the Added entry"
    );
    assert!(
        workspace_file("README.md").contains("service tokens"),
        "README: the browser capability"
    );
}

#[test]
fn the_decision_is_recorded() {
    let adr = workspace_file("docs/adrs/0036-admit-storage-connections-with-exchanged-tokens.md");
    for needle in ["Revision 1.1", "reserved", r#"`{"audience": "<name>"}`"#] {
        assert!(adr.contains(needle), "ADR-0036: {needle}");
    }
    assert!(
        workspace_file("rules/0036-admit-storage-connections-with-exchanged-tokens.md")
            .contains("service token"),
        "rules/0036: the service-token rule"
    );
    let threat_model = workspace_file("docs/THREAT_MODEL.md");
    let n8 = threat_model
        .lines()
        .find(|line| line.starts_with("**N8"))
        .expect("THREAT_MODEL.md has N8");
    assert!(
        n8.contains("service token"),
        "THREAT_MODEL N8: service tokens cross the wasm boundary by the embedder's request"
    );
    assert!(
        threat_model.contains("| 0.9 "),
        "THREAT_MODEL.md: revision 0.9"
    );
}

#[test]
fn the_browser_test_runs_in_ci() {
    assert!(
        workspace_file(".github/workflows/ci.yml")
            .contains("--test step_139_browser_service_token"),
        "CI's wasm32 clippy step must list the new browser test"
    );
    assert!(
        workspace_file("scripts/wasm-test.sh").contains("step_139_browser_service_token"),
        "scripts/wasm-test.sh must run the new browser test"
    );
}
