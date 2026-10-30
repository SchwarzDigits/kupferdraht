// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 8 — join links: issue, fetch, revoke.
//!
//! Drives `Room::{create_join_link, current_join_link,
//! revoke_join_link}` and `Client::join_via_link` against an
//! `httpmock`-backed Wire stand-in. Pins the public-API shape, the
//! request bodies the HTTP layer emits, and the
//! `JoinLink { url, password_protected }` projection of Wire's
//! `ConversationCodeInfo` response.

use std::sync::Arc;

use httpmock::Method::{DELETE, GET, POST};
use httpmock::MockServer;
use kupferdraht::keystore::{InMemoryKeyStore, SecureKeyStore};
use kupferdraht::{ClientBuilder, JoinLink, SyncSettings};
use tempfile::TempDir;

mod common;

const ROOM_UUID: &str = "dm-ab";
const ROOM_DOMAIN: &str = "example.test";

#[tokio::test]
async fn room_create_join_link_without_password_marks_password_off() {
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    mount_sync_metadata(&server);
    let create_mock = server.mock(|when, then| {
        when.method(POST)
            .path(format!("/v14/conversations/{ROOM_UUID}/code"))
            .json_body(serde_json::json!({}));
        // 200 = code already existed; Wire returns ConversationCodeInfo
        // directly. The 201 = newly-created path returns an Event +
        // requires a follow-up GET — covered by the dedicated test.
        then.status(200)
            .header("content-type", "application/json")
            .body(
                r#"{"key":"k1","code":"c1","uri":"https://wire.com/join/k1/c1","has_password":false}"#,
            );
    });

    let (_client, room) = logged_in_room(&server).await;
    let link: JoinLink = room.create_join_link(None).await.expect("create_join_link");
    create_mock.assert();
    assert_eq!(link.url, "https://wire.com/join/k1/c1");
    assert!(!link.password_protected);
}

#[tokio::test]
async fn room_create_join_link_201_follows_up_with_get_to_fetch_envelope() {
    // Wire's swagger: `POST /code` returns 201 with an Event payload
    // when the code is *newly* created. The Event doesn't carry the
    // link material, so Kupferdraht must immediately follow up with
    // `GET /code` to retrieve the canonical envelope. This test
    // pins that two-call shape.
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    mount_sync_metadata(&server);
    let post_mock = server.mock(|when, then| {
        when.method(POST)
            .path(format!("/v14/conversations/{ROOM_UUID}/code"));
        then.status(201)
            .header("content-type", "application/json")
            .body(r#"{"type":"conversation.code-update","conversation":"dm-ab"}"#);
    });
    let get_mock = server.mock(|when, then| {
        when.method(GET)
            .path(format!("/v14/conversations/{ROOM_UUID}/code"));
        then.status(200)
            .header("content-type", "application/json")
            .body(
                r#"{"key":"k201","code":"c201","uri":"https://wire.com/join/k201/c201","has_password":false}"#,
            );
    });

    let (_client, room) = logged_in_room(&server).await;
    let link = room
        .create_join_link(None)
        .await
        .expect("create_join_link with 201 follow-up");
    post_mock.assert();
    get_mock.assert();
    assert_eq!(link.url, "https://wire.com/join/k201/c201");
}

#[tokio::test]
async fn room_create_join_link_with_password_marks_password_on() {
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    mount_sync_metadata(&server);
    let create_mock = server.mock(|when, then| {
        when.method(POST)
            .path(format!("/v14/conversations/{ROOM_UUID}/code"))
            .json_body(serde_json::json!({"password": "letmein!"}));
        // 200 path: code already existed (covers the Wire shape that
        // was the test's original target). 201 path is exercised by
        // `room_create_join_link_201_follows_up_with_get_to_fetch_envelope`.
        then.status(200)
            .header("content-type", "application/json")
            .body(
                r#"{"key":"k2","code":"c2","uri":"https://wire.com/join/k2/c2","has_password":true}"#,
            );
    });

    let (_client, room) = logged_in_room(&server).await;
    let link = room
        .create_join_link(Some("letmein!".into()))
        .await
        .expect("create_join_link with password");
    create_mock.assert();
    assert!(link.password_protected);
    assert_eq!(link.url, "https://wire.com/join/k2/c2");
}

#[tokio::test]
async fn room_current_join_link_returns_none_when_no_code_exists() {
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    mount_sync_metadata(&server);
    server.mock(|when, then| {
        when.method(GET)
            .path(format!("/v14/conversations/{ROOM_UUID}/code"));
        then.status(404)
            .header("content-type", "application/json;charset=utf-8")
            .body(
                r#"{"code":404,"label":"no-conversation-code","message":"Conversation code not found"}"#,
            );
    });

    let (_client, room) = logged_in_room(&server).await;
    let current = room
        .current_join_link()
        .await
        .expect("current_join_link must surface 404 as Ok(None)");
    assert!(current.is_none());
}

#[tokio::test]
async fn room_current_join_link_returns_some_when_code_exists() {
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    mount_sync_metadata(&server);
    server.mock(|when, then| {
        when.method(GET)
            .path(format!("/v14/conversations/{ROOM_UUID}/code"));
        then.status(200)
            .header("content-type", "application/json")
            .body(
                r#"{"key":"k3","code":"c3","uri":"https://wire.com/join/k3/c3","has_password":false}"#,
            );
    });

    let (_client, room) = logged_in_room(&server).await;
    let link = room
        .current_join_link()
        .await
        .expect("current_join_link")
        .expect("link present");
    assert_eq!(link.url, "https://wire.com/join/k3/c3");
    assert!(!link.password_protected);
}

#[tokio::test]
async fn room_revoke_join_link_succeeds() {
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    mount_sync_metadata(&server);
    let delete_mock = server.mock(|when, then| {
        when.method(DELETE)
            .path(format!("/v14/conversations/{ROOM_UUID}/code"));
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"events":[],"time":"2026-05-03T15:00:00.000Z"}"#);
    });

    let (_client, room) = logged_in_room(&server).await;
    room.revoke_join_link().await.expect("revoke_join_link");
    delete_mock.assert();
}

#[tokio::test]
async fn client_join_via_link_posts_code_key_password_to_join_endpoint() {
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    let join_mock = server.mock(|when, then| {
        when.method(POST)
            .path("/v14/conversations/join")
            .json_body(serde_json::json!({
                "code": "c4",
                "key":  "k4",
                "password": "secret-pw",
            }));
        // 200 returns an Event; for the HTTP-shape test we don't
        // need to drive the MLS commit path (Step 3 already covers
        // that). Kupferdraht's `Client::join_via_link` is allowed to fail
        // on the post-redeem MLS bootstrap — the test asserts only
        // that the request body matched the swagger contract.
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"events":[],"time":"2026-05-03T15:01:00.000Z"}"#);
    });

    // Step 122: the redeem follows the lookup of the conversation the
    // link names.
    server.mock(|when, then| {
        when.method(GET)
            .path("/v14/conversations/join")
            .query_param("key", "k4")
            .query_param("code", "c4");
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(r#"{{"id":"{ROOM_UUID}","has_password":true}}"#));
    });

    let client = logged_in_client(&server).await;
    // Best-effort: the MLS handshake afterwards may fail because
    // the test doesn't mount sync/groupinfo/commit-bundle mocks.
    // What we assert is that the redeem POST happened with the
    // correct body shape.
    let _ = client
        .join_via_link("c4".into(), "k4".into(), Some("secret-pw".into()))
        .await;
    join_mock.assert();
}

// ---- fixture --------------------------------------------------------

async fn logged_in_client(server: &MockServer) -> Arc<kupferdraht::Client> {
    let home = TempDir::new().unwrap();
    let keystore: Arc<dyn SecureKeyStore> = Arc::new(InMemoryKeyStore::new());
    let base = home.path().to_path_buf();
    // Leak the TempDir so it outlives the returned Arc<Client>; the
    // per-account stores inside `home` are referenced by the client. The
    // OS reaps it on process exit.
    let _ = Box::leak(Box::new(home));
    ClientBuilder::new()
        .server(common::server_config(&server.base_url()))
        .permit_http()
        .base_dir(base)
        .keystore(keystore)
        .login("alice@example.test".into(), "pw".into(), None)
        .await
        .expect("login")
        .register_device()
        .await
        .expect("register device")
}

async fn logged_in_room(server: &MockServer) -> (Arc<kupferdraht::Client>, Arc<kupferdraht::Room>) {
    let client = logged_in_client(server).await;
    client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync");
    let qualified = format!("{ROOM_UUID}@{ROOM_DOMAIN}");
    let room = client
        .rooms()
        .await
        .into_iter()
        .find(|r| r.id() == qualified)
        .expect("room from sync_metadata");
    (client, room)
}

fn mount_sync_metadata(server: &MockServer) {
    common::mount_empty_notifications(server);
    let _ = server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list-ids");
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(
                r#"{{"qualified_conversations":[{{"id":"{ROOM_UUID}","domain":"{ROOM_DOMAIN}"}}],"has_more":false,"paging_state":"END"}}"#,
            ));
    });
    let _ = server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list");
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(
                r#"{{"found":[{{
                    "qualified_id":{{"id":"{ROOM_UUID}","domain":"{ROOM_DOMAIN}"}},
                    "type":2,
                    "protocol":"mls",
                    "epoch":5,
                    "group_id":"Z3JwLWFh",
                    "name":"",
                    "members":{{
                        "self":{{"qualified_id":{{"id":"user-uuid","domain":"example.test"}}}},
                        "others":[{{"qualified_id":{{"id":"bob","domain":"example.test"}}}}]
                    }}
                }}], "not_found":[], "failed":[]}}"#,
            ));
    });
    let _ = server.mock(|when, then| {
        when.method(POST).path("/v14/list-users");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"found":[{"qualified_id":{"id":"bob","domain":"example.test"},"name":"Bob Example","handle":"bobx"}]}"#);
    });
}
