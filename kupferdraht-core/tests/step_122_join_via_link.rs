// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 122 — `join_via_link` joins the room the link names.
//!
//! Before: after the redeem the call took the first cached room whose id
//! ended in `@{domain}` — with any other room of that domain cached it
//! returned (and re-joined) that room and left the link's room without an
//! MLS join; and the caller had to pass the conversation's domain, which
//! the link does not carry. Now the link names the conversation
//! (`GET /conversations/join?key&code`), the room is the one with that id,
//! and there is no domain parameter (`docs/plan/122-web-sdk-gaps.md` D5).

use std::sync::Arc;

use base64::Engine as _;
use httpmock::Method::{GET, POST};
use httpmock::{Mock, MockServer};
use kupferdraht::{ClientBuilder, KupferdrahtError};

mod common;

use common::drain_fixture::{self, DOMAIN};

const KEY: &str = "link-key";
const CODE: &str = "link-code";
/// A room of the same domain the client already has, listed first.
const OTHER_UUID: &str = "0a0a0a0a-0000-4000-8000-000000000001";
const OTHER_GROUP: &[u8] = b"grp-other";
/// The room the link names.
const LINK_UUID: &str = "1b1b1b1b-0000-4000-8000-000000000002";
const LINK_GROUP: &[u8] = b"grp-link";

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// `GET /conversations/join?key&code`: the conversation the link names.
fn mount_link_lookup<'a>(server: &'a MockServer, conversation: &str) -> Mock<'a> {
    let body = format!(r#"{{"id":"{conversation}","has_password":false}}"#);
    server.mock(move |when, then| {
        when.method(GET)
            .path("/v14/conversations/join")
            .query_param("key", KEY)
            .query_param("code", CODE);
        then.status(200)
            .header("content-type", "application/json")
            .body(body.clone());
    })
}

/// `POST /conversations/join` answering `status` (`200` joined, `204` a
/// member already).
fn mount_redeem(server: &MockServer, status: u16) -> Mock<'_> {
    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/join");
        if status == 200 {
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"events":[],"time":"2026-10-05T10:00:00.000Z"}"#);
        } else {
            then.status(status);
        }
    })
}

/// One MLS group conversation of the room list.
fn conversation(uuid: &str, group_id: &[u8]) -> String {
    format!(
        r#"{{"qualified_id":{{"id":"{uuid}","domain":"{DOMAIN}"}},
             "type":0,"protocol":"mls","epoch":1,"group_id":"{group}","name":"{uuid}",
             "members":{{"self":{{"qualified_id":{{"id":"user-uuid","domain":"{DOMAIN}"}}}},
                        "others":[]}}}}"#,
        group = b64(group_id),
    )
}

/// The room list `sync_metadata` fetches: `rooms` in this order.
fn mount_room_list(server: &MockServer, rooms: &[(&str, &[u8])]) {
    let ids: Vec<String> = rooms
        .iter()
        .map(|(uuid, _)| format!(r#"{{"id":"{uuid}","domain":"{DOMAIN}"}}"#))
        .collect();
    let ids = format!(
        r#"{{"qualified_conversations":[{}],"has_more":false,"paging_state":"END"}}"#,
        ids.join(",")
    );
    server.mock(move |when, then| {
        when.method(POST).path("/v14/conversations/list-ids");
        then.status(200)
            .header("content-type", "application/json")
            .body(ids.clone());
    });
    let found: Vec<String> = rooms
        .iter()
        .map(|(uuid, group)| conversation(uuid, group))
        .collect();
    let found = format!(
        r#"{{"found":[{}],"not_found":[],"failed":[]}}"#,
        found.join(",")
    );
    server.mock(move |when, then| {
        when.method(POST).path("/v14/conversations/list");
        then.status(200)
            .header("content-type", "application/json")
            .body(found.clone());
    });
    server.mock(|when, then| {
        when.method(POST).path("/v14/list-users");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"found":[]}"#);
    });
}

/// `GET …/groupinfo` of `uuid`, serving a fixture peer's group `group_id`.
fn mount_group_info<'a>(server: &'a MockServer, uuid: &str, group_id: &[u8]) -> Mock<'a> {
    let path = format!("/v14/conversations/{DOMAIN}/{uuid}/groupinfo");
    let body = common::mls::group_info_tls(group_id, b"carol-uuid:caroldev@example.test");
    server.mock(move |when, then| {
        when.method(GET).path(path.clone());
        then.status(200)
            .header("content-type", "message/mls")
            .body(body.clone());
    })
}

fn mount_commit_bundle(server: &MockServer) -> Mock<'_> {
    server.mock(|when, then| {
        when.method(POST).path("/v14/mls/commit-bundles");
        then.status(201)
            .header("content-type", "application/json")
            .body(r#"{"events":[],"time":"2026-10-05T10:00:01.000Z"}"#);
    })
}

/// A logged-in client with MLS state and no room joined yet.
async fn client(server: &MockServer, home: &std::path::Path) -> Arc<kupferdraht::Client> {
    common::mount_full_login_flow(server);
    common::mount_empty_notifications(server);
    ClientBuilder::new()
        .server(common::server_config(&server.base_url()))
        .permit_http()
        .base_dir(home.to_path_buf())
        .login("alice@example.test".into(), "pw".into(), None)
        .await
        .expect("login")
        .register_device()
        .await
        .expect("register device")
}

/// The fault: another room of the same domain, cached first, was taken.
#[tokio::test]
async fn the_link_room_is_joined_not_the_first_room_of_its_domain() {
    let home = tempfile::tempdir().expect("tempdir");
    let server = MockServer::start();
    let client = client(&server, home.path()).await;
    mount_room_list(
        &server,
        &[(OTHER_UUID, OTHER_GROUP), (LINK_UUID, LINK_GROUP)],
    );
    let lookup = mount_link_lookup(&server, LINK_UUID);
    let redeem = mount_redeem(&server, 200);
    let other_info = mount_group_info(&server, OTHER_UUID, OTHER_GROUP);
    let link_info = mount_group_info(&server, LINK_UUID, LINK_GROUP);
    let commit = mount_commit_bundle(&server);

    let room = client
        .join_via_link(CODE.into(), KEY.into(), None)
        .await
        .expect("join via link");

    assert_eq!(room.id(), format!("{LINK_UUID}@{DOMAIN}"));
    lookup.assert_calls(1);
    redeem.assert_calls(1);
    link_info.assert_calls(1);
    other_info.assert_calls(0);
    commit.assert_calls(1);
}

/// A `204` redeem (a member already) into a room whose group is here: no
/// second join — an external commit would replace the own leaf for nothing.
#[tokio::test]
async fn a_room_joined_already_is_not_joined_again() {
    let server = MockServer::start();
    let fixture = drain_fixture::joined_client(&server).await;
    let lookup = mount_link_lookup(&server, drain_fixture::ROOM_UUID);
    let redeem = mount_redeem(&server, 204);
    let group_info = mount_group_info(&server, drain_fixture::ROOM_UUID, drain_fixture::GROUP_ID);
    let commit = mount_commit_bundle(&server);

    let room = fixture
        .client
        .join_via_link(CODE.into(), KEY.into(), None)
        .await
        .expect("join via link");

    assert_eq!(room.id(), drain_fixture::room_id());
    lookup.assert_calls(1);
    redeem.assert_calls(1);
    group_info.assert_calls(0);
    commit.assert_calls(0);
}

/// The redeem succeeded but the room list lacks the room: the error must
/// not read like the caller passed bad arguments.
#[tokio::test]
async fn a_room_missing_after_the_redeem_is_not_invalid_input() {
    let home = tempfile::tempdir().expect("tempdir");
    let server = MockServer::start();
    let client = client(&server, home.path()).await;
    mount_room_list(&server, &[(OTHER_UUID, OTHER_GROUP)]);
    mount_link_lookup(&server, LINK_UUID);
    mount_redeem(&server, 200);
    let other_info = mount_group_info(&server, OTHER_UUID, OTHER_GROUP);

    let result = client.join_via_link(CODE.into(), KEY.into(), None).await;

    assert!(
        matches!(result, Err(KupferdrahtError::Generic(_))),
        "joined, but the room is missing: {:?}",
        result.err()
    );
    other_info.assert_calls(0);
}

/// A link the own backend does not know — another operator's, or a revoked
/// one — fails at the lookup, before anything is redeemed.
#[tokio::test]
async fn an_unknown_link_fails_before_the_redeem() {
    let home = tempfile::tempdir().expect("tempdir");
    let server = MockServer::start();
    let client = client(&server, home.path()).await;
    server.mock(|when, then| {
        when.method(GET).path("/v14/conversations/join");
        then.status(404)
            .header("content-type", "application/json")
            .body(r#"{"code":404,"label":"no-conversation-code","message":"Conversation code not found"}"#);
    });
    let redeem = mount_redeem(&server, 200);

    let result = client.join_via_link(CODE.into(), KEY.into(), None).await;

    assert!(
        matches!(&result, Err(KupferdrahtError::BackendRejected(m)) if m.contains("no-conversation-code")),
        "{:?}",
        result.err()
    );
    redeem.assert_calls(0);
}
