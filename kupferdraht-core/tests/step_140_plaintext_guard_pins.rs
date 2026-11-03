// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 140 — native pins for the plaintext guard of sqlite-remote-vfs
//! 0.6.0: both `Config` sites ask the VFS to refuse an unencrypted page 1,
//! the failure is mapped to a named storage error, and the doc homes say
//! so. The guard itself is exercised in the browser
//! (`step_140_browser_plaintext_guard`).

use std::path::Path;

fn workspace_file(rel: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

#[test]
fn both_config_sites_refuse_plaintext() {
    for file in [
        "kupferdraht-core/src/storage/remote/server.rs",
        "kupferdraht-core/src/storage/browser.rs",
    ] {
        assert!(
            workspace_file(file).contains("config.refuse_plaintext = true;"),
            "{file}: the VFS config must refuse plaintext (Step 140)"
        );
    }
}

#[test]
fn the_refused_page_is_a_named_storage_error() {
    let browser = workspace_file("kupferdraht-core/src/storage/browser.rs");
    assert!(
        browser.contains("Failure::Plaintext"),
        "browser.rs: check_database maps Failure::Plaintext instead of letting it through"
    );
}

#[test]
fn the_doc_homes_name_the_guard() {
    for (file, needle) in [
        ("docs/ARCHITECTURE.md", "refuse_plaintext"),
        ("docs/bindings.md", "plaintext"),
        ("CHANGELOG.md", "refuse_plaintext"),
    ] {
        assert!(
            workspace_file(file).contains(needle),
            "{file} lacks {needle:?}"
        );
    }
}

#[test]
fn the_browser_test_runs_in_ci() {
    assert!(
        workspace_file(".github/workflows/ci.yml")
            .contains("--test step_140_browser_plaintext_guard"),
        "CI's wasm32 clippy step must list the new browser test"
    );
    assert!(
        workspace_file("scripts/wasm-test.sh").contains("step_140_browser_plaintext_guard"),
        "scripts/wasm-test.sh must run the new browser test"
    );
}
