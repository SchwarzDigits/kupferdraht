// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 94 — the native pins for the leader-tab module (ADR-0026).
//!
//! **Proxy parity** is the JS twin of the FFI-parity rule: every public
//! method and getter the `#[wasm_bindgen]` classes in `kupferdraht-core/src/wasm/`
//! export must be mirrored, name for name, by `bindings/web/kupferdraht-tabs/`
//! — its method table (`protocol.js`), its proxy (`proxy.js`) and its
//! hand-written types (`index.d.ts`). The table is also checked the other
//! way round (no phantom methods). **Preservation** pins that the
//! single-tab path is untouched: `Kupferdraht.open`'s shape, the smoke worker's
//! direct open, and no reference from the core to the module. The
//! **doc pins** name the doc homes the step must update.

use std::collections::BTreeSet;
use std::path::Path;

fn workspace_file(rel: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// `(class, js_name)` of a method or getter.
type Member = (String, String);

/// Enumerate the JS-facing members of every `#[wasm_bindgen] impl` block
/// in `src`: the `js_name` of the preceding attribute, else the Rust name.
/// Getters land in the second set. Static entry points count as methods
/// (the caller excludes the leader-only ones explicitly).
fn wasm_bindgen_members(src: &str) -> (BTreeSet<Member>, BTreeSet<Member>) {
    let mut methods = BTreeSet::new();
    let mut getters = BTreeSet::new();
    let mut class: Option<String> = None;
    let mut pending_attr: Option<String> = None;
    let lines: Vec<&str> = src.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        if line.starts_with("impl ") && i > 0 && lines[i - 1].trim() == "#[wasm_bindgen]" {
            class = line
                .trim_start_matches("impl ")
                .trim_end_matches(" {")
                .split_whitespace()
                .next()
                .map(str::to_string);
            continue;
        }
        if line.starts_with('}') {
            class = None;
            continue;
        }
        let Some(cls) = &class else { continue };
        let t = line.trim();
        if t.starts_with("#[wasm_bindgen(") {
            pending_attr = Some(t.to_string());
            continue;
        }
        if t.starts_with("pub fn ") || t.starts_with("pub async fn ") {
            let rust_name = t
                .trim_start_matches("pub async fn ")
                .trim_start_matches("pub fn ")
                .split(['(', '<'])
                .next()
                .expect("fn name");
            let attr = pending_attr.take().unwrap_or_default();
            let js_name = attr
                .split("js_name = ")
                .nth(1)
                .map(|rest| {
                    rest.trim_end_matches(")]")
                        .trim_end_matches(',')
                        .to_string()
                })
                .unwrap_or_else(|| rust_name.to_string());
            let member = (cls.clone(), js_name);
            if attr.contains("getter") {
                getters.insert(member);
            } else {
                methods.insert(member);
            }
        } else if !t.starts_with("#[") {
            pending_attr = None;
        }
    }
    (methods, getters)
}

/// The wasm surface: the five classes across the two binding files.
fn wasm_surface() -> (BTreeSet<Member>, BTreeSet<Member>) {
    let (mut methods, mut getters) =
        wasm_bindgen_members(&workspace_file("kupferdraht-core/src/wasm/manager.rs"));
    let (m2, g2) = wasm_bindgen_members(&workspace_file("kupferdraht-core/src/wasm/live_sync.rs"));
    methods.extend(m2);
    getters.extend(g2);
    // The leader-only entry point: `Kupferdraht.open` runs in the elected
    // worker (the lease hand-off, ADR-0026 L4); a thin tab never opens.
    assert!(
        methods.remove(&("Kupferdraht".to_string(), "open".to_string())),
        "the wasm surface has Kupferdraht.open"
    );
    (methods, getters)
}

/// Parse one `export const NAME = Object.freeze([ … ]);` array of string
/// tuples from `protocol.js` into `(class, name, rest…)` rows.
fn protocol_rows(src: &str, name: &str) -> Vec<Vec<String>> {
    let start = src
        .find(&format!("export const {name} = Object.freeze(["))
        .unwrap_or_else(|| panic!("protocol.js exports {name}"));
    let body = &src[start..];
    let end = body.find("]);").expect("array closes");
    body[..end]
        .lines()
        .filter_map(|l| {
            let t = l.trim();
            let inner = t.strip_prefix('[')?.split(']').next()?;
            let cells: Vec<String> = inner
                .split(',')
                .map(|c| c.trim().trim_matches('"').to_string())
                .collect();
            (cells.len() >= 2).then_some(cells)
        })
        .collect()
}

fn module_file(name: &str) -> String {
    workspace_file(&format!("bindings/web/kupferdraht-tabs/{name}"))
}

#[test]
fn every_wasm_method_and_getter_has_a_proxy_twin() {
    let (methods, getters) = wasm_surface();
    assert!(
        methods.len() > 20 && getters.len() >= 5,
        "the enumeration found the surface"
    );
    let protocol = module_file("protocol.js");
    let table: BTreeSet<Member> = protocol_rows(&protocol, "METHODS")
        .iter()
        .map(|r| (r[0].clone(), r[1].clone()))
        .collect();
    let table_getters: BTreeSet<Member> = protocol_rows(&protocol, "GETTERS")
        .iter()
        .map(|r| (r[0].clone(), r[1].clone()))
        .collect();
    let missing: Vec<_> = methods.difference(&table).collect();
    assert!(missing.is_empty(), "protocol.js METHODS lacks {missing:?}");
    let missing: Vec<_> = getters.difference(&table_getters).collect();
    assert!(missing.is_empty(), "protocol.js GETTERS lacks {missing:?}");
    let phantom: Vec<_> = table.difference(&methods).collect();
    assert!(
        phantom.is_empty(),
        "protocol.js METHODS names non-existent {phantom:?}"
    );
    let phantom: Vec<_> = table_getters.difference(&getters).collect();
    assert!(
        phantom.is_empty(),
        "protocol.js GETTERS names non-existent {phantom:?}"
    );
}

#[test]
fn every_method_is_classified_and_typed() {
    let protocol = module_file("protocol.js");
    for row in protocol_rows(&protocol, "METHODS") {
        assert!(
            matches!(row[2].as_str(), "idempotent" | "mutation" | "subscription"),
            "{}.{} has kind {:?}",
            row[0],
            row[1],
            row[2]
        );
        assert!(
            matches!(
                row[3].as_str(),
                "null" | "PendingLogin" | "Client" | "Recovery" | "SyncHandle"
            ),
            "{}.{} returns {:?}",
            row[0],
            row[1],
            row[3]
        );
    }
}

#[test]
fn proxy_and_types_define_every_member() {
    let protocol = module_file("protocol.js");
    let proxy = module_file("proxy.js");
    let dts = module_file("index.d.ts");
    for row in protocol_rows(&protocol, "METHODS") {
        let name = &row[1];
        assert!(
            proxy.contains(&format!("\n  {name}("))
                || proxy.contains(&format!("\n  async {name}(")),
            "proxy.js does not define {}.{name}",
            row[0]
        );
        assert!(
            dts.contains(&format!("{name}(")),
            "index.d.ts does not declare {name}"
        );
    }
    for row in protocol_rows(&protocol, "GETTERS") {
        let name = &row[1];
        assert!(
            proxy.contains(&format!("get {name}()")),
            "proxy.js lacks getter {name}"
        );
        assert!(
            dts.contains(&format!("readonly {name}")),
            "index.d.ts lacks readonly {name}"
        );
    }
}

#[test]
fn single_tab_path_is_preserved() {
    let manager = workspace_file("kupferdraht-core/src/wasm/manager.rs");
    assert!(
        manager.contains(
            "pub async fn open(\n        options: JsValue,\n        on_evicted: Option<js_sys::Function>,\n    ) -> Result<Kupferdraht, JsValue>"
        ),
        "Kupferdraht.open keeps its Step-92 shape (Step 110: it throws a named JS Error)"
    );
    let worker = workspace_file("bindings/web/opfs-smoke/worker.js");
    assert!(
        // Step 111b added the `storage` option after `takeOver`.
        worker.contains("Kupferdraht.open({ permitHttp: true, takeOver"),
        "the single-tab smoke worker still opens directly, without the module"
    );
    let core = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut stack = vec![core];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("read src") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                stack.push(path);
            } else if std::fs::read_to_string(&path).is_ok_and(|s| s.contains("kupferdraht-tabs")) {
                panic!("{} references the opt-in module", path.display());
            }
        }
    }
    for file in [
        "index.js",
        "host.js",
        "worker.js",
        "proxy.js",
        "leader.js",
        "protocol.js",
    ] {
        assert!(
            !module_file(file).contains("pkg/kupferdraht"),
            "{file} must not hard-wire the wasm package location — the embedder's worker loads it"
        );
    }
}

#[test]
fn doc_homes_name_the_multi_tab_contract() {
    let bindings = workspace_file("docs/bindings.md");
    for needle in [
        "kupferdraht-tabs",
        "LeaderChanged",
        "onSync",
        "leader-changed",
        "kupferdraht:tabs",
    ] {
        assert!(
            bindings.contains(needle),
            "docs/bindings.md lacks {needle:?}"
        );
    }
    assert!(
        !bindings.contains("No read-only second tab in V1"),
        "docs/bindings.md still states the pre-ADR-0026 limitation"
    );
    assert!(
        workspace_file("README.md").contains("kupferdraht-tabs"),
        "README Platforms lacks the module"
    );
    assert!(
        workspace_file("docs/ARCHITECTURE.md").contains("kupferdraht-tabs"),
        "ARCHITECTURE lacks the browser leader/thin-tab flow"
    );
    assert!(
        workspace_file("docs/adrs/0023-single-active-kupferdraht-instance-per-origin.md")
            .contains("ADR-0026"),
        "ADR-0023's revision history must point at ADR-0026"
    );
    assert!(
        workspace_file("docs/api-mapping-matrix-rust-sdk.md").contains("onSync"),
        "api-mapping lacks the startSync onSync row"
    );
    assert!(
        workspace_file(".github/workflows/ci.yml").contains("--test step_94_leader_tab"),
        "CI's wasm32 clippy step must list the new browser test"
    );
    assert!(
        workspace_file("scripts/wasm-test.sh").contains("step_94_leader_tab"),
        "scripts/wasm-test.sh must run the new browser test"
    );
}
