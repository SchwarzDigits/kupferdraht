// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 14d — `Room::access()` returns `RoomAccess`.
//!
//! TDD red-phase. The tests below pin the public-API contract from
//! `docs/plan/14-matrix-sdk-parity.md` §"TDD starter tests — 14d":
//!
//! * `Room::access() async -> Result<RoomAccess, KupferdrahtError>` — the
//!   first method on `Room` that issues a Wire HTTP call. Replaces
//!   `Client::fetch_room_access(&str)` (which is removed from the
//!   public API in green-phase — verified by
//!   `grep -n 'pub async fn fetch_' kupferdraht-core/src/` returning zero
//!   hits, recorded as a separate manual check in the landing
//!   commit message).
//! * `RoomAccess` is a public struct with two fields, `modes:
//!   Vec<String>` (Wire's `access` array — `"invite"`, `"code"`,
//!   `"link"`, `"private"`) and `roles: Vec<String>` (Wire's
//!   `access_role` array — `"team_member"`, `"non_team_member"`,
//!   `"guest"`, `"service"`). Re-exported from `kupferdraht::RoomAccess`.
//! * Under the hood `Room::access()` issues the same single-id
//!   `POST /v{N}/conversations/list` that today's
//!   `Client::fetch_room_access` drives — same HTTP call, method
//!   moves to `Room` to fit matrix-rust-sdk's per-room-accessor
//!   pattern.
//!
//! Two behaviour tests + one type-level compile pin cover the
//! contract. The behavioural tests run against a logged-in client
//! with a single cached room; after `sync_once`, `Room::access()`
//! re-fetches the conversation through the same `conversations/list`
//! endpoint and returns the access fields as a typed `RoomAccess`.
//!
//! At red-phase `Room::access` is stubbed via `todo!()` so the test
//! file compiles; the behaviour tests panic once they reach the
//! mutation call. Green-phase replaces the body with the same
//! `POST /conversations/list` logic that today's
//! `Client::fetch_room_access` carries, returning `RoomAccess`
//! instead of `(Vec<String>, Vec<String>)`, and removes
//! `Client::fetch_room_access` from the public surface.

use std::sync::Arc;

use httpmock::Method::POST;
use httpmock::MockServer;
use kupferdraht::{Client, RoomAccess, SyncSettings};

mod common;

const ROOM_UUID: &str = "room-uuid";
const ROOM_DOMAIN: &str = "wire.com";

async fn logged_in_client(server: &MockServer) -> Arc<Client> {
    common::mount_full_login_flow(server);
    common::mount_empty_notifications(server);
    let (client, _store) = common::run_login(server).await;
    client
}

fn room_id_str() -> String {
    format!("{ROOM_UUID}@{ROOM_DOMAIN}")
}

/// JSON for a single `OwnConversation` entry. Carries `access` and
/// `access_role` so the room's cached snapshot — and the post-call
/// `Room::access()` re-fetch — both reflect the configured values.
fn conv_json(access: &[&str], access_role: &[&str]) -> String {
    let join_quoted = |xs: &[&str]| {
        xs.iter()
            .map(|s| format!("\"{s}\""))
            .collect::<Vec<_>>()
            .join(",")
    };
    format!(
        r#"{{
            "qualified_id":{{"id":"{ROOM_UUID}","domain":"{ROOM_DOMAIN}"}},
            "type":0,
            "protocol":"mls",
            "epoch":0,
            "group_id":"Z3JwLWFh",
            "name":"Engineering",
            "access":[{}],
            "access_role":[{}],
            "members":{{
              "self":{{"qualified_id":{{"id":"user-uuid","domain":"example.test"}}}},
              "others":[]
            }}
        }}"#,
        join_quoted(access),
        join_quoted(access_role),
    )
}

/// Mounts the three sync endpoints so `sync_once` populates the
/// cache with a single MLS room carrying the requested access
/// fields. The `conversations/list` mock has no `times(...)` bound,
/// so the same body is returned on both the initial-sync call AND
/// the subsequent `Room::access()` re-fetch — exactly the live-Wire
/// behaviour, since Wire returns the same data on a repeat
/// `POST /conversations/list` against the same id. Returns the
/// `conversations/list` mock handle so the test can assert the
/// per-endpoint hit count.
fn mount_initial_sync<'a>(
    server: &'a MockServer,
    access: &[&str],
    access_role: &[&str],
) -> httpmock::Mock<'a> {
    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list-ids");
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(
                r#"{{"qualified_conversations":[
                     {{"id":"{ROOM_UUID}","domain":"{ROOM_DOMAIN}"}}
                   ],"has_more":false,"paging_state":"END"}}"#
            ));
    });
    let list_mock = server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list");
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(
                r#"{{"found":[{}], "not_found":[], "failed":[]}}"#,
                conv_json(access, access_role),
            ));
    });
    server.mock(|when, then| {
        when.method(POST).path("/v14/list-users");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"found":[]}"#);
    });
    list_mock
}

// ---- 14d — behavioural tests for Room::access -------------------

#[tokio::test]
async fn room_access_returns_room_access_struct_with_modes_and_roles() {
    let server = MockServer::start();
    let client = logged_in_client(&server).await;
    let _list_mock = mount_initial_sync(&server, &["invite", "code"], &["team_member", "guest"]);

    client
        .sync_once(SyncSettings::default())
        .await
        .expect("initial sync");

    let room = client
        .room(&room_id_str())
        .await
        .expect("room must be in cache after sync");

    let access: RoomAccess = room.access().await.expect("Room::access must succeed");

    assert_eq!(
        access.modes,
        vec!["invite".to_string(), "code".to_string()],
        "RoomAccess.modes must mirror Wire's `access` array",
    );
    assert_eq!(
        access.roles,
        vec!["team_member".to_string(), "guest".to_string()],
        "RoomAccess.roles must mirror Wire's `access_role` array",
    );
}

#[tokio::test]
async fn room_access_re_fetches_via_conversations_list_endpoint() {
    // `Room::access()` must hit `POST /v{N}/conversations/list` —
    // it is a server fetch, not a cached projection. The mock for
    // `/conversations/list` is hit once by `sync_once` (initial
    // metadata refresh) and a second time by `Room::access()` —
    // both hits are accumulated on the same mock handle, so a
    // `hits() >= 2` assertion proves the re-fetch happened.
    let server = MockServer::start();
    let client = logged_in_client(&server).await;
    let list_mock = mount_initial_sync(&server, &["invite"], &["team_member"]);

    client
        .sync_once(SyncSettings::default())
        .await
        .expect("initial sync");
    assert_eq!(
        list_mock.calls(),
        1,
        "sync_once hits conversations/list once"
    );

    let room = client.room(&room_id_str()).await.expect("cached room");
    let _ = room.access().await.expect("Room::access must succeed");

    assert_eq!(
        list_mock.calls(),
        2,
        "Room::access must hit conversations/list a second time (server fetch, not cache projection)",
    );
}

// ---- 14d — compile-time signature pin ---------------------------

/// Type-level pin for `Room::access() -> Result<RoomAccess, _>`
/// and `RoomAccess`'s two-field shape. Never invoked at runtime;
/// its presence forces the new signature on green-phase
/// compilation.
///
/// Companion grep assertion (kept in the landing commit message,
/// not enforceable from Rust):
///   `grep -n 'pub async fn fetch_' kupferdraht-core/src/` → zero hits.
/// `Client::fetch_room_access` becomes a `pub(crate)` helper (or
/// is inlined into the `Room::access` implementation) — either
/// way the public-API symbol disappears.
#[allow(dead_code)]
async fn _room_access_signature_compile(room: Arc<kupferdraht::Room>) {
    let access: RoomAccess = room.access().await.expect("never reached");
    let _: Vec<String> = access.modes;
    let _: Vec<String> = access.roles;
}
