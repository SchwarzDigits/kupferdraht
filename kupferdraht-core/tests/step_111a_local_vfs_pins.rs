// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 111a — native pins for the browser storage on sqlite-remote-vfs
//! (ADR-0035). TDD red. The behaviour is pinned in the browser
//! (`step_111a_local_vfs_storage`, `step_111a_eviction_reason`); these hold
//! the build to it: OPFS is gone, the VFS is pinned to v0.1.0, the
//! isolation check exists and the embedder guide names the headers.

use std::path::Path;

fn workspace_file(rel: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

#[test]
fn the_opfs_pool_is_gone() {
    let manifest = workspace_file("kupferdraht-core/Cargo.toml");
    assert!(
        !manifest.contains("sqlite-wasm-vfs"),
        "the OPFS sahpool dependency is gone"
    );
    let browser = workspace_file("kupferdraht-core/src/storage/browser.rs");
    for gone in ["sahpool", "OpfsSAHPool", "BROWSER_STORAGE_CAPACITY"] {
        assert!(!browser.contains(gone), "storage/browser.rs: {gone}");
    }
}

/// Pinned to a reviewed revision: v0.1.0 with Step 111a, v0.1.1
/// (`failure()`, the reason a database stopped) since Step 111b, v0.2.0
/// (`8ecfc4d`: a database heals in place after an outage) since Step 113,
/// v0.3.0 (`fb257c7`: a takeover reported as an event) since Step 119,
/// v0.4.0 (`01688f2`: the account's slot) since Step 120, v0.5.0
/// (`2c6930f`: a commit the server failed internally is resent) since
/// Step 123.
#[test]
fn the_vfs_is_pinned_to_a_reviewed_revision() {
    let manifest = workspace_file("kupferdraht-core/Cargo.toml");
    assert!(
        manifest.contains(
            r#"sqlite-remote-vfs = { git = "https://github.com/SchwarzDigits/sqlite-remote-vfs", rev = "2c6930f" }"#
        ),
        "sqlite-remote-vfs v0.5.0 (a failed commit is resent)"
    );
    assert!(
        workspace_file("deny.toml")
            .contains(r#"allow-git = ["https://github.com/SchwarzDigits/sqlite-remote-vfs"]"#),
        "cargo deny allows that source"
    );
}

#[test]
fn the_storage_requires_cross_origin_isolation() {
    let browser = workspace_file("kupferdraht-core/src/storage/browser.rs");
    assert!(
        browser.contains("crossOriginIsolated"),
        "Kupferdraht.open checks crossOriginIsolated before it takes the lease"
    );
}

#[test]
fn the_embedder_guide_names_the_headers() {
    let doc = workspace_file("docs/bindings.md");
    for needle in [
        "Cross-Origin-Opener-Policy: same-origin",
        "Cross-Origin-Embedder-Policy: require-corp",
        "HeldElsewhere",
        "StorageFull",
    ] {
        assert!(doc.contains(needle), "docs/bindings.md: {needle}");
    }
}
