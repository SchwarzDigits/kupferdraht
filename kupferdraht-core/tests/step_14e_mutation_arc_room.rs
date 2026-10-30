// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 14e — mutation methods return `Arc<Room>`.
//!
//! TDD red-phase. The tests below pin the public-API contract from
//! `docs/plan/14-matrix-sdk-parity.md` §"TDD starter tests — 14e":
//!
//! * `Client::invite_members`, `Client::kick_members`,
//!   `Client::set_room_access`, `Client::rename_room` change from
//!   `Result<(), KupferdrahtError>` to `Result<Arc<Room>, KupferdrahtError>`.
//! * The returned `Arc<Room>` is identity-equal (via `Arc::ptr_eq`)
//!   to a subsequent `client.room(&room_id)` call — same cache
//!   handle, not a fresh allocation.
//! * The `Room::set_name` / `Room::set_access` / `Room::invite` /
//!   `Room::kick` delegators carry the new return shape up to the
//!   `Room` surface for parity.
//! * Existing call-sites that discarded the result with `?;`
//!   continue to compile (additive change in practice).
//!
//! The four behavioural tests cover `rename_room` and `set_room_access`
//! end-to-end (plus their `Room::set_name` / `Room::set_access`
//! delegators) because those two paths avoid the MLS commit-bundle
//! setup that `invite_members` / `kick_members` need (no group_id
//! alignment, no signer, no group-info export). The compile-only
//! `_mutation_signatures_compile` function covers all four Client
//! methods + all four Room delegators at the type level — it is
//! `#[allow(dead_code)]`, never invoked at runtime, but its
//! presence forces every signature into the new
//! `Result<Arc<Room>, _>` shape on green-phase compilation. Their
//! full HTTP/MLS pipeline is exercised by the CLI live-smoke flow.
//!
//! At red-phase the four Client methods are stubbed with `todo!()`
//! bodies under the new signature so this file compiles; the four
//! behavioural tests panic via that `todo!()` once they reach the
//! mutation call. Green-phase replaces the bodies with real logic.

use std::sync::Arc;

use httpmock::Method::{POST, PUT};
use httpmock::MockServer;
use kupferdraht::{Client, SyncSettings};

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
/// `access_role` so the `set_room_access` test can assert against
/// `Room::access_modes` / `Room::access_roles` post-mutation.
fn conv_json(name: &str, access: &[&str], access_role: &[&str]) -> String {
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
            "name":"{name}",
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

fn mount_sync_with_single_room(
    server: &MockServer,
    name: &str,
    access: &[&str],
    access_role: &[&str],
) {
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
    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list");
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(
                r#"{{"found":[{}], "not_found":[], "failed":[]}}"#,
                conv_json(name, access, access_role)
            ));
    });
    server.mock(|when, then| {
        when.method(POST).path("/v14/list-users");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"found":[]}"#);
    });
}

// ---- 14e — Client::rename_room -----------------------------------

#[tokio::test]
async fn rename_room_returns_arc_room_with_new_name() {
    let server = MockServer::start();
    let client = logged_in_client(&server).await;
    // Mock returns the new name for both sync_once (initial) and the
    // implicit sync_metadata refresh that rename_room triggers post-
    // mutation. The PUT-body assertion below proves the rename was
    // actually issued; the return-shape assertion proves 14e's
    // contract.
    mount_sync_with_single_room(&server, "Engineering", &["invite"], &["team_member"]);

    let put_name = server.mock(|when, then| {
        when.method(PUT)
            .path(format!("/v14/conversations/{ROOM_DOMAIN}/{ROOM_UUID}/name"))
            .header("authorization", "Bearer rebound.jwt.token")
            .json_body(serde_json::json!({ "name": "Engineering" }));
        then.status(204);
    });

    // Populate the cache so rename_room's lookup finds the room.
    client
        .sync_once(SyncSettings::default())
        .await
        .expect("initial sync");

    let room = client
        .rename_room(&room_id_str(), "Engineering")
        .await
        .expect("rename_room must succeed and return Arc<Room>");

    put_name.assert_calls(1);
    assert_eq!(room.id(), room_id_str(), "returned room must match target");
    assert_eq!(
        room.name(),
        "Engineering",
        "returned room must carry the new name"
    );

    let looked_up = client
        .room(&room_id_str())
        .await
        .expect("post-rename lookup must succeed");
    assert!(
        Arc::ptr_eq(&room, &looked_up),
        "rename_room return must be the same cached Arc<Room> handle"
    );
}

// ---- 14e — Client::set_room_access -------------------------------

#[tokio::test]
async fn set_room_access_returns_arc_room_with_new_access_fields() {
    let server = MockServer::start();
    let client = logged_in_client(&server).await;
    let new_modes = vec!["invite".to_string(), "code".to_string()];
    let new_roles = vec!["team_member".to_string(), "guest".to_string()];
    // The mock pretends the room already has the new access shape, so
    // post-mutation refresh reflects what set_room_access just pushed.
    mount_sync_with_single_room(
        &server,
        "Engineering",
        &["invite", "code"],
        &["team_member", "guest"],
    );

    let put_access = server.mock(|when, then| {
        when.method(PUT)
            .path(format!(
                "/v14/conversations/{ROOM_DOMAIN}/{ROOM_UUID}/access"
            ))
            .header("authorization", "Bearer rebound.jwt.token")
            .json_body(serde_json::json!({
                "access": ["invite", "code"],
                "access_role": ["team_member", "guest"]
            }));
        then.status(204);
    });

    client
        .sync_once(SyncSettings::default())
        .await
        .expect("initial sync");

    let room = client
        .set_room_access(&room_id_str(), &new_modes, &new_roles)
        .await
        .expect("set_room_access must succeed and return Arc<Room>");

    put_access.assert_calls(1);
    assert_eq!(room.id(), room_id_str());
    assert_eq!(
        room.access_modes(),
        new_modes,
        "returned room must carry the new access modes"
    );
    assert_eq!(
        room.access_roles(),
        new_roles,
        "returned room must carry the new access roles"
    );

    let looked_up = client
        .room(&room_id_str())
        .await
        .expect("post-mutation lookup must succeed");
    assert!(
        Arc::ptr_eq(&room, &looked_up),
        "set_room_access return must be the same cached Arc<Room>"
    );
}

// ---- 14e — Room delegators carry the new return shape -----------

#[tokio::test]
async fn room_set_name_delegator_returns_arc_room() {
    let server = MockServer::start();
    let client = logged_in_client(&server).await;
    mount_sync_with_single_room(&server, "Renamed", &["invite"], &["team_member"]);

    server.mock(|when, then| {
        when.method(PUT)
            .path(format!("/v14/conversations/{ROOM_DOMAIN}/{ROOM_UUID}/name"));
        then.status(204);
    });

    client
        .sync_once(SyncSettings::default())
        .await
        .expect("initial sync");

    let room = client
        .room(&room_id_str())
        .await
        .expect("cached room before rename");

    let renamed = room
        .set_name("Renamed")
        .await
        .expect("Room::set_name must succeed and return Arc<Room>");

    assert_eq!(renamed.name(), "Renamed");
    assert_eq!(renamed.id(), room_id_str());
    let looked_up = client
        .room(&room_id_str())
        .await
        .expect("post-rename lookup");
    assert!(
        Arc::ptr_eq(&renamed, &looked_up),
        "Room::set_name return must be the same cached Arc<Room>"
    );
}

#[tokio::test]
async fn room_set_access_delegator_returns_arc_room() {
    let server = MockServer::start();
    let client = logged_in_client(&server).await;
    let new_modes = vec!["invite".to_string(), "code".to_string()];
    let new_roles = vec!["team_member".to_string(), "guest".to_string()];
    mount_sync_with_single_room(
        &server,
        "Engineering",
        &["invite", "code"],
        &["team_member", "guest"],
    );

    server.mock(|when, then| {
        when.method(PUT).path(format!(
            "/v14/conversations/{ROOM_DOMAIN}/{ROOM_UUID}/access"
        ));
        then.status(204);
    });

    client
        .sync_once(SyncSettings::default())
        .await
        .expect("initial sync");

    let room = client
        .room(&room_id_str())
        .await
        .expect("cached room before set_access");

    let updated = room
        .set_access(new_modes.clone(), new_roles.clone())
        .await
        .expect("Room::set_access must succeed and return Arc<Room>");

    assert_eq!(updated.access_modes(), new_modes);
    assert_eq!(updated.access_roles(), new_roles);
    let looked_up = client
        .room(&room_id_str())
        .await
        .expect("post-mutation lookup");
    assert!(
        Arc::ptr_eq(&updated, &looked_up),
        "Room::set_access return must be the same cached Arc<Room>"
    );
}

// ---- 14e — compile-time signature contract for all four ---------

/// Type-level pin for the four Client mutators + four Room delegators
/// that 14e retrofits. Never invoked — its purpose is to fail
/// compilation on red-phase (today's methods return `Result<(), _>`)
/// and pass on green-phase (after the retrofit). Each line forces
/// the return type into `Arc<kupferdraht::Room>` via the local binding.
///
/// Covers `invite_members` and `kick_members` at type-level — they
/// share the post-mutation cache-lookup helper with `rename_room` /
/// `set_room_access`, so the behavioural test for the latter two
/// covers the shared return-shape path. Their full HTTP/MLS pipeline
/// (commit-bundles + sync_metadata) is already exercised by the
/// CLI live-smoke flow.
#[allow(dead_code)]
async fn _mutation_signatures_compile(client: Arc<Client>, room: Arc<kupferdraht::Room>) {
    let _: Arc<kupferdraht::Room> = client
        .invite_members("room@dom", &["u@dom".to_string()])
        .await
        .expect("never reached");
    let _: Arc<kupferdraht::Room> = client
        .kick_members("room@dom", &["u@dom".to_string()])
        .await
        .expect("never reached");
    let _: Arc<kupferdraht::Room> = client
        .rename_room("room@dom", "n")
        .await
        .expect("never reached");
    let _: Arc<kupferdraht::Room> = client
        .set_room_access(
            "room@dom",
            &["invite".to_string()],
            &["team_member".to_string()],
        )
        .await
        .expect("never reached");

    let _: Arc<kupferdraht::Room> = room
        .invite(&["u@dom".to_string()])
        .await
        .expect("never reached");
    let _: Arc<kupferdraht::Room> = room
        .kick(&["u@dom".to_string()])
        .await
        .expect("never reached");
    let _: Arc<kupferdraht::Room> = room.set_name("n").await.expect("never reached");
    let _: Arc<kupferdraht::Room> = room
        .set_access(vec!["invite".to_string()], vec!["team_member".to_string()])
        .await
        .expect("never reached");
}

/// Existing call-sites that discarded the unit return with `?;`
/// must continue to compile after 14e — the result is now
/// `Arc<Room>`, which `?` unwraps and the trailing `;` discards.
/// Marked `#[allow(...)]` because the function is intentionally
/// never called.
#[allow(dead_code, clippy::no_effect_underscore_binding)]
async fn _existing_discard_pattern_still_compiles(client: Arc<Client>) {
    let _ = client
        .invite_members("room@dom", &["u@dom".to_string()])
        .await;
    let _ = client
        .kick_members("room@dom", &["u@dom".to_string()])
        .await;
    let _ = client.rename_room("room@dom", "n").await;
    let _ = client
        .set_room_access(
            "room@dom",
            &["invite".to_string()],
            &["team_member".to_string()],
        )
        .await;
}
