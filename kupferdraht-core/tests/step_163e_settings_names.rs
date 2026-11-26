// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 163e — the server's settings, the account data and the store contract
//! in plain names (TDD red).
//!
//! The words themselves are held by `public_vocabulary`. These pins hold what
//! a lexical check cannot see: the shapes of the records a consumer reads (a
//! setting, an account-data entry, the store's rows), the code names behind
//! the public ones, and that every binding, the CLI and the error texts follow
//! the core.

use std::path::{Path, PathBuf};

use kupferdraht::KupferdrahtError;
use kupferdraht::storage::{
    AccountDataEntry, DeliveryReceiptRecord, ExternalSenderKey, OwnUserRecord, ServerSetting,
    SyncStateRecord,
};
use serde_json::json;

fn workspace_file(rel: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// Every `.rs` file under `dir`, recursively.
fn rust_sources(dir: &Path, found: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read source dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            rust_sources(&path, found);
        } else if path.extension().is_some_and(|e| e == "rs") {
            found.push(path);
        }
    }
}

/// `(path, source)` of every Rust file under `src/` outside `wire/`.
fn core_sources() -> Vec<(PathBuf, String)> {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_sources(&src, &mut files);
    files
        .into_iter()
        .filter(|path| !path.starts_with(src.join("wire")))
        .map(|path| {
            let source = std::fs::read_to_string(&path).expect("read source");
            (path, source)
        })
        .collect()
}

/// Where the raw string literal (`r"…"`, `r#"…"#`) starting at `i` ends, or
/// `None` when none starts there.
fn raw_string_end(chars: &[char], i: usize) -> Option<usize> {
    let starts_word = i == 0 || !(chars[i - 1].is_alphanumeric() || chars[i - 1] == '_');
    let after_b = i > 0 && chars[i - 1] == 'b' && (i == 1 || !chars[i - 2].is_alphanumeric());
    if chars[i] != 'r' || !(starts_word || after_b) {
        return None;
    }
    let hashes = chars[i + 1..].iter().take_while(|c| **c == '#').count();
    let open = i + 1 + hashes;
    if chars.get(open) != Some(&'"') {
        return None;
    }
    let close: String = std::iter::once('"')
        .chain(std::iter::repeat_n('#', hashes))
        .collect();
    let rest: String = chars[open + 1..].iter().collect();
    let at = rest.find(&close)?;
    Some(open + 1 + rest[..at].chars().count() + close.chars().count())
}

/// `source` without its comments and without the contents of its string and
/// character literals: the names the code itself uses. A string literal
/// keeps its quotes, so SQL column names and the backend's query
/// parameters drop out.
fn code_names(source: &str) -> String {
    let chars: Vec<char> = source.chars().collect();
    let mut out = String::with_capacity(source.len());
    let mut i = 0;
    while i < chars.len() {
        if let Some(end) = raw_string_end(&chars, i) {
            out.push_str("\"\"");
            i = end;
            continue;
        }
        match chars[i] {
            '/' if chars.get(i + 1) == Some(&'/') => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            '"' => {
                out.push('"');
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    i += if chars[i] == '\\' { 2 } else { 1 };
                }
                out.push('"');
                i += 1;
            }
            '\'' if chars.get(i + 2) == Some(&'\'') || chars.get(i + 1) == Some(&'\\') => {
                i += 1;
                while i < chars.len() && chars[i] != '\'' {
                    i += if chars[i] == '\\' { 2 } else { 1 };
                }
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

/// Every core file outside `wire/` whose code names `name`.
fn files_naming(name: &str) -> Vec<String> {
    core_sources()
        .into_iter()
        .filter(|(_, source)| code_names(source).contains(name))
        .map(|(path, _)| path.display().to_string())
        .collect()
}

/// The body of `pub <kind> <name> {` in `rel`, up to its closing brace at
/// the start of a line.
fn item_body(rel: &str, kind: &str, name: &str) -> String {
    let source = workspace_file(rel);
    let opener = format!("pub {kind} {name} {{");
    let at = source
        .find(&opener)
        .unwrap_or_else(|| panic!("{rel} has no `{opener}`"));
    let body = &source[at + opener.len()..];
    body[..body.find("\n}").expect("item closes")].to_owned()
}

/// The `pub` fields of `pub struct <name>` in `rel`, in order.
fn public_fields(rel: &str, name: &str) -> Vec<String> {
    item_body(rel, "struct", name)
        .lines()
        .map(str::trim)
        .filter_map(|line| line.strip_prefix("pub "))
        .filter_map(|line| line.split_once(':').map(|(field, _)| field.to_owned()))
        .collect()
}

/// The variants of `pub enum <name>` in `rel`, in order.
fn variants(rel: &str, name: &str) -> Vec<String> {
    item_body(rel, "enum", name)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with("//") && !line.starts_with('#'))
        .filter_map(|line| {
            let end = line
                .find(|c: char| !(c.is_alphanumeric() || c == '_'))
                .unwrap_or(line.len());
            (end > 0).then(|| line[..end].to_owned())
        })
        .collect()
}

/// The signature of `pub [async ]fn <name>(` in `rel`, up to its body.
fn signature(rel: &str, name: &str) -> String {
    let source = workspace_file(rel);
    let at = [format!("pub async fn {name}("), format!("pub fn {name}(")]
        .iter()
        .find_map(|opener| source.find(opener.as_str()))
        .unwrap_or_else(|| panic!("{rel} has no `pub fn {name}(`"));
    let rest = &source[at..];
    rest[..rest.find('{').expect("a body")].to_owned()
}

/// The `#[wasm_bindgen]` item exported as `js_name`, up to its body.
fn wasm_export(js_name: &str) -> String {
    let source = workspace_file("kupferdraht-core/src/wasm/manager.rs");
    let at = [
        format!("#[wasm_bindgen(js_name = {js_name})]"),
        format!("#[wasm_bindgen(getter, js_name = {js_name})]"),
    ]
    .iter()
    .find_map(|attr| source.find(attr.as_str()))
    .unwrap_or_else(|| panic!("manager.rs exports no {js_name}"));
    let rest = &source[at..];
    rest[..rest.find('{').expect("a body")].to_owned()
}

/// `name` is no longer exported to the browser.
fn not_exported(js_name: &str) -> bool {
    !workspace_file("kupferdraht-core/src/wasm/manager.rs")
        .contains(&format!("js_name = {js_name})]"))
}

const SETTINGS: &str = "kupferdraht-core/src/client/server_settings.rs";
const RECORDS: &str = "kupferdraht-core/src/storage/records.rs";
const ADMIN: &str = "kupferdraht-core/src/ffi/admin.rs";
const TYPES: &str = "bindings/web/kupferdraht-tabs/index.d.ts";
const TABS: &str = "bindings/web/kupferdraht-tabs/";

/// `name` is no code name outside `wire/` any more.
fn gone(name: &str) {
    let files = files_naming(name);
    assert!(files.is_empty(), "{name} is gone: {files:?}");
}

#[test]
fn the_server_settings_are_read_by_their_name() {
    assert!(signature(SETTINGS, "server_settings").contains("Vec<ServerSetting>"));
    let one = signature(SETTINGS, "server_setting");
    assert!(
        one.contains("name: &str") && one.contains("Option<ServerSetting>"),
        "server_setting(name): {one}"
    );
    assert!(signature(ADMIN, "server_settings").contains("Vec<ServerSetting>"));
    assert!(signature(ADMIN, "server_setting").contains("Option<ServerSetting>"));
    assert_eq!(
        public_fields(RECORDS, "ServerSetting"),
        ["name", "status", "config_json", "last_synced_at_ms"]
    );
    for old in [
        "FeatureConfigEntry",
        ".feature_configs(",
        "fn feature_configs(",
        ".feature_config(",
        "fn feature_config(",
        "mod feature_configs",
        "mod server_configs",
    ] {
        gone(old);
    }
}

#[test]
fn the_account_data_is_read_by_its_name() {
    assert!(signature(SETTINGS, "account_data").contains("Vec<AccountDataEntry>"));
    let one = signature(SETTINGS, "account_data_entry");
    assert!(
        one.contains("key: &str") && one.contains("Option<AccountDataEntry>"),
        "account_data_entry(key): {one}"
    );
    assert!(signature(ADMIN, "account_data").contains("Vec<AccountDataEntry>"));
    assert!(signature(ADMIN, "account_data_entry").contains("Option<AccountDataEntry>"));
    assert_eq!(
        public_fields(RECORDS, "AccountDataEntry"),
        ["key", "value", "last_synced_at_ms"]
    );
    for old in [
        "UserPropertyEntry",
        "user_properties(",
        "user_property(",
        "mod user_properties",
    ] {
        gone(old);
    }
}

#[test]
fn the_read_receipt_setting_is_a_typed_setting() {
    assert_eq!(variants(SETTINGS, "ReadReceiptSetting"), ["Off", "On"]);
    assert!(signature(SETTINGS, "read_receipt_setting").contains("Result<ReadReceiptSetting,"));
    assert!(signature(ADMIN, "read_receipt_setting").contains("ReadReceiptSetting"));
    assert!(
        workspace_file("kupferdraht-core/src/lib.rs").contains("ReadReceiptSetting"),
        "the crate root re-exports the setting"
    );
    for old in ["ReceiptMode", "receipt_mode("] {
        gone(old);
    }
}

/// The store contract names its rows and methods in plain terms; the SQL
/// tables and columns behind them keep theirs.
#[test]
fn the_store_contract_carries_the_plain_names() {
    assert_eq!(
        public_fields(RECORDS, "ExternalSenderKey"),
        ["purpose", "signature_scheme", "public_key"]
    );
    let store = workspace_file(RECORDS);
    for method in [
        "save_external_sender_key(",
        "load_external_sender_key(",
        "save_own_user(",
        "load_own_user(",
        "replace_server_settings(",
        "upsert_server_setting(",
        "list_server_settings(",
        "get_server_setting(",
        "replace_account_data(",
        "upsert_account_data_entry(",
        "delete_account_data_entry(",
        "list_account_data(",
        "get_account_data_entry(",
    ] {
        assert!(
            store.contains(&format!("async fn {method}")),
            "Store has {method}"
        );
    }
    for old in [
        "BackendMlsKey",
        "backend_mls_key(",
        "mod backend_mls_keys",
        "SelfUserRecord",
        "self_user(",
        "mod self_user",
        "sync_marker",
        "confirmer",
    ] {
        gone(old);
    }
}

#[test]
fn the_store_records_read_and_write_the_plain_names() {
    fn round_trip<T: serde::Serialize + serde::de::DeserializeOwned>(row: &serde_json::Value) {
        let read: T = serde_json::from_value(row.clone()).expect("the record reads its names");
        assert_eq!(&serde_json::to_value(&read).expect("serialize"), row);
    }
    round_trip::<OwnUserRecord>(&json!({
        "id": "alice@example.test",
        "username": "alice",
        "name": "Alice",
        "email": null,
        "team_id": null,
        "team_name": null,
        "avatar_source": { "thumbnail": "t", "full": "f" },
        "team_avatar_version": "icon",
        "supported_protocols": ["mls"]
    }));
    round_trip::<SyncStateRecord>(&json!({
        "sync_token": "11111111-2222-3333-4444-555555555555",
        "last_event_id": "e1",
        "last_connected_at_ms": 1,
        "last_key_package_check_at_ms": null
    }));
    round_trip::<DeliveryReceiptRecord>(&json!({
        "message_id": "m1",
        "room_id": "room@example.test",
        "reader_user_id": "bob@example.test",
        "confirmed_at_ms": 2
    }));
    round_trip::<ExternalSenderKey>(&json!({
        "purpose": "removal",
        "signature_scheme": "ecdsa_secp256r1_sha256",
        "public_key": [1, 2, 3]
    }));
    round_trip::<ServerSetting>(&json!({
        "name": "fileSharing",
        "status": "enabled",
        "config_json": "{}",
        "last_synced_at_ms": 3
    }));
    round_trip::<AccountDataEntry>(&json!({
        "key": "WIRE_RECEIPT_MODE",
        "value": "1",
        "last_synced_at_ms": 4
    }));
}

#[test]
fn the_browser_follows() {
    let types = workspace_file(TYPES);
    for declared in [
        "export interface ServerSetting { name: string; status: string | null; configJson: string; \
         lastSyncedAtMs: number }",
        "serverSettings(): Promise<ServerSetting[]>;",
    ] {
        assert!(types.contains(declared), "index.d.ts declares {declared}");
    }
    assert!(
        !types.contains("FeatureConfig"),
        "index.d.ts has no FeatureConfig"
    );
    assert!(wasm_export("serverSettings").contains("pub async fn server_settings("));
    assert!(not_exported("featureConfigs"));
    assert!(
        workspace_file(&format!("{TABS}protocol.js"))
            .contains(r#"["Client", "serverSettings", "idempotent", null],"#),
        "the tabs protocol routes serverSettings"
    );
    assert!(
        workspace_file(&format!("{TABS}proxy.js"))
            .contains("serverSettings() { return this.#call('serverSettings', []); }"),
        "the client proxy forwards serverSettings"
    );
    let readme = workspace_file(&format!("{TABS}README.md"));
    assert!(
        readme.contains("serverSettings") && !readme.contains("featureConfigs"),
        "the tabs README lists serverSettings"
    );
}

#[test]
fn the_cli_follows() {
    let main = workspace_file("kupferdraht-cli/src/main.rs");
    let commands = &main[..main.find("#[cfg(test)]").unwrap_or(main.len())];
    let settings = &commands[commands.find("\n    Settings {").expect("settings")..];
    let settings = &settings[..settings.find("\n    },").expect("its fields end")];
    assert!(
        settings.contains("name: Option<String>"),
        "settings --name: {settings}"
    );
    let data = &commands[commands.find("\n    AccountData {").expect("account-data")..];
    let data = &data[..data.find("\n    },").expect("its fields end")];
    assert!(
        data.contains("key: Option<String>"),
        "account-data --key: {data}"
    );
    assert!(
        !commands.contains("\n    Features {") && !commands.contains("\n    Properties {"),
        "the old verbs are gone"
    );
}

#[test]
fn the_error_texts_speak_of_rooms() {
    assert_eq!(
        KupferdrahtError::MlsGroupMissing.to_string(),
        "MLS group not loaded for this room; call `Client::join_room` first"
    );
    assert_eq!(
        KupferdrahtError::MlsStaleMessage.to_string(),
        "MLS message epoch is stale; resync the room and retry"
    );
}
