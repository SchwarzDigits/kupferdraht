// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 130a — the native pins for the keyed surface (TDD red).
//!
//! The browser half of the step cannot run here (the browser suite is
//! CI-only and has no backend there), so the shapes are pinned by
//! reading the sources: the three failover classes of `protocol.js` and
//! the table the tabs README publishes, the trailing `options` of the six
//! keyed wasm methods and the key on their handle, the proxy adding a key
//! when the caller gave none, the types, the FFI mirror, the CLI flag,
//! the V32 digest pin and the doc homes.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

fn workspace_file(rel: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn module_file(name: &str) -> String {
    workspace_file(&format!("bindings/web/kupferdraht-tabs/{name}"))
}

/// The six calls a key makes safe to re-issue.
const KEYED: [&str; 6] = [
    "sendText",
    "sendReply",
    "sendKnock",
    "sendLocation",
    "sendFile",
    "toggleReaction",
];

/// Mutations a repeat converges and answers like the first call — re-issued.
const NOW_IDEMPOTENT: [&str; 10] = [
    "markAsRead",
    "markAsUnread",
    "startTyping",
    "stopTyping",
    "redact",
    "createDm",
    "renameRoom",
    "setRoomAccess",
    "allowGuests",
    "setDisplayName",
];

/// Mutations that keep rejecting `LeaderChanged`: a repeat after a call
/// that landed rejects, or the call is not safe to repeat at all.
const STAY_MUTATION: [&str; 11] = [
    "invite",
    "kick",
    "leave",
    "deleteRoom",
    "revokeJoinLink",
    "setHandle",
    "createRoom",
    "editMessage",
    "createJoinLink",
    "joinViaLink",
    "updateEmail",
];

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

#[test]
fn protocol_classifies_the_three_classes() {
    let kinds = method_kinds();
    for method in KEYED {
        assert_eq!(
            kinds.get(&format!("Client.{method}")).map(String::as_str),
            Some("keyed"),
            "Client.{method}"
        );
    }
    for method in NOW_IDEMPOTENT {
        assert_eq!(
            kinds.get(&format!("Client.{method}")).map(String::as_str),
            Some("idempotent"),
            "Client.{method} converges on a repeat and is re-issued"
        );
    }
    for method in STAY_MUTATION {
        assert_eq!(
            kinds.get(&format!("Client.{method}")).map(String::as_str),
            Some("mutation"),
            "Client.{method} keeps rejecting LeaderChanged"
        );
    }
    let keyed: BTreeSet<&str> = kinds
        .iter()
        .filter(|(_, kind)| kind.as_str() == "keyed")
        .map(|(name, _)| name.as_str())
        .collect();
    let expected: BTreeSet<String> = KEYED.iter().map(|m| format!("Client.{m}")).collect();
    assert_eq!(
        keyed,
        expected.iter().map(String::as_str).collect(),
        "exactly the six sends are keyed"
    );
    for method in [
        "login",
        "finalizeSso",
        "registerDevice",
        "recover",
        "cancel",
    ] {
        let row = kinds
            .iter()
            .find(|(name, _)| name.ends_with(&format!(".{method}")))
            .unwrap_or_else(|| panic!("{method} is in METHODS"));
        assert_eq!(row.1, "mutation", "{method}: setup stays a mutation");
    }
    assert_eq!(
        kinds.get("Client.logout").map(String::as_str),
        Some("mutation")
    );
    assert_eq!(
        kinds.get("Client.deleteDevice").map(String::as_str),
        Some("mutation")
    );
}

/// `kind → {Class.method}` from the tabs README's failover table: rows
/// `| <kind> | `Class.method`, … |`.
fn readme_classes() -> BTreeMap<String, BTreeSet<String>> {
    let readme = module_file("README.md");
    let mut classes = BTreeMap::new();
    for line in readme.lines() {
        let cells: Vec<&str> = line.split('|').map(str::trim).collect();
        if cells.len() < 4 {
            continue;
        }
        let kind = cells[1];
        if !matches!(kind, "keyed" | "idempotent" | "mutation" | "subscription") {
            continue;
        }
        let names: BTreeSet<String> = cells[2]
            .split(',')
            .map(|n| n.trim().trim_matches('`').to_string())
            .filter(|n| !n.is_empty())
            .collect();
        classes.insert(kind.to_string(), names);
    }
    classes
}

#[test]
fn the_tabs_readme_publishes_the_class_table() {
    let published = readme_classes();
    let mut expected: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (name, kind) in method_kinds() {
        expected.entry(kind).or_default().insert(name);
    }
    assert_eq!(
        published, expected,
        "the README's failover table is METHODS, class by class"
    );
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
fn the_keyed_wasm_methods_take_options_and_hand_back_the_key() {
    for method in KEYED {
        let body = wasm_method(method);
        let signature = &body[..body.find("-> Result").expect("signature")];
        assert!(
            signature.contains("options: JsValue"),
            "{method} takes a trailing options object"
        );
        assert!(
            body.contains("send_keyed("),
            "{method} sends through Timeline::send_keyed"
        );
        assert!(
            body.contains("keyed_handle_to_js("),
            "{method} hands back {{ messageId, sentAtMs, idempotencyKey }}"
        );
    }
}

#[test]
fn the_proxy_adds_a_key_to_every_keyed_call() {
    let proxy = module_file("proxy.js");
    assert!(
        proxy.contains("function withKey(") && proxy.contains("crypto.randomUUID()"),
        "proxy.js generates a key when the caller gave none"
    );
    for method in KEYED {
        let line = proxy
            .lines()
            .find(|l| l.contains(&format!("#call('{method}'")))
            .unwrap_or_else(|| panic!("proxy.js forwards {method}"));
        assert!(
            line.contains("withKey(options)"),
            "{method}: the proxy forwards the options with a key: {line}"
        );
    }
}

#[test]
fn the_failover_re_issues_keyed_calls() {
    let proxy = module_file("proxy.js");
    let start = proxy.find("#failOver() {").expect("proxy.js has #failOver");
    let body = &proxy[start..start + proxy[start..].find("\n  }\n").expect("#failOver ends")];
    assert!(
        body.contains("entry.kind === 'mutation'"),
        "a mutation in flight still rejects"
    );
    for line in body.lines().filter(|l| l.contains("entry.reject(")) {
        assert!(
            line.contains("'mutation'") || line.contains("'subscription'"),
            "only a mutation or a subscription rejects at a leader change: {line}"
        );
    }
    let protocol = module_file("protocol.js");
    assert!(
        protocol.contains("`keyed`"),
        "protocol.js documents the keyed class"
    );
}

#[test]
fn the_types_carry_the_options_and_the_keyed_handle() {
    let types = module_file("index.d.ts");
    assert!(
        types.contains("idempotencyKey?: string"),
        "SendOptions.idempotencyKey"
    );
    assert!(
        types.contains("interface KeyedMessageHandle"),
        "the keyed handle type"
    );
    for method in KEYED {
        let line = types
            .lines()
            .find(|l| l.trim_start().starts_with(&format!("{method}(")))
            .unwrap_or_else(|| panic!("index.d.ts declares {method}"));
        assert!(
            line.contains("options?: SendOptions") && line.contains("Promise<KeyedMessageHandle>"),
            "{method}: {line}"
        );
    }
}

#[test]
fn the_ffi_timeline_mirrors_send_keyed() {
    let ffi = workspace_file("kupferdraht-core/src/ffi/timeline.rs");
    assert!(
        ffi.contains("pub async fn send_keyed("),
        "the FFI Timeline mirrors send_keyed"
    );
}

#[test]
fn the_cli_sends_with_a_key() {
    let main = workspace_file("kupferdraht-cli/src/main.rs");
    assert!(
        main.matches("idempotency_key").count() >= 2,
        "send and react take --idempotency-key"
    );
    let timeline = workspace_file("kupferdraht-cli/src/cli/timeline.rs");
    assert!(
        timeline.contains("send_keyed("),
        "the CLI sends keyed calls through Timeline::send_keyed"
    );
}

#[test]
fn the_v32_migration_is_pinned() {
    let pins = workspace_file("kupferdraht-core/tests/migration_immutability.rs");
    assert!(
        pins.contains("V32__step_130a_send_keys.sql"),
        "V32 is pinned by its digest"
    );
    let sql =
        workspace_file("kupferdraht-core/src/storage/migrations/V32__step_130a_send_keys.sql");
    assert!(sql.contains("CREATE TABLE send_keys"));
}

#[test]
fn the_doc_homes_name_the_keys() {
    let adr = workspace_file("docs/adrs/0026-leader-tab-serves-thin-client-tabs.md");
    assert!(
        adr.contains("130a") && adr.contains("keyed"),
        "ADR-0026 has a revision for the keyed class"
    );
    let rule = workspace_file("rules/0026-leader-tab-serves-thin-client-tabs.md");
    assert!(rule.contains("keyed"), "rules/0026 names the keyed class");
    for (file, needle) in [
        ("docs/bindings.md", "idempotencyKey"),
        ("docs/library.md", "send_keyed"),
        ("docs/cli.md", "--idempotency-key"),
        ("docs/ARCHITECTURE.md", "send_keys"),
        ("docs/api-mapping-matrix-rust-sdk.md", "send_keyed"),
        ("CHANGELOG.md", "send_keyed"),
    ] {
        assert!(
            workspace_file(file).contains(needle),
            "{file} names {needle}"
        );
    }
}
