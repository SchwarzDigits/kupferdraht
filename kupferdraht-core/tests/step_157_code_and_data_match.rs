// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 157: code and data match. The databases' refusals are tested where
//! they are opened (`storage/schema.rs`, `storage/global.rs`,
//! `encryption/migration_tests.rs`), the tab protocol in the browser
//! (`step_94_leader_tab.rs`); this file pins the surfaces and the docs.

use kupferdraht::KupferdrahtError;

fn workspace_file(path: &str) -> String {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace");
    std::fs::read_to_string(root.join(path)).expect(path)
}

/// The FFI carries the error with its database and reason.
#[test]
fn the_error_names_the_database_and_the_reason() {
    let error = KupferdrahtError::IncompatibleStore {
        database: "mls.db".into(),
        reason: "newer".into(),
    };

    let text = error.to_string();

    assert!(text.contains("mls.db") && text.contains("newer"), "{text}");
}

/// The tabs module knows its own version without loading the wasm, and it is
/// the crate's: a release that bumps one and not the other turns CI red.
#[test]
fn the_tabs_module_carries_the_crate_version() {
    let protocol = workspace_file("bindings/web/kupferdraht-tabs/protocol.js");
    let expected = format!(
        "export const KUPFERDRAHT_VERSION = '{}';",
        env!("CARGO_PKG_VERSION")
    );
    assert!(protocol.contains(&expected), "protocol.js lacks {expected}");
}

#[test]
fn the_tab_types_name_the_versions() {
    let types = workspace_file("bindings/web/kupferdraht-tabs/index.d.ts");
    for needle in [
        "export const KUPFERDRAHT_VERSION: string;",
        "export class VersionMismatchError extends Error",
        "readonly version: string;",
        "readonly leaderVersion: string | null;",
        "export type VersionRelation = 'same' | 'newer' | 'older' | 'unknown';",
        "version?: string | (() => string);",
    ] {
        assert!(types.contains(needle), "index.d.ts lacks {needle}");
    }
}

#[test]
fn the_docs_say_how() {
    let readme = workspace_file("bindings/web/kupferdraht-tabs/README.md");
    for needle in [
        "relation",
        "takeOver()",
        "location.reload()",
        "IncompatibleStore",
        "VersionMismatch",
    ] {
        assert!(readme.contains(needle), "the tabs README lacks {needle}");
    }
    let changelog = workspace_file("CHANGELOG.md");
    assert!(changelog.contains("`IncompatibleStore`"));
    assert!(
        changelog.contains("`KUPFERDRAHT_VERSION`"),
        "the release procedure bumps the tabs module's version"
    );
    assert!(workspace_file("docs/library.md").contains("IncompatibleStore"));
    assert!(workspace_file("docs/bindings.md").contains("IncompatibleStore"));
    assert!(
        workspace_file("docs/ARCHITECTURE.md").contains("Step 157"),
        "ARCHITECTURE §7 states the rule"
    );
}
