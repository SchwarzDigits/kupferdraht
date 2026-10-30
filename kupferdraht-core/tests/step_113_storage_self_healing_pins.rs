// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 113 — native pins for the self-healing browser storage (ADR-0038).
//! TDD red. The VFS heals a database in place once the storage server
//! answers again; Kupferdraht fails fast meanwhile and reports the state. The
//! account's health is tested in the crate, the tabs broadcast in the
//! browser, the heal against a real storage server in the smoke. These
//! hold what the healing relies on and the surface.

use std::path::Path;

fn workspace_file(rel: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn rust_sources(dir: &Path, out: &mut Vec<(String, String)>) {
    for entry in std::fs::read_dir(dir).expect("read dir") {
        let path = entry.expect("entry").path();
        if path.is_dir() {
            rust_sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            let text = std::fs::read_to_string(&path).expect("read source");
            out.push((path.display().to_string(), text));
        }
    }
}

/// The VFS's healing rolls a failed commit back through its journal: a
/// failed COMMIT means "not committed" only with a journal file. The
/// browser keeps SQLite's default rollback journal — `journal_mode` is set
/// natively only (WAL), never to MEMORY or OFF.
#[test]
fn the_browser_keeps_a_rollback_journal() {
    let mut sources = Vec::new();
    rust_sources(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut sources,
    );
    for (file, text) in sources {
        let lines: Vec<&str> = text.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            let upper = line.to_uppercase();
            if !upper.contains("JOURNAL_MODE") || line.trim_start().starts_with("//") {
                continue;
            }
            assert!(
                !upper.contains("MEMORY") && !upper.contains("\"OFF\""),
                "{file}:{}: a journal mode without a journal file",
                i + 1
            );
            let guarded = lines[i.saturating_sub(3)..i]
                .iter()
                .any(|l| l.contains("#[cfg(not(target_arch = \"wasm32\"))]"));
            assert!(
                guarded,
                "{file}:{}: journal_mode is set natively only",
                i + 1
            );
        }
    }
}

/// With the VFS healing later, a commit that meets an outage gives up after
/// 3 s instead of the default 10 s — the only block an outage causes.
#[test]
fn the_storage_server_connection_gives_up_after_three_seconds() {
    let server = workspace_file("kupferdraht-core/src/storage/remote/server.rs");
    assert!(
        server.contains("server.reconnect_timeout = Duration::from_secs(3)"),
        "storage/remote/server.rs sets the reconnect timeout"
    );
}

#[test]
fn both_databases_are_gated() {
    let store = workspace_file("kupferdraht-core/src/storage/sqlite/mod.rs");
    let provider = workspace_file("kupferdraht-core/src/encryption/provider.rs");
    for (file, text) in [
        ("storage/sqlite/mod.rs", &store),
        ("encryption/provider.rs", &provider),
    ] {
        assert!(
            text.contains("check_database("),
            "{file}: every operation checks the database's storage first"
        );
    }
}

#[test]
fn the_binding_reports_the_storage_state() {
    let binding = workspace_file("kupferdraht-core/src/wasm/manager.rs");
    for needle in [
        "on_storage: Option<js_sys::Function>",
        "js_name = storageState",
    ] {
        assert!(binding.contains(needle), "wasm/manager.rs: {needle}");
    }
}

#[test]
fn the_tabs_layer_carries_the_storage_state() {
    let protocol = workspace_file("bindings/web/kupferdraht-tabs/protocol.js");
    for needle in ["'storage'", r#"["Client", "storageState"]"#] {
        assert!(protocol.contains(needle), "protocol.js: {needle}");
    }
    let dts = workspace_file("bindings/web/kupferdraht-tabs/index.d.ts");
    for needle in [
        "onStorage?: (state: 'online' | 'reconnecting') => void",
        "readonly storageState: 'online' | 'reconnecting';",
        "'storage'",
    ] {
        assert!(dts.contains(needle), "index.d.ts: {needle}");
    }
}

#[test]
fn the_embedder_guide_describes_the_self_healing_storage() {
    let bindings = workspace_file("docs/bindings.md");
    for needle in ["onStorage", "storageState", "\"reconnecting\"", "ADR-0038"] {
        assert!(bindings.contains(needle), "docs/bindings.md: {needle}");
    }
}
