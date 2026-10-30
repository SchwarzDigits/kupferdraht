// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Shared enumeration of the `#[wasm_bindgen]` surface, for the Step 98
//! sub-steps that pin it.
//!
//! The browser surface is hand-written rather than generated, so "is this
//! capability bound?" is a question about the source text. Parsing it here
//! keeps the sub-step tests to their actual subject and needs no browser,
//! no backend and no MLS state — the behaviour underneath is covered by the
//! native tests of the `ffi/` wrappers these bindings delegate to.
//!
//! `step_94_tab_proxy_parity.rs` keeps its own copy on purpose: it pins the
//! JS mirrors, and a shared helper that drifted would weaken both at once.

use std::collections::BTreeSet;
use std::path::Path;

/// `(class, js_name)` of a JS-facing method.
pub type Member = (String, String);

fn workspace_file(rel: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

pub fn manager_source() -> String {
    workspace_file("kupferdraht-core/src/wasm/manager.rs")
}

/// Every `#[wasm_bindgen]` method per class, using the `js_name` override
/// when present. Getters are skipped — the sub-steps pin methods.
fn wasm_methods(src: &str) -> BTreeSet<Member> {
    let mut methods = BTreeSet::new();
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
            if attr.contains("getter") {
                continue;
            }
            let js_name = attr
                .split("js_name = ")
                .nth(1)
                .map(|rest| {
                    rest.trim_end_matches(")]")
                        .trim_end_matches(',')
                        .to_string()
                })
                .unwrap_or_else(|| rust_name.to_string());
            methods.insert((cls.clone(), js_name));
        } else if !t.starts_with("#[") {
            pending_attr = None;
        }
    }
    methods
}

pub fn surface() -> BTreeSet<Member> {
    wasm_methods(&manager_source())
}

pub fn assert_bound(surface: &BTreeSet<Member>, class: &str, method: &str) {
    assert!(
        surface.contains(&(class.to_string(), method.to_string())),
        "the wasm surface binds {class}.{method}()"
    );
}
