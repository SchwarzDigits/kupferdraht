// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 168 — the personal room found by its route (TDD red).
//!
//! Wire's clients ask `GET /conversations/mls-self` for the self conversation;
//! Kupferdraht took it from the rooms list (Step 86). The rooms refresh now asks
//! the route first and takes its answer as the personal room's conversation
//! object; a `404` or any other error falls back to the list's type-`1` room,
//! logged. `Client::personal_room` stays the local lookup;
//! `Client::personal_room_source` says which way the last refresh found it.

use std::sync::Arc;

use httpmock::Method::{GET, POST};
use httpmock::{Mock, MockServer};
use kupferdraht::storage::Store;
use kupferdraht::{Client, PersonalRoomSource, SyncSettings};

mod common;

const DOMAIN: &str = "example.test";
const SELF_UUID: &str = "5f98a472-6364-50da-bf25-b77b0b2638e3";
const SELF_GROUP_ID_B64: &str = "Z3JwLXNlbGY=";
const GROUP_UUID: &str = "4b1ff12e-52a2-4b58-9bf6-1b40e64ae7a2";
const GROUP_GROUP_ID_B64: &str = "Z3JwLWdycA==";

fn personal_room_id() -> String {
    format!("{SELF_UUID}@{DOMAIN}")
}

fn self_conv_json(epoch: u64) -> String {
    format!(
        r#"{{
            "qualified_id":{{"id":"{SELF_UUID}","domain":"{DOMAIN}"}},
            "type":1,"protocol":"mls","epoch":{epoch},
            "group_id":"{SELF_GROUP_ID_B64}","name":null,
            "members":{{
              "self":{{"qualified_id":{{"id":"user-uuid","domain":"{DOMAIN}"}}}},
              "others":[]
            }}
        }}"#
    )
}

fn group_conv_json() -> String {
    format!(
        r#"{{
            "qualified_id":{{"id":"{GROUP_UUID}","domain":"{DOMAIN}"}},
            "type":0,"protocol":"mls","epoch":2,
            "group_id":"{GROUP_GROUP_ID_B64}","name":"G",
            "members":{{
              "self":{{"qualified_id":{{"id":"user-uuid","domain":"{DOMAIN}"}}}},
              "others":[{{"qualified_id":{{"id":"bob-uuid","domain":"{DOMAIN}"}}}}]
            }}
        }}"#
    )
}

/// The rooms list with `convs`, plus the list-users and notifications mocks
/// `sync_once` walks.
fn mount_rooms(server: &MockServer, convs: &[(&str, String)]) {
    let ids: Vec<String> = convs
        .iter()
        .map(|(uuid, _)| format!(r#"{{"id":"{uuid}","domain":"{DOMAIN}"}}"#))
        .collect();
    let ids_body = format!(
        r#"{{"qualified_conversations":[{}],"has_more":false,"paging_state":"END"}}"#,
        ids.join(",")
    );
    server.mock(move |when, then| {
        when.method(POST).path("/v14/conversations/list-ids");
        then.status(200)
            .header("content-type", "application/json")
            .body(ids_body.clone());
    });
    let found: Vec<String> = convs.iter().map(|(_, json)| json.clone()).collect();
    let list_body = format!(
        r#"{{"found":[{}], "not_found":[], "failed":[]}}"#,
        found.join(",")
    );
    server.mock(move |when, then| {
        when.method(POST).path("/v14/conversations/list");
        then.status(200)
            .header("content-type", "application/json")
            .body(list_body.clone());
    });
    server.mock(|when, then| {
        when.method(POST).path("/v14/list-users");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"found":[]}"#);
    });
    common::mount_empty_notifications(server);
}

/// `GET /v14/conversations/mls-self` answering `status` with `body`.
fn mount_mls_self<'a>(server: &'a MockServer, status: u16, body: &str) -> Mock<'a> {
    let body = body.to_owned();
    server.mock(move |when, then| {
        when.method(GET).path("/v14/conversations/mls-self");
        then.status(status)
            .header("content-type", "application/json")
            .body(body.clone());
    })
}

async fn logged_in(server: &MockServer) -> (Arc<Client>, Arc<dyn Store>) {
    common::mount_full_login_flow(server);
    common::run_login(server).await
}

async fn sync(client: &Client) {
    client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync_once");
}

/// The refresh asks the route once and takes its answer: the personal room
/// carries the route's epoch, not the list's.
#[tokio::test]
async fn the_refresh_asks_the_route_once_and_takes_its_room() {
    let server = MockServer::start();
    let (client, _store) = logged_in(&server).await;
    mount_rooms(
        &server,
        &[
            (GROUP_UUID, group_conv_json()),
            (SELF_UUID, self_conv_json(0)),
        ],
    );
    let route = mount_mls_self(&server, 200, &self_conv_json(3));

    sync(&client).await;

    route.assert_calls(1);
    let room = client.personal_room().expect("the personal room");
    assert_eq!(room.id(), personal_room_id());
    assert_eq!(room.epoch(), 3, "the route's object wins over the list's");
    assert_eq!(
        client.personal_room_source(),
        Some(PersonalRoomSource::Route)
    );
    assert_eq!(client.rooms().await.len(), 2, "the list's rooms stay");
}

/// A `404` falls back to the list's type-1 room; the refresh succeeds.
#[tokio::test]
async fn a_404_falls_back_to_the_list() {
    let server = MockServer::start();
    let (client, _store) = logged_in(&server).await;
    mount_rooms(&server, &[(SELF_UUID, self_conv_json(0))]);
    let route = mount_mls_self(
        &server,
        404,
        r#"{"code":404,"label":"no-conversation","message":"conversation not found"}"#,
    );

    sync(&client).await;

    route.assert_calls(1);
    let room = client
        .personal_room()
        .expect("the personal room from the list");
    assert_eq!(room.epoch(), 0);
    assert_eq!(
        client.personal_room_source(),
        Some(PersonalRoomSource::List)
    );
}

/// Any other error of the route falls back the same way, without failing
/// the refresh.
#[tokio::test]
async fn a_route_error_falls_back_to_the_list() {
    let server = MockServer::start();
    let (client, _store) = logged_in(&server).await;
    mount_rooms(&server, &[(SELF_UUID, self_conv_json(0))]);
    let route = mount_mls_self(
        &server,
        500,
        r#"{"code":500,"label":"server-error","message":"boom"}"#,
    );

    sync(&client).await;

    route.assert_calls(1);
    assert!(client.personal_room().is_some());
    assert_eq!(
        client.personal_room_source(),
        Some(PersonalRoomSource::List)
    );
}

/// A room the list does not carry yet is still the personal room when the
/// route answers it.
#[tokio::test]
async fn the_route_adds_a_room_the_list_does_not_know() {
    let server = MockServer::start();
    let (client, _store) = logged_in(&server).await;
    mount_rooms(&server, &[(GROUP_UUID, group_conv_json())]);
    let _route = mount_mls_self(&server, 200, &self_conv_json(1));

    sync(&client).await;

    let room = client
        .personal_room()
        .expect("the personal room from the route");
    assert_eq!(room.id(), personal_room_id());
    assert_eq!(client.rooms().await.len(), 2);
}

/// Every refresh asks once — `sync_once`'s first refresh and an explicit
/// `sync_metadata` (a later `sync_once` refreshes only a stale list, Step
/// 118); a client without a refresh yet has no source.
#[tokio::test]
async fn every_refresh_asks_once_and_a_cold_client_names_no_source() {
    let server = MockServer::start();
    let (client, _store) = logged_in(&server).await;
    assert_eq!(client.personal_room_source(), None);
    mount_rooms(&server, &[(SELF_UUID, self_conv_json(0))]);
    let route = mount_mls_self(&server, 200, &self_conv_json(2));

    sync(&client).await;
    client.sync_metadata().await.expect("sync_metadata");

    route.assert_calls(2);
}
