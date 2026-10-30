// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 125 — the browser `Client` answers the account's handle.
//!
//! `setHandle` writes it and nothing reads it back, so an embedder whose
//! session type carries a handle keeps a `GET /self` of its own alive for
//! one string the engine already holds (`Account::handle`, which
//! `accounts()` lists today).
//!
//! These are source and doc pins, in the idiom of
//! `step_94_tab_proxy_parity.rs`. That test pins the surface/mirror pair
//! itself, from both sides — which class a getter belongs to is its
//! assertion, not this file's.

use std::path::Path;

fn workspace_file(rel: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn module_file(name: &str) -> String {
    workspace_file(&format!("bindings/web/kupferdraht-tabs/{name}"))
}

#[test]
fn the_wasm_surface_exports_the_handle_getter() {
    assert!(
        workspace_file("kupferdraht-core/src/wasm/manager.rs")
            .contains("pub fn handle(&self) -> Option<String>"),
        "the wasm Client does not give the account's handle back"
    );
}

#[test]
fn the_tab_mirror_carries_the_handle_getter() {
    assert!(
        module_file("protocol.js").contains(r#"["Client", "handle"]"#),
        "protocol.js has no GETTERS row for Client.handle"
    );
    assert!(
        module_file("proxy.js").contains("get handle()"),
        "proxy.js does not define the handle getter"
    );
    assert!(
        module_file("index.d.ts").contains("readonly handle"),
        "index.d.ts does not declare readonly handle"
    );
}

/// The bug this step walked into, pinned as a class rather than as one name.
///
/// `HandleProxy` stores the leader-assigned routing id of a proxy, and it
/// used to store it as `this.handle`. A mirrored getter named `handle` then
/// shadowed that field with a getter-only accessor, and the constructor's
/// own assignment threw `TypeError: setting getter-only property "handle"`
/// — every tab-proxied session died at `restoreLastSession`. The parity
/// test could not see it (both sides had the member); a browser found it in
/// one run. So: no proxy may assign an instance field whose name is a
/// mirrored getter. The wire format calls the routing id `$handle`, which is
/// what it is now called on the proxy too.
#[test]
fn no_proxy_field_collides_with_a_mirrored_getter() {
    let proxy = module_file("proxy.js");
    let protocol = module_file("protocol.js");
    for name in getter_names(&protocol) {
        let collision = format!("this.{name} =");
        assert!(
            !proxy.contains(&collision),
            "proxy.js assigns `{collision}` while `{name}` is a mirrored getter — \
             the getter shadows the field and the assignment throws"
        );
    }
}

/// The getter names of `protocol.js`'s `GETTERS` table.
fn getter_names(protocol: &str) -> Vec<String> {
    protocol
        .lines()
        .skip_while(|l| !l.contains("GETTERS"))
        .take_while(|l| !l.contains("]);"))
        .filter_map(|l| l.split('"').nth(3).map(str::to_owned))
        .collect()
}

#[test]
fn the_doc_homes_record_the_step() {
    for home in ["docs/bindings.md", "docs/api-mapping-matrix-rust-sdk.md"] {
        assert!(
            workspace_file(home).contains("Step 125"),
            "{home} does not name Step 125"
        );
    }
}
