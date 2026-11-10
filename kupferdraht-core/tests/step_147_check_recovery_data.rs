// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 147: `PendingLogin.checkRecoveryData` on the browser surface. The
//! behaviour is tested beside `recover`'s (`manager/recovery_tests.rs`):
//! recovery is browser-only (ADR-0035), so no FFI mirror and no CLI command.

fn workspace_file(path: &str) -> String {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace");
    std::fs::read_to_string(root.join(path)).expect(path)
}

#[test]
fn the_browser_has_the_call() {
    assert!(
        workspace_file("kupferdraht-core/src/wasm/manager.rs")
            .contains("js_name = checkRecoveryData")
    );
    let protocol = workspace_file("bindings/web/kupferdraht-tabs/protocol.js");
    assert!(protocol.contains(r#"["PendingLogin", "checkRecoveryData", "idempotent", null]"#));
    assert!(
        workspace_file("bindings/web/kupferdraht-tabs/proxy.js")
            .contains("checkRecoveryData(data)")
    );
    let types = workspace_file("bindings/web/kupferdraht-tabs/index.d.ts");
    assert!(types.contains("checkRecoveryData(data: Uint8Array): Promise<RecoveryDataVerdict>;"));
    assert!(types.contains(
        "export type RecoveryDataVerdict = 'ok' | 'notRecoveryData' | 'unsupported' | 'otherAccount' | 'oldDevice';"
    ));
}

#[test]
fn the_docs_say_how() {
    assert!(workspace_file("docs/bindings.md").contains("checkRecoveryData(data)"));
    assert!(
        workspace_file("bindings/web/kupferdraht-tabs/README.md").contains("checkRecoveryData")
    );
    assert!(workspace_file("CHANGELOG.md").contains("checkRecoveryData"));
    assert!(
        workspace_file("docs/adrs/0035-store-browser-databases-on-sqlite-remote-vfs.md")
            .contains("checkRecoveryData")
    );
}
