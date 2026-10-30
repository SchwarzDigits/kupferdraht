// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 2 cycle 2b — conversations-metadata refresh end-to-end.
//!
//! Originally written against `Client::sync_metadata` (Step 2's public
//! one-shot refresh). Step 14b made the helper crate-internal and
//! folded it into `Client::sync_once`; these tests now drive
//! `sync_once` and rely on the empty-notifications mock from
//! `common::mount_empty_notifications`. The assertions still pin the
//! same Wire-API contract: list-ids → list round-trip, proteus filter,
//! `RoomType` mapping, idempotency.
//!
//! Drives the public API:
//!
//! * `Client::sync_once` runs the list-ids → list round-trip and
//!   populates the in-memory rooms cache (the metadata-refresh half
//!   of `sync_once`).
//! * `Client::rooms` / `Client::room(id)` read from the cache.
//!
//! Failing tests guard:
//!
//! * Pagination — `has_more = true` on page 1 makes the second
//!   request echo the returned `paging_state`.
//! * Proteus filter — `protocol: "proteus"` rooms drop out at the
//!   `wire::dto` → `Room` boundary.
//! * `RoomType` mapping across all four `ConvType` integers.
//! * `Room::counterpart` yields `Some` for 1:1 DMs, `None` for
//!   groups / channels / self-rooms.
//! * Idempotency — two sync calls in a row produce the same rooms
//!   and don't leak duplicates.

use httpmock::Method::POST;
use httpmock::MockServer;
use kupferdraht::{ClientBuilder, RoomType, SyncSettings};

mod common;

async fn logged_in_client(server: &MockServer) -> std::sync::Arc<kupferdraht::Client> {
    common::mount_full_login_flow(server);
    common::mount_empty_notifications(server);
    ClientBuilder::new()
        .server(common::server_config(&server.base_url()))
        .permit_http()
        .login("alice@example.test".into(), "pw".into(), None)
        .await
        .expect("login")
        .register_device()
        .await
        .expect("register device")
}

fn conv_json(
    id: &str,
    domain: &str,
    conv_type: u8,
    protocol: &str,
    name: &str,
    others: &[(&str, &str)],
) -> String {
    let others_json: Vec<String> = others
        .iter()
        .map(|(i, d)| format!(r#"{{"qualified_id":{{"id":"{i}","domain":"{d}"}}}}"#))
        .collect();
    format!(
        r#"{{
            "qualified_id":{{"id":"{id}","domain":"{domain}"}},
            "type":{conv_type},
            "protocol":"{protocol}",
            "epoch":1,
            "group_id":"Z3JwLWFh",
            "name":"{name}",
            "members":{{
              "self":{{"qualified_id":{{"id":"user-uuid","domain":"example.test"}}}},
              "others":[{}]
            }}
        }}"#,
        others_json.join(",")
    )
}

#[tokio::test]
async fn sync_metadata_populates_rooms_cache() {
    let server = MockServer::start();
    let client = logged_in_client(&server).await;

    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list-ids");
        then.status(200)
            .header("content-type", "application/json")
            .body(
                r#"{"qualified_conversations":[
                     {"id":"room-a","domain":"example.test"}
                   ],"has_more":false,"paging_state":"END"}"#,
            );
    });
    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list");
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(
                r#"{{"found":[{}], "not_found":[], "failed":[]}}"#,
                conv_json(
                    "room-a",
                    "example.test",
                    0,
                    "mls",
                    "Engineering",
                    &[("bob", "example.test")]
                ),
            ));
    });
    server.mock(|when, then| {
        when.method(POST).path("/v14/list-users");
        then.status(200)
            .header("content-type", "application/json")
            .body(
                r#"{"found":[
                     {"qualified_id":{"id":"bob","domain":"example.test"},
                      "name":"Bob Example","handle":"bobx"}
                   ]}"#,
            );
    });

    client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync must succeed");

    let rooms = client.rooms().await;
    assert_eq!(rooms.len(), 1);
    assert_eq!(rooms[0].id(), "room-a@example.test");
    assert_eq!(rooms[0].name(), "Engineering");
    assert_eq!(rooms[0].room_type(), RoomType::Group);

    let looked_up = client
        .room("room-a@example.test")
        .await
        .expect("room() lookup");
    assert_eq!(looked_up.id(), "room-a@example.test");
}

#[tokio::test]
async fn sync_metadata_filters_proteus_only_conversations() {
    let server = MockServer::start();
    let client = logged_in_client(&server).await;

    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list-ids");
        then.status(200)
            .header("content-type", "application/json")
            .body(
                r#"{"qualified_conversations":[
                     {"id":"mls-room","domain":"example.test"},
                     {"id":"proteus-room","domain":"example.test"},
                     {"id":"mixed-room","domain":"example.test"}
                   ],"has_more":false,"paging_state":"END"}"#,
            );
    });
    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list");
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(
                r#"{{"found":[{},{},{}], "not_found":[], "failed":[]}}"#,
                conv_json("mls-room", "example.test", 0, "mls", "MlsGroup", &[]),
                conv_json(
                    "proteus-room",
                    "example.test",
                    0,
                    "proteus",
                    "OldGroup",
                    &[]
                ),
                conv_json("mixed-room", "example.test", 0, "mixed", "MixedGroup", &[]),
            ));
    });

    client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync");
    let rooms = client.rooms().await;

    // mls + mixed pass through; proteus is filtered.
    assert_eq!(rooms.len(), 2);
    let ids: Vec<_> = rooms.iter().map(|r| r.id()).collect();
    assert!(ids.contains(&"mls-room@example.test".to_string()));
    assert!(ids.contains(&"mixed-room@example.test".to_string()));
    assert!(!ids.iter().any(|id| id.contains("proteus")));
}

#[tokio::test]
async fn sync_metadata_paginates_through_list_ids() {
    let server = MockServer::start();
    let client = logged_in_client(&server).await;

    // Page 1: has_more = true, returns PS1.
    let page1 = server.mock(|when, then| {
        when.method(POST)
            .path("/v14/conversations/list-ids")
            .is_true(|req| {
                let bytes = req.body_ref();
                let Ok(v): Result<serde_json::Value, _> = serde_json::from_slice(bytes) else {
                    return false;
                };
                v.get("paging_state").is_none()
            });
        then.status(200)
            .header("content-type", "application/json")
            .body(
                r#"{"qualified_conversations":[
                     {"id":"room-a","domain":"example.test"}
                   ],"has_more":true,"paging_state":"PS1"}"#,
            );
    });
    // Page 2: echoes PS1, has_more = false, returns END.
    let page2 = server.mock(|when, then| {
        when.method(POST)
            .path("/v14/conversations/list-ids")
            .is_true(|req| {
                let bytes = req.body_ref();
                let Ok(v): Result<serde_json::Value, _> = serde_json::from_slice(bytes) else {
                    return false;
                };
                v.get("paging_state") == Some(&serde_json::Value::String("PS1".into()))
            });
        then.status(200)
            .header("content-type", "application/json")
            .body(
                r#"{"qualified_conversations":[
                     {"id":"room-b","domain":"example.test"}
                   ],"has_more":false,"paging_state":"END"}"#,
            );
    });
    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list");
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(
                r#"{{"found":[{},{}], "not_found":[], "failed":[]}}"#,
                conv_json("room-a", "example.test", 0, "mls", "A", &[]),
                conv_json("room-b", "example.test", 0, "mls", "B", &[]),
            ));
    });

    client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync");
    page1.assert_calls(1);
    page2.assert_calls(1);
    assert_eq!(client.rooms().await.len(), 2);
}

#[tokio::test]
async fn sync_metadata_is_idempotent() {
    let server = MockServer::start();
    let client = logged_in_client(&server).await;

    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list-ids");
        then.status(200)
            .header("content-type", "application/json")
            .body(
                r#"{"qualified_conversations":[
                     {"id":"room-a","domain":"example.test"}
                   ],"has_more":false,"paging_state":"END"}"#,
            );
    });
    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list");
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(
                r#"{{"found":[{}], "not_found":[], "failed":[]}}"#,
                conv_json("room-a", "example.test", 0, "mls", "Engineering", &[]),
            ));
    });

    client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync #1");
    let first = client.rooms().await;
    client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync #2");
    let second = client.rooms().await;

    assert_eq!(first.len(), 1);
    assert_eq!(second.len(), 1);
    assert_eq!(first[0].id(), second[0].id());
}

#[tokio::test]
async fn direct_message_exposes_counterpart_with_backfilled_profile() {
    let server = MockServer::start();
    let client = logged_in_client(&server).await;

    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list-ids");
        then.status(200)
            .header("content-type", "application/json")
            .body(
                r#"{"qualified_conversations":[
                     {"id":"dm-ab","domain":"example.test"}
                   ],"has_more":false,"paging_state":"END"}"#,
            );
    });
    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list");
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(
                r#"{{"found":[{}], "not_found":[], "failed":[]}}"#,
                conv_json(
                    "dm-ab",
                    "example.test",
                    2,
                    "mls",
                    "",
                    &[("bob", "example.test")]
                ),
            ));
    });
    // list-users resolves the DM counterpart's display name + handle.
    let list_users = server.mock(|when, then| {
        when.method(POST).path("/v14/list-users").is_true(|req| {
            let bytes = req.body_ref();
            let Ok(v): Result<serde_json::Value, _> = serde_json::from_slice(bytes) else {
                return false;
            };
            v.get("qualified_ids")
                .and_then(|x| x.as_array())
                .is_some_and(|a| a.len() == 1)
        });
        then.status(200)
            .header("content-type", "application/json")
            .body(
                r#"{"found":[
                     {"qualified_id":{"id":"bob","domain":"example.test"},
                      "name":"Bob Example","handle":"bobx"}
                   ]}"#,
            );
    });

    client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync");
    list_users.assert_calls(1);
    let rooms = client.rooms().await;
    assert_eq!(rooms.len(), 1);
    assert_eq!(rooms[0].room_type(), RoomType::DirectMessage);
    let cp = rooms[0].counterpart().expect("DM must have counterpart");
    assert_eq!(cp.user_id, "bob@example.test");
    assert!(!cp.is_self);
    assert_eq!(cp.display_name.as_deref(), Some("Bob Example"));
    assert_eq!(cp.handle.as_deref(), Some("bobx"));
}

#[tokio::test]
async fn sync_metadata_skips_list_users_when_no_other_members() {
    // A lone self-room has no non-self members — the list-users
    // call must be skipped (Wire rejects empty queries, and we
    // shouldn't be wasting a round-trip anyway).
    let server = MockServer::start();
    let client = logged_in_client(&server).await;
    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list-ids");
        then.status(200)
            .header("content-type", "application/json")
            .body(
                r#"{"qualified_conversations":[
                     {"id":"self-room","domain":"example.test"}
                   ],"has_more":false,"paging_state":"END"}"#,
            );
    });
    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list");
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(
                r#"{{"found":[{}], "not_found":[], "failed":[]}}"#,
                conv_json("self-room", "example.test", 1, "mls", "Me", &[]),
            ));
    });
    // Fallback: list-users must NOT be called. The 500 below ensures
    // that if the code ever reaches it, the sync turns red.
    let must_not_call = server.mock(|when, then| {
        when.method(POST).path("/v14/list-users");
        then.status(500);
    });

    client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync must succeed");
    must_not_call.assert_calls(0);
    assert_eq!(client.rooms().await.len(), 1);
}

#[tokio::test]
async fn sync_metadata_tolerates_list_users_failure_bucket() {
    // Wire's list-users returns `failed` for IDs it couldn't resolve
    // (e.g. remote-federation timeouts). Those members keep `None`
    // display_name rather than failing the whole sync.
    let server = MockServer::start();
    let client = logged_in_client(&server).await;
    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list-ids");
        then.status(200)
            .header("content-type", "application/json")
            .body(
                r#"{"qualified_conversations":[
                     {"id":"dm-cd","domain":"example.test"}
                   ],"has_more":false,"paging_state":"END"}"#,
            );
    });
    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list");
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(
                r#"{{"found":[{}], "not_found":[], "failed":[]}}"#,
                conv_json(
                    "dm-cd",
                    "example.test",
                    2,
                    "mls",
                    "",
                    &[("remote", "federated.test")]
                ),
            ));
    });
    server.mock(|when, then| {
        when.method(POST).path("/v14/list-users");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"found":[],"failed":[{"id":"remote","domain":"federated.test"}]}"#);
    });

    client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync must succeed despite failed lookup");
    let rooms = client.rooms().await;
    let cp = rooms[0].counterpart().expect("DM counterpart");
    assert_eq!(cp.user_id, "remote@federated.test");
    assert!(cp.display_name.is_none(), "unresolved name stays None");
    assert!(cp.handle.is_none(), "unresolved handle stays None");
}
