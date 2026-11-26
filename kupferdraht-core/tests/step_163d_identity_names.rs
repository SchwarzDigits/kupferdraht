// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 163d — identity, devices and the session in plain names (TDD red).
//!
//! The words themselves are held by `public_vocabulary`. These pins hold what
//! a lexical check cannot see: the shapes of the records a consumer reads (a
//! device, a member, a user id, the store's rows), the code names behind the
//! public ones, and that every binding follows the core.

use std::path::{Path, PathBuf};

use httpmock::Method::GET;
use httpmock::MockServer;
use kupferdraht::storage::{DeviceRecord, OwnUserRecord, RoomMemberRecord, SessionRecord};
use kupferdraht::{DeviceInfo, DeviceKind};
use serde_json::json;

mod common;

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

const FFI: &str = "kupferdraht-core/src/ffi/mod.rs";
const DEVICE: &str = "kupferdraht-core/src/client/device.rs";
const ROOM: &str = "kupferdraht-core/src/rooms/room.rs";
const TYPES: &str = "bindings/web/kupferdraht-tabs/index.d.ts";

#[test]
fn the_user_id_crosses_the_bindings_under_its_own_name() {
    let ffi = workspace_file(FFI);
    assert_eq!(public_fields(FFI, "UserId"), ["localpart", "domain"]);
    assert!(
        ffi.contains("pub fn parse_user_id(id: &str) -> Result<UserId, KupferdrahtError>"),
        "parse_user_id answers a UserId"
    );
    assert!(
        files_naming("QualifiedUserId").is_empty(),
        "QualifiedUserId is gone: {:?}",
        files_naming("QualifiedUserId")
    );
    let kotlin = workspace_file("bindings/kotlin/src/test/kotlin/KupferdrahtTest.kt");
    assert!(kotlin.contains("import uniffi.kupferdraht.UserId"));
    assert!(!kotlin.contains("QualifiedUserId"));
}

#[test]
fn a_device_is_described_in_plain_terms() {
    assert_eq!(
        public_fields(DEVICE, "DeviceInfo"),
        [
            "device_id",
            "kind",
            "display_name",
            "model",
            "registered_at_ms",
            "last_seen_ms",
            "is_own"
        ]
    );
    let body = item_body(DEVICE, "struct", "DeviceInfo");
    for field in [
        "kind: DeviceKind",
        "registered_at_ms: Option<i64>",
        "last_seen_ms: Option<i64>",
    ] {
        assert!(body.contains(field), "DeviceInfo has {field}");
    }
    assert_eq!(
        variants(DEVICE, "DeviceKind"),
        ["Phone", "Tablet", "Desktop", "LegalHold", "Other"]
    );
}

/// The backend's device classes are mapped in `wire/` only.
#[test]
fn the_backends_device_classes_stay_in_wire() {
    let code = workspace_file(DEVICE);
    let code = code.split("#[cfg(test)]").next().expect("code");
    for value in ["\"phone\"", "\"tablet\"", "\"desktop\"", "\"legalhold\""] {
        assert!(
            !code.contains(value),
            "device.rs names the backend's {value}"
        );
    }
}

/// The backend's answer, read as the plain record: the kind typed, the times
/// in milliseconds like every other time, the own device marked.
#[tokio::test]
async fn the_device_list_reads_in_plain_terms() {
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    server.mock(|when, then| {
        when.method(GET).path("/v14/clients");
        then.status(200)
            .header("content-type", "application/json")
            .body(
                r#"[
                {"id":"new-client-id","type":"permanent","class":"desktop","label":"Work laptop",
                 "model":"Kupferdraht","time":"2026-09-28T10:00:01.000Z",
                 "last_active":"2026-09-28T12:00:01.000Z"},
                {"id":"held","type":"legalhold","class":"legalhold","time":"2026-09-28T10:00:01.042Z"},
                {"id":"later","type":"permanent","class":"a-class-of-a-later-release"}
            ]"#,
            );
    });
    let (client, _) = common::run_login(&server).await;

    let devices = client.devices().await.expect("devices");

    let unnamed = |device_id: &str, kind, registered_at_ms| DeviceInfo {
        device_id: device_id.to_owned(),
        kind,
        display_name: None,
        model: None,
        registered_at_ms,
        last_seen_ms: None,
        is_own: false,
    };
    assert_eq!(
        devices,
        [
            DeviceInfo {
                device_id: "new-client-id".to_owned(),
                kind: DeviceKind::Desktop,
                display_name: Some("Work laptop".to_owned()),
                model: Some("Kupferdraht".to_owned()),
                registered_at_ms: Some(1_790_589_601_000),
                last_seen_ms: Some(1_790_596_801_000),
                is_own: true,
            },
            unnamed("held", DeviceKind::LegalHold, Some(1_790_589_601_042)),
            unnamed("later", DeviceKind::Other, None),
        ]
    );
}

#[test]
fn a_device_is_named_by_its_display_name() {
    assert!(
        workspace_file("kupferdraht-core/src/client/builder.rs")
            .contains("pub fn initial_device_display_name("),
        "ClientBuilder::initial_device_display_name"
    );
    for (rel, name) in [
        (DEVICE, "rename_device"),
        ("kupferdraht-core/src/ffi/admin.rs", "rename_device"),
    ] {
        let sig = signature(rel, name);
        assert!(sig.contains("display_name: &str"), "{rel}: {sig}");
        assert!(!sig.contains("label"), "{rel}: {sig}");
    }
    for js_name in ["login", "loginWithSsoCallback"] {
        let export = wasm_export(js_name);
        assert!(
            export.contains("initial_device_display_name: Option<String>"),
            "{js_name}: {export}"
        );
    }
    assert!(
        files_naming("device_label").is_empty(),
        "device_label is gone: {:?}",
        files_naming("device_label")
    );
}

#[test]
fn the_session_label_names_a_homes_login() {
    for rel in [
        "kupferdraht-core/src/manager/mod.rs",
        "kupferdraht-core/src/ffi/manager.rs",
    ] {
        assert!(
            workspace_file(rel).contains("pub fn session_label(&self) -> String"),
            "{rel} has session_label"
        );
    }
    assert!(wasm_export("sessionLabel").contains("pub fn session_label(&self) -> String"));
    assert!(not_exported("cookieLabel"));
    assert!(
        files_naming("cookie_label").is_empty(),
        "cookie_label is gone: {:?}",
        files_naming("cookie_label")
    );
}

#[test]
fn the_username_is_read_and_set_by_its_name() {
    assert!(
        workspace_file("kupferdraht-core/src/client/account.rs")
            .contains("pub fn username(&self) -> Option<&str>"),
        "Account::username"
    );
    assert_eq!(
        public_fields(FFI, "Account"),
        [
            "user_id",
            "display_name",
            "username",
            "email",
            "team_id",
            "team_name",
            "supported_protocols"
        ]
    );
    for rel in [
        "kupferdraht-core/src/client/profile.rs",
        "kupferdraht-core/src/ffi/admin.rs",
    ] {
        let sig = signature(rel, "set_username");
        assert!(sig.contains("username: &str"), "{rel}: {sig}");
    }
    assert!(wasm_export("setUsername").contains("username: String"));
    let browser = workspace_file("kupferdraht-core/src/wasm/manager.rs");
    assert!(browser.contains("pub fn username(&self) -> Option<String>"));
    assert!(!browser.contains("pub fn handle(&self)"));
    assert!(not_exported("setHandle"));
    assert_eq!(
        public_fields("kupferdraht-core/src/storage/global.rs", "AccountEntry"),
        [
            "account_id",
            "server",
            "username",
            "display_name",
            "last_active_ms"
        ]
    );
    assert!(
        files_naming("set_handle").is_empty(),
        "set_handle is gone: {:?}",
        files_naming("set_handle")
    );
}

#[test]
fn a_member_is_own_and_named_by_its_username() {
    assert_eq!(
        public_fields(ROOM, "RoomMember"),
        [
            "user_id",
            "is_own",
            "display_name",
            "username",
            "role",
            "avatar_source"
        ]
    );
    assert!(item_body(ROOM, "struct", "RoomMember").contains("avatar_source: AvatarSource"));
    assert!(
        files_naming("is_self").is_empty(),
        "is_self is gone: {:?}",
        files_naming("is_self")
    );
}

#[test]
fn an_avatar_is_named_by_its_source() {
    let avatar = "kupferdraht-core/src/avatar/mod.rs";
    assert_eq!(public_fields(avatar, "AvatarSource"), ["thumbnail", "full"]);
    let account = workspace_file("kupferdraht-core/src/client/account.rs");
    assert!(account.contains("pub fn avatar_source(&self) -> &AvatarSource"));
    assert!(account.contains("pub fn team_avatar_version(&self) -> Option<&str>"));
    assert!(workspace_file(ROOM).contains("pub fn avatar_version(&self) -> Option<&str>"));
    for rel in [
        "kupferdraht-core/src/client/avatars.rs",
        "kupferdraht-core/src/ffi/avatars.rs",
    ] {
        signature(rel, "avatar_version");
    }
    wasm_export("avatarVersion");
    assert!(not_exported("avatarKey"));
    for gone in ["AvatarKeys", "avatar_key", "login_avatar_keys"] {
        assert!(
            files_naming(gone).is_empty(),
            "{gone} is gone: {:?}",
            files_naming(gone)
        );
    }
}

#[test]
fn the_backends_signing_key_is_the_external_sender_key() {
    assert!(
        signature(
            "kupferdraht-core/src/client/session.rs",
            "external_sender_key"
        )
        .contains("-> &[u8]")
    );
    assert!(
        signature("kupferdraht-core/src/ffi/admin.rs", "external_sender_key")
            .contains("-> Vec<u8>")
    );
    assert!(
        files_naming("removal_key").is_empty(),
        "removal_key is gone: {:?}",
        files_naming("removal_key")
    );
}

/// A device is a device: the backend's word for it stays in `wire/`, in its
/// SQL columns and in its query parameters.
#[test]
fn a_device_id_is_never_a_client_id() {
    assert_eq!(
        public_fields("kupferdraht-core/src/storage/records.rs", "DeviceRecord"),
        ["user_id", "device_id", "mls_public_key"]
    );
    for gone in ["client_id", "CLIENT_ID"] {
        assert!(
            files_naming(gone).is_empty(),
            "{gone} is gone: {:?}",
            files_naming(gone)
        );
    }
}

/// The store contract reads and writes its rows under the new names; the SQL
/// columns stay.
#[test]
fn the_store_records_carry_the_plain_names() {
    fn round_trip<T: serde::Serialize + serde::de::DeserializeOwned>(row: &serde_json::Value) {
        let read: T = serde_json::from_value(row.clone()).expect("the record reads its names");
        assert_eq!(&serde_json::to_value(&read).expect("serialize"), row);
    }
    round_trip::<SessionRecord>(&json!({
        "user_id": "alice@example.test",
        "device_id": "d1",
        "access_token": "a",
        "refresh_token": "r",
        "session_label": "kupferdraht-home",
        "api_version": 14,
        "access_token_issued_at_ms": 1
    }));
    round_trip::<DeviceRecord>(&json!({
        "user_id": "alice@example.test",
        "device_id": "d1",
        "mls_public_key": null
    }));
    round_trip::<RoomMemberRecord>(&json!({
        "room_id": "room@example.test",
        "user_id": "alice@example.test",
        "is_own": true,
        "display_name": "Alice",
        "username": "alice",
        "role": "member",
        "avatar_source": { "thumbnail": "t", "full": "f" }
    }));
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
}

#[test]
fn the_browser_types_follow() {
    let types = workspace_file(TYPES);
    for declared in [
        "export type DeviceKind = 'phone' | 'tablet' | 'desktop' | 'legalHold' | 'other';",
        "deviceId: string; kind: DeviceKind; displayName: string | null; model: string | null;",
        "registeredAtMs: number | null; lastSeenMs: number | null; isOwn: boolean;",
        "export interface Member { userId: string; isOwn: boolean; displayName: string | null; \
         username: string | null; role: string; avatarVersion: string | null }",
        "export interface Reaction { emoji: string; count: number; isOwn: boolean }",
        "username: string | null; displayName: string | null; lastActiveMs: number }",
        "isThisTab: boolean;",
        "readonly sessionLabel: string | undefined;",
        "secondFactorCode?: string, initialDeviceDisplayName?: string): Promise<PendingLoginProxy>;",
        "message: unknown, initialDeviceDisplayName?: string): Promise<PendingLoginProxy>;",
        "readonly username: string | undefined;",
        "setUsername(username: string): Promise<void>;",
        "avatarVersion(userId: string): Promise<string | null>;",
    ] {
        assert!(types.contains(declared), "index.d.ts declares {declared}");
    }
}

/// The tabs package carries the new names on its protocol, its proxies and
/// its host's events.
#[test]
fn the_tabs_package_follows() {
    for rel in [
        "bindings/web/kupferdraht-tabs/protocol.js",
        "bindings/web/kupferdraht-tabs/proxy.js",
        "bindings/web/kupferdraht-tabs/host.js",
        "bindings/web/kupferdraht-tabs/leader.js",
        TYPES,
    ] {
        let source = workspace_file(rel);
        for gone in [
            "cookieLabel",
            "deviceLabel",
            "setHandle",
            "\"handle\"]",
            "avatarKey",
            "isSelf",
        ] {
            assert!(!source.contains(gone), "{rel} still says {gone}");
        }
    }
    let protocol = workspace_file("bindings/web/kupferdraht-tabs/protocol.js");
    for named in [
        "[\"Kupferdraht\", \"sessionLabel\"]",
        "[\"Client\", \"username\"]",
        "[\"Client\", \"setUsername\",",
        "[\"Client\", \"avatarVersion\",",
    ] {
        assert!(protocol.contains(named), "protocol.js names {named}");
    }
}

#[test]
fn the_cli_follows() {
    let main = workspace_file("kupferdraht-cli/src/main.rs");
    let commands = &main[..main.find("#[cfg(test)]").unwrap_or(main.len())];
    let rename = &commands[commands.find("DeviceRename {").expect("device-rename")..];
    let rename = &rename[..rename.find("\n    },").expect("its fields end")];
    assert!(
        rename.contains("name: String"),
        "device-rename --name: {rename}"
    );
    assert!(
        !rename.contains("label"),
        "device-rename takes no label: {rename}"
    );
    let profile = &commands[commands.find("ProfileSet {").expect("profile-set")..];
    let profile = &profile[..profile.find("\n    },").expect("its fields end")];
    assert!(
        profile.contains("username: Option<String>"),
        "profile-set --username: {profile}"
    );
    assert!(
        !profile.contains("handle"),
        "profile-set takes no handle: {profile}"
    );
}
