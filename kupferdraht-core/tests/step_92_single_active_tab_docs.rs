// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 92 — the native pins for the one-active-tab contract (ADR-0023):
//! the wasm surface documents takeover and eviction, the smoke page uses
//! the new `Kupferdraht.open` shape, and the error messages the tests match on
//! are the ones the docs name.

use std::path::{Path, PathBuf};

fn workspace_file(rel: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

#[test]
fn bindings_doc_names_the_tab_contract() {
    let doc = workspace_file("docs/bindings.md");
    for needle in [
        "takeOver",
        "onEvicted",
        "already open in another tab",
        "kupferdraht:instance",
    ] {
        assert!(doc.contains(needle), "docs/bindings.md lacks {needle:?}");
    }
}

#[test]
fn smoke_page_opens_with_the_options_object() {
    let worker = workspace_file("bindings/web/opfs-smoke/worker.js");
    assert!(
        worker.contains("Kupferdraht.open({"),
        "the smoke worker must call Kupferdraht.open with the options object"
    );
    assert!(
        worker.contains("takeOver") && worker.contains("evicted"),
        "the smoke page must exercise takeover and report eviction"
    );
}

#[test]
fn browser_lease_name_is_published() {
    let src = workspace_file("kupferdraht-core/src/storage/browser.rs");
    assert!(
        src.contains("pub const BROWSER_INSTANCE_LEASE: &str = \"kupferdraht:instance\";"),
        "the lease name is the pub const ADR-0023 names"
    );
    let _ = PathBuf::new();
}
