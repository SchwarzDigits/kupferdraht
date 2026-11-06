// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 130b — the native pins for the resend path on the browser surface
//! (TDD red).
//!
//! The browser suite runs in CI only and has no backend there, so the
//! shapes are pinned by reading the sources: the three wasm methods and
//! what they call, their failover class in `protocol.js`, the proxy's
//! forwarding, the types and the doc homes. The resend behaviour itself
//! is the core's, pinned natively in `step_79_failed_send_recovery`.

use std::collections::BTreeMap;
use std::path::Path;

fn workspace_file(rel: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn module_file(name: &str) -> String {
    workspace_file(&format!("bindings/web/kupferdraht-tabs/{name}"))
}

/// The three calls of the Step-79 path, as the browser names them.
const RESEND_CALLS: [&str; 3] = ["failedSends", "resend", "resendFailed"];

/// `Class.method → kind` from `protocol.js`'s `METHODS`.
fn method_kinds() -> BTreeMap<String, String> {
    let src = module_file("protocol.js");
    let start = src
        .find("export const METHODS = Object.freeze([")
        .expect("protocol.js exports METHODS");
    let body = &src[start..];
    let end = body.find("]);").expect("METHODS closes");
    body[..end]
        .lines()
        .filter_map(|l| {
            let inner = l.trim().strip_prefix('[')?.split(']').next()?;
            let cells: Vec<String> = inner
                .split(',')
                .map(|c| c.trim().trim_matches('"').to_string())
                .collect();
            (cells.len() >= 3).then(|| (format!("{}.{}", cells[0], cells[1]), cells[2].clone()))
        })
        .collect()
}

/// The `#[wasm_bindgen]` fn exported as `js_name`, from its attribute to
/// the next attribute: its signature and body.
fn wasm_method(js_name: &str) -> String {
    let src = workspace_file("kupferdraht-core/src/wasm/manager.rs");
    let at = src
        .find(&format!("#[wasm_bindgen(js_name = {js_name})]"))
        .unwrap_or_else(|| panic!("manager.rs exports {js_name}"));
    let rest = &src[at + 1..];
    let end = rest.find("#[wasm_bindgen").unwrap_or(rest.len());
    rest[..end].to_string()
}

#[test]
fn the_three_calls_are_re_issued_at_a_leader_change() {
    let kinds = method_kinds();
    for method in RESEND_CALLS {
        assert_eq!(
            kinds.get(&format!("Client.{method}")).map(String::as_str),
            Some("idempotent"),
            "Client.{method}: a repeat answers like the first call (the same id re-driven, a sent row's handle)"
        );
    }
}

#[test]
fn the_wasm_methods_take_the_room_and_call_the_core_path() {
    let failed = wasm_method("failedSends");
    assert!(
        failed.contains("room_id: String") && failed.contains(".failed_sends()"),
        "failedSends(roomId) reads Room::failed_sends"
    );
    assert!(
        failed.contains("TimelineItemView::project"),
        "failedSends returns the item view messages() returns"
    );
    let resend = wasm_method("resend");
    assert!(
        resend.contains("room_id: String") && resend.contains("message_id: String"),
        "resend(roomId, messageId)"
    );
    assert!(
        resend.contains(".resend(") && resend.contains("message_handle_to_js("),
        "resend re-drives through Timeline::resend and hands back {{ messageId, sentAtMs }}"
    );
    let pass = wasm_method("resendFailed");
    assert!(
        pass.contains("room_id: String") && pass.contains("resend_pass("),
        "resendFailed(roomId) runs the pass that keeps the stopping error"
    );
    assert!(
        pass.contains("resend::summary_view("),
        "resendFailed hands back the summary view"
    );
}

#[test]
fn the_summary_names_the_stopping_error() {
    let view = workspace_file("kupferdraht-core/src/wasm/resend.rs");
    for field in [
        "resent: Vec<",
        "skipped: Vec<String>",
        "stopped_at: Option<",
        "remaining: u32",
        "message_id: String",
        "name: String",
        "message: String",
    ] {
        assert!(view.contains(field), "wasm/resend.rs: {field}");
    }
    let summary = workspace_file("kupferdraht-core/src/timeline/resend.rs");
    assert!(
        summary.contains("pub stopped_at: Option<(String, String)>"),
        "the public ResendSummary keeps its shape (no BREAKING change)"
    );
}

#[test]
fn the_proxy_forwards_the_three_calls() {
    let proxy = module_file("proxy.js");
    for (method, args) in [
        ("failedSends", "[roomId]"),
        ("resend", "[roomId, messageId]"),
        ("resendFailed", "[roomId]"),
    ] {
        let line = proxy
            .lines()
            .find(|l| l.contains(&format!("#call('{method}'")))
            .unwrap_or_else(|| panic!("proxy.js forwards {method}"));
        assert!(line.contains(args), "{method} forwards {args}: {line}");
    }
}

#[test]
fn the_types_declare_the_three_calls() {
    let types = module_file("index.d.ts");
    for (method, signature) in [
        (
            "failedSends",
            "failedSends(roomId: string): Promise<TimelineItem[]>",
        ),
        (
            "resend",
            "resend(roomId: string, messageId: string): Promise<MessageHandle>",
        ),
        (
            "resendFailed",
            "resendFailed(roomId: string): Promise<ResendSummary>",
        ),
    ] {
        assert!(
            types.contains(signature),
            "index.d.ts declares {method}: {signature}"
        );
    }
    assert!(
        types.contains("interface ResendSummary")
            && types.contains("stoppedAt: ResendStop | null")
            && types.contains("interface ResendStop"),
        "the summary and its stop are typed, the stop null when the pass went through"
    );
}

#[test]
fn the_doc_homes_name_the_resend_path() {
    for (file, needle) in [
        ("docs/bindings.md", "resendFailed"),
        ("bindings/web/kupferdraht-tabs/README.md", "failedSends"),
        ("docs/library.md", "130b"),
        ("docs/api-mapping-matrix-rust-sdk.md", "resendFailed"),
        ("CHANGELOG.md", "failedSends"),
        (
            "docs/adrs/0026-leader-tab-serves-thin-client-tabs.md",
            "130b",
        ),
    ] {
        assert!(
            workspace_file(file).contains(needle),
            "{file} names {needle}"
        );
    }
}
