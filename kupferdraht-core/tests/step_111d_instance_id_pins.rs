// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 111d — a native pin for the stable VFS instance id (ADR-0035).
//! TDD red. The derivation is unit-tested (`storage::remote::instance_id`);
//! a reload that takes its lease back is the smoke. This holds the wiring:
//! the server registration sets the id instead of a random one.

use std::path::Path;

fn workspace_file(rel: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

#[test]
fn the_server_registration_sets_the_stable_instance_id() {
    let server = workspace_file("kupferdraht-core/src/storage/remote/server.rs");
    for needle in ["storage_instance_id(", "server.instance_id = Some("] {
        assert!(
            server.contains(needle),
            "storage/remote/server.rs: {needle}"
        );
    }
}

/// The embedder guide no longer tells the embedder to wait out its own
/// lease after a reload.
#[test]
fn the_embedder_guide_drops_the_reload_wait() {
    let bindings = workspace_file("docs/bindings.md");
    assert!(
        !bindings.contains("right after a reload or a crash the page's own"),
        "docs/bindings.md still describes the 30 s wait after a reload"
    );
    assert!(
        bindings.contains("takes its own lease back"),
        "docs/bindings.md: a reload takes its own lease back"
    );
}
