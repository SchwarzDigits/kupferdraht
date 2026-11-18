// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 163c — the rooms in plain names (TDD red).
//!
//! The words themselves are held by `public_vocabulary`. These pins hold the
//! shapes a lexical check cannot see: room access is typed — join rules and
//! the users allowed to join, never the backend's strings — the join link is
//! redeemed by the URL `create_join_link` hands out, and the account's
//! personal room carries no backend name. Every binding follows the core.

use std::path::{Path, PathBuf};

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

/// The body of the first `pub <kind> <name> {` under `src/`, up to its
/// closing brace at the start of a line.
fn item_body(kind: &str, name: &str) -> String {
    let opener = format!("pub {kind} {name} {{");
    let source = core_sources()
        .into_iter()
        .map(|(_, source)| source)
        .find(|source| source.contains(&opener))
        .unwrap_or_else(|| panic!("no `{opener}` under src/"));
    let body = &source[source.find(&opener).expect("opener") + opener.len()..];
    body[..body.find("\n}").expect("item closes")].to_owned()
}

/// The `pub` fields of `pub struct <name>`.
fn public_fields(name: &str) -> Vec<String> {
    item_body("struct", name)
        .lines()
        .map(str::trim)
        .filter_map(|line| line.strip_prefix("pub "))
        .filter_map(|line| line.split_once(':').map(|(field, _)| field.to_owned()))
        .collect()
}

/// The variants of `pub enum <name>`, in order.
fn variants(name: &str) -> Vec<String> {
    item_body("enum", name)
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

/// The signature of `pub async fn <name>(` in `rel`, up to its return type.
fn signature(rel: &str, name: &str) -> String {
    let source = workspace_file(rel);
    let opener = format!("pub async fn {name}(");
    let at = source
        .find(&opener)
        .unwrap_or_else(|| panic!("{rel} has no `{opener}`"));
    let rest = &source[at..];
    rest[..rest.find("->").expect("a return type")].to_owned()
}

/// The `#[wasm_bindgen]` method exported as `js_name`: its signature.
fn wasm_signature(js_name: &str) -> String {
    let source = workspace_file("kupferdraht-core/src/wasm/manager.rs");
    let at = source
        .find(&format!("#[wasm_bindgen(js_name = {js_name})]"))
        .unwrap_or_else(|| panic!("manager.rs exports {js_name}"));
    let rest = &source[at..];
    rest[..rest.find("->").expect("a return type")].to_owned()
}

#[test]
fn room_access_is_typed() {
    assert_eq!(public_fields("RoomAccess"), ["join_rules", "allowed"]);
    assert_eq!(
        variants("JoinRule"),
        ["Private", "Invite", "Link", "JoinLink"]
    );
    assert_eq!(
        variants("Joiner"),
        ["TeamMembers", "NonTeamMembers", "Guests", "Services"]
    );
}

#[test]
fn room_access_is_set_typed_and_read_cached_typed() {
    for (rel, name) in [
        ("kupferdraht-core/src/rooms/room.rs", "set_access"),
        ("kupferdraht-core/src/ffi/room.rs", "set_access"),
    ] {
        let sig = signature(rel, name);
        assert!(sig.contains("access: RoomAccess"), "{rel}: {sig}");
        assert!(!sig.contains("Vec<String>"), "{rel}: {sig}");
    }
    for rel in [
        "kupferdraht-core/src/rooms/room.rs",
        "kupferdraht-core/src/ffi/room.rs",
    ] {
        let source = workspace_file(rel);
        assert!(
            source.contains("pub fn cached_access(&self) -> RoomAccess"),
            "{rel} reads the cached access typed"
        );
        for gone in ["fn access_modes(", "fn access_roles("] {
            assert!(!source.contains(gone), "{rel} still has {gone}");
        }
    }
}

/// The backend's access strings are mapped in `wire/` only.
#[test]
fn the_backends_access_strings_stay_in_wire() {
    let values = [
        "\"private\"",
        "\"invite\"",
        "\"link\"",
        "\"code\"",
        "\"team_member\"",
        "\"non_team_member\"",
        "\"guest\"",
        "\"service\"",
    ];
    for rel in [
        "kupferdraht-core/src/rooms/room.rs",
        "kupferdraht-core/src/client/rooms.rs",
        "kupferdraht-core/src/ffi/room.rs",
    ] {
        let source = workspace_file(rel);
        let code = source.split("#[cfg(test)]").next().expect("code");
        for value in values {
            assert!(!code.contains(value), "{rel} names the backend's {value}");
        }
    }
}

#[test]
fn guests_are_allowed_by_one_name_everywhere() {
    for rel in [
        "kupferdraht-core/src/rooms/room.rs",
        "kupferdraht-core/src/ffi/room.rs",
    ] {
        assert!(
            workspace_file(rel).contains("pub async fn allow_guests(&self)"),
            "{rel} has allow_guests"
        );
    }
    for (path, source) in core_sources() {
        assert!(
            !source.contains("open_to_guests"),
            "{} still says open_to_guests",
            path.display()
        );
    }
}

#[test]
fn a_join_link_is_redeemed_by_its_url() {
    for (rel, name) in [
        ("kupferdraht-core/src/client/rooms.rs", "join_via_link"),
        ("kupferdraht-core/src/ffi/admin.rs", "join_via_link"),
    ] {
        let sig = signature(rel, name);
        assert!(sig.contains("url: String"), "{rel}: {sig}");
        assert!(
            !sig.contains("code") && !sig.contains("key"),
            "{rel} still takes the link's parts: {sig}"
        );
    }
    let sig = wasm_signature("joinViaLink");
    assert!(sig.contains("url: String"), "joinViaLink: {sig}");
    assert!(!sig.contains("code") && !sig.contains("key"), "{sig}");
}

#[test]
fn the_personal_room_has_its_own_name() {
    assert_eq!(
        variants("RoomType"),
        ["DirectMessage", "Group", "Channel", "Personal"]
    );
    assert!(
        core_sources()
            .iter()
            .any(|(_, source)| source.contains("pub fn personal_room(&self)")),
        "Client::personal_room"
    );
    assert!(
        workspace_file("kupferdraht-core/src/ffi/room.rs").contains("pub fn personal_room(&self)"),
        "the FFI mirrors personal_room"
    );
    for (path, source) in core_sources() {
        assert!(
            !source.contains("fn self_room(") && !source.contains("SelfRoom"),
            "{} still names the self room",
            path.display()
        );
    }
}

#[test]
fn the_browser_types_follow() {
    let types = workspace_file("bindings/web/kupferdraht-tabs/index.d.ts");
    assert!(
        types.contains("export interface RoomAccess { joinRules: JoinRule[]; allowed: Joiner[] }"),
        "RoomAccess is typed in index.d.ts"
    );
    assert!(types.contains("setRoomAccess(roomId: string, access: RoomAccess): Promise<Room>;"));
    assert!(types.contains("joinViaLink(url: string, password?: string): Promise<Room>;"));
    assert!(types.contains("allowGuests(roomId: string): Promise<Room>;"));
}

/// The CLI reads the URL like the key it carries: never from argv.
#[test]
fn the_cli_follows() {
    let main = workspace_file("kupferdraht-cli/src/main.rs");
    let commands = &main[..main.find("#[cfg(test)]").unwrap_or(main.len())];
    assert!(commands.contains("PersonalRoom"), "personal-room");
    assert!(!commands.contains("SelfRoom"), "self-room is gone");
    let join = &commands[commands.find("JoinViaLink {").expect("join-via-link")..];
    let join = &join[..join.find("\n    },").expect("its fields end")];
    assert!(
        join.contains("url_stdin: bool"),
        "join-via-link --url-stdin"
    );
    assert!(
        !join.contains("code:") && !join.contains("key_stdin"),
        "join-via-link takes the URL, not its parts: {join}"
    );
    let set = &commands[commands.find("RoomAccessSet {").expect("room-access set")..];
    let set = &set[..set.find("\n    },").expect("its fields end")];
    assert!(
        set.contains("join: String") && set.contains("allow: String"),
        "room-access set --join … --allow …: {set}"
    );
}
