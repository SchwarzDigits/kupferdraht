// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 72a — member roles from conversation metadata (TDD red).
//!
//! ADR-0018 gates case-snapshot core fields on the sender being a
//! conversation administrator, so `Room::members()` must surface
//! Wire's `conversation_role` (`wire_admin` / `wire_member`,
//! `RoleName` string on v14 `Member` / `OtherMember`) as
//! `RoomMemberRole` — and the role must survive the store round-trip
//! (`load_rooms_from_store` replaces the cache from `room_members`).
//! The payload half of 72a is pinned by the `timeline::case` unit
//! tests (proto round-trip, unknown-enum forward compatibility).

use httpmock::Method::POST;
use httpmock::MockServer;
use kupferdraht::rooms::RoomMemberRole;

mod common;

const ROOM: &str = "aa11aa11-2222-4333-8444-555566667777";
const DOMAIN: &str = "example.test";

/// Metadata for one group room: self is `wire_admin`, `bob-uuid` is
/// `wire_member`, `carol-uuid` carries no role field at all (Wire
/// omits it on some legacy rows — must default to member), and
/// `dave-uuid` carries a custom `RoleName` (`custom_admin`) — only
/// the exact `wire_admin` literal may grant administration.
fn mount_room_with_roles(server: &MockServer) {
    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list-ids");
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(
                r#"{{"qualified_conversations":[
                     {{"id":"{ROOM}","domain":"{DOMAIN}"}}
                   ],"has_more":false,"paging_state":"END"}}"#
            ));
    });
    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list");
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(
                r#"{{"found":[{{
                     "qualified_id":{{"id":"{ROOM}","domain":"{DOMAIN}"}},
                     "type":0,"protocol":"mls","epoch":1,"group_id":"Z3JwLXJvbGVz",
                     "name":"Roles",
                     "members":{{
                       "self":{{"qualified_id":{{"id":"user-uuid","domain":"{DOMAIN}"}},
                               "conversation_role":"wire_admin"}},
                       "others":[
                         {{"qualified_id":{{"id":"bob-uuid","domain":"{DOMAIN}"}},
                          "conversation_role":"wire_member"}},
                         {{"qualified_id":{{"id":"carol-uuid","domain":"{DOMAIN}"}}}},
                         {{"qualified_id":{{"id":"dave-uuid","domain":"{DOMAIN}"}},
                          "conversation_role":"custom_admin"}}
                       ]
                     }}
                   }}], "not_found":[], "failed":[]}}"#
            ));
    });
    server.mock(|when, then| {
        when.method(POST).path("/v14/list-users");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"found":[]}"#);
    });
    common::mount_empty_notifications(server);
}

fn role_of(members: &[kupferdraht::rooms::RoomMember], uuid: &str) -> RoomMemberRole {
    members
        .iter()
        .find(|m| m.user_id.starts_with(uuid))
        .unwrap_or_else(|| panic!("member {uuid} present"))
        .role
}

#[tokio::test]
async fn member_roles_decode_from_conversation_metadata() {
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    mount_room_with_roles(&server);
    let (client, _store) = common::run_login(&server).await;
    client.sync_metadata().await.expect("sync_metadata");

    let room_id = format!("{ROOM}@{DOMAIN}");
    let room = client.room(&room_id).await.expect("room cached");
    let members = room.members().await.expect("members");
    assert_eq!(members.len(), 4, "self + three others");
    assert_eq!(
        role_of(&members, "user-uuid"),
        RoomMemberRole::Administrator,
        "self carries wire_admin"
    );
    assert_eq!(role_of(&members, "bob-uuid"), RoomMemberRole::Member);
    assert_eq!(
        role_of(&members, "carol-uuid"),
        RoomMemberRole::Member,
        "an absent conversation_role defaults to Member"
    );
    assert_eq!(
        role_of(&members, "dave-uuid"),
        RoomMemberRole::Member,
        "a custom RoleName must not grant administration — only the \
         exact wire_admin literal does"
    );

    // The role must survive persistence: replace the cache from the
    // store and re-read.
    client
        .load_rooms_from_store()
        .await
        .expect("load_rooms_from_store");
    let reloaded = client.room(&room_id).await.expect("room reloaded");
    let members = reloaded.members().await.expect("members after reload");
    assert_eq!(
        role_of(&members, "user-uuid"),
        RoomMemberRole::Administrator,
        "role persisted in room_members and reloaded"
    );
    assert_eq!(role_of(&members, "bob-uuid"), RoomMemberRole::Member);
    assert_eq!(
        role_of(&members, "carol-uuid"),
        RoomMemberRole::Member,
        "an absent role must persist + reload as Member, not fail or invent"
    );
}

/// The external-join surface for the fixture group under the same
/// group id the metadata mock advertises (`grp-roles`), so
/// `join_room` gives the client a local MLS group whose leaves are
/// the fixture peer (bob) + self.
fn mount_external_join(server: &MockServer) {
    let group_info = common::mls::group_info_tls(b"grp-roles", b"bob-uuid:bobdev01@example.test");
    server.mock(move |when, then| {
        when.method(httpmock::Method::GET)
            .path(format!("/v14/conversations/{DOMAIN}/{ROOM}/groupinfo"));
        then.status(200)
            .header("content-type", "message/mls")
            .body(group_info.clone());
    });
    server.mock(|when, then| {
        when.method(POST).path("/v14/mls/commit-bundles");
        then.status(201)
            .header("content-type", "application/json")
            .body(r#"{"events":[],"time":"2026-08-18T10:00:00.000Z"}"#);
    });
}

/// Step 72a (review feedback 2026-08-18): `Room::sync_members()`
/// rebuilds the member list from MLS leaves — it must MERGE the roles
/// from the conversation-metadata snapshot, not reset every member to
/// `Member` (which would silently strip every admin and starve the
/// 72b fold gate).
#[tokio::test]
async fn sync_members_keeps_roles_from_the_metadata_snapshot() {
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    mount_room_with_roles(&server);
    mount_external_join(&server);
    let (client, _store) = common::run_login(&server).await;
    client.sync_metadata().await.expect("sync_metadata");
    let room_id = format!("{ROOM}@{DOMAIN}");
    client.join_room(&room_id).await.expect("external join");

    let room = client.room(&room_id).await.expect("room cached");
    let members = room.sync_members().await.expect("sync_members");
    // Leaves: the fixture peer (bob) + self — carol is a metadata-only
    // member without a leaf and is legitimately absent here.
    assert_eq!(members.len(), 2, "leaf-derived members: bob + self");
    assert_eq!(
        role_of(&members, "user-uuid"),
        RoomMemberRole::Administrator,
        "sync_members must keep the admin role from the metadata snapshot"
    );
    assert_eq!(role_of(&members, "bob-uuid"), RoomMemberRole::Member);
}
