// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 99 — one unknown conversation must not stop the sync (review
//! finding H2). TDD red.
//!
//! The metadata sync — the first phase of every sync — decoded the
//! conversation list and the user list strictly: one conversation of a
//! kind Kupferdraht does not know (a new `type`, a new `protocol`), or one
//! user profile that does not decode, failed the whole list, the
//! metadata sync, and the sync. Now each entry decodes on its own; an
//! undecodable one is logged and skipped.

use std::sync::Arc;

use httpmock::Method::POST;
use httpmock::MockServer;
use kupferdraht::keystore::{InMemoryKeyStore, SecureKeyStore};
use kupferdraht::{Client, ClientBuilder, SyncSettings};

mod common;

const DOMAIN: &str = "example.test";
const GOOD_ROOM: &str = "99000000-0000-4000-8000-000000000001";
const NEW_KIND_ROOM: &str = "99000000-0000-4000-8000-000000000002";
const NEW_PROTOCOL_ROOM: &str = "99000000-0000-4000-8000-000000000003";

async fn logged_in_client(server: &MockServer) -> Arc<Client> {
    common::mount_full_login_flow(server);
    common::mount_empty_notifications(server);
    let home = tempfile::tempdir().expect("tempdir");
    let path = home.path().to_path_buf();
    let _ = Box::leak(Box::new(home));
    let keystore: Arc<dyn SecureKeyStore> = Arc::new(InMemoryKeyStore::new());
    ClientBuilder::new()
        .server(common::server_config(&server.base_url()))
        .permit_http()
        .base_dir(path)
        .keystore(keystore)
        .login("alice@example.test".into(), "pw".into(), None)
        .await
        .expect("login")
        .register_device()
        .await
        .expect("register device")
}

fn conversation(uuid: &str, conv_type: u8, protocol: &str) -> String {
    format!(
        r#"{{"qualified_id":{{"id":"{uuid}","domain":"{DOMAIN}"}},
             "type":{conv_type},"protocol":"{protocol}","epoch":1,
             "name":"room {uuid}",
             "members":{{
               "self":{{"qualified_id":{{"id":"user-uuid","domain":"{DOMAIN}"}}}},
               "others":[{{"qualified_id":{{"id":"bob-uuid","domain":"{DOMAIN}"}}}}]
             }}}}"#
    )
}

fn mount_conversations(server: &MockServer, rooms: &[(&str, String)]) {
    let ids: Vec<String> = rooms
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
    let found: Vec<String> = rooms.iter().map(|(_, body)| body.clone()).collect();
    let list_body = format!(
        r#"{{"found":[{}],"not_found":[],"failed":[]}}"#,
        found.join(",")
    );
    server.mock(move |when, then| {
        when.method(POST).path("/v14/conversations/list");
        then.status(200)
            .header("content-type", "application/json")
            .body(list_body.clone());
    });
}

fn mount_users(server: &MockServer, found: &str) {
    let body = format!(r#"{{"found":[{found}],"failed":[]}}"#);
    server.mock(move |when, then| {
        when.method(POST).path("/v14/list-users");
        then.status(200)
            .header("content-type", "application/json")
            .body(body.clone());
    });
}

fn qualified(uuid: &str) -> String {
    format!("{uuid}@{DOMAIN}")
}

/// A conversation of a new kind (`type` 9) and one with a new protocol
/// sit next to an ordinary group: the sync succeeds, the group is there,
/// the other two are skipped.
#[tokio::test]
async fn one_unknown_conversation_does_not_stop_the_sync() {
    let server = MockServer::start();
    let client = logged_in_client(&server).await;
    mount_conversations(
        &server,
        &[
            (NEW_KIND_ROOM, conversation(NEW_KIND_ROOM, 9, "mls")),
            (GOOD_ROOM, conversation(GOOD_ROOM, 0, "mls")),
            (
                NEW_PROTOCOL_ROOM,
                conversation(NEW_PROTOCOL_ROOM, 0, "quantum"),
            ),
        ],
    );
    mount_users(
        &server,
        &format!(r#"{{"qualified_id":{{"id":"bob-uuid","domain":"{DOMAIN}"}},"name":"Bob"}}"#),
    );

    client
        .sync_once(SyncSettings::default())
        .await
        .expect("one unknown conversation does not fail the sync");
    assert!(
        client.room(&qualified(GOOD_ROOM)).await.is_some(),
        "the ordinary group is synced"
    );
    assert!(client.room(&qualified(NEW_KIND_ROOM)).await.is_none());
    assert!(client.room(&qualified(NEW_PROTOCOL_ROOM)).await.is_none());
}

/// A user profile that does not decode (no `name`) must not fail the
/// sync: the name backfill is best-effort, the room is still there.
#[tokio::test]
async fn one_undecodable_user_profile_does_not_stop_the_sync() {
    let server = MockServer::start();
    let client = logged_in_client(&server).await;
    mount_conversations(&server, &[(GOOD_ROOM, conversation(GOOD_ROOM, 0, "mls"))]);
    mount_users(
        &server,
        &format!(r#"{{"qualified_id":{{"id":"bob-uuid","domain":"{DOMAIN}"}}}}"#),
    );

    client
        .sync_once(SyncSettings::default())
        .await
        .expect("one undecodable profile does not fail the sync");
    assert!(client.room(&qualified(GOOD_ROOM)).await.is_some());
}
