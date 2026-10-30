// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 111b — native pins for the browser's server mode (ADR-0035,
//! ADR-0036). TDD red. The pieces are unit-tested in the crate
//! (`storage::remote`); the chain is proven by the smoke on the test stage.
//! These hold the entry point and the rules to it: the addresses come from
//! the embedder's configuration only, the options take the server mode,
//! and the embedder guide describes it.

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
        } else if path.extension().is_some_and(|e| e == "rs" || e == "js") {
            let text = std::fs::read_to_string(&path).expect("read source");
            out.push((path.display().to_string(), text));
        }
    }
}

/// Other stages have other addresses: no storage server, token exchange or
/// PIN service address is built into the code or the bindings.
#[test]
fn no_service_address_is_built_in() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut sources = Vec::new();
    rust_sources(&root.join("kupferdraht-core/src"), &mut sources);
    rust_sources(&root.join("bindings/web"), &mut sources);
    for (path, text) in sources {
        for needle in [
            "odj.cloud",
            "natrium-token-exchange.",
            "sqlite-remote-server.",
        ] {
            assert!(
                !text.contains(needle),
                "{path} names a service address ({needle})"
            );
        }
    }
}

#[test]
fn kupferdraht_open_takes_the_server_mode_from_the_options() {
    let src = workspace_file("kupferdraht-core/src/wasm/manager.rs");
    for needle in [
        "Server {",
        "token_exchange_url",
        "#[serde(rename_all = \"camelCase\")]",
    ] {
        assert!(src.contains(needle), "wasm StorageOptions: {needle}");
    }
}

#[test]
fn the_embedder_guide_describes_the_server_mode() {
    let doc = workspace_file("docs/bindings.md");
    for needle in [
        "mode: \"server\"",
        "tokenExchangeUrl",
        "NothingStored",
        "StorageAccessDenied",
        "ServiceUnavailable",
        "onEvicted(\"access\")",
    ] {
        assert!(doc.contains(needle), "docs/bindings.md: {needle}");
    }
}
