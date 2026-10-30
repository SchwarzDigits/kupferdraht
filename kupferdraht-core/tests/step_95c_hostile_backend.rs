// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 95c — no panics on backend data. TDD red.
//!
//! Every field a (malicious or buggy) backend returns is runtime data:
//! an empty token, id or key, a malformed conversation or an oversized
//! list must fail as a typed error — or be skipped — never reach an
//! `assert!` / `debug_assert!` further down (CLAUDE.md "Assertions").
//! A hostile mock is mounted **before** the shared login flow: httpmock
//! serves the first matching mock.

use std::sync::Arc;

use httpmock::Method::{DELETE, GET, POST};
use httpmock::MockServer;
use kupferdraht::keystore::{InMemoryKeyStore, SecureKeyStore};
use kupferdraht::{Client, ClientBuilder, KupferdrahtError, SyncSettings};

mod common;

const DOMAIN: &str = "example.test";

/// Login + device registration against `server`; the registration
/// result is returned, not unwrapped.
async fn login_and_register(server: &MockServer) -> Result<Arc<Client>, KupferdrahtError> {
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
        .await?
        .register_device()
        .await
}

fn assert_encoding(result: Result<Arc<Client>, KupferdrahtError>, what: &str) {
    match result {
        Err(KupferdrahtError::Encoding(_)) => {}
        Err(other) => panic!("{what}: expected Encoding, got {other:?}"),
        Ok(_) => panic!("{what}: expected Encoding, got a client"),
    }
}

// ---- login / registration responses ----

/// `/login` answering 200 with an empty `access_token` used to carry the
/// empty bearer into the next call's `debug_assert!`.
#[tokio::test]
async fn an_empty_login_access_token_is_an_encoding_error() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/v14/login");
        then.status(200)
            .header("content-type", "application/json")
            .header("set-cookie", "zuid=test; HttpOnly; Secure; Path=/access")
            .body(
                r#"{"access_token":"","token_type":"Bearer","expires_in":900,"user":"user-uuid"}"#,
            );
    });
    common::mount_full_login_flow(&server);

    assert_encoding(login_and_register(&server).await, "empty login token");
}

/// `POST /clients` answering with an empty `id` used to become the
/// device id — asserted by the rebind, the key-package upload and, in
/// release builds too, the push loop.
#[tokio::test]
async fn an_empty_registered_client_id_is_an_encoding_error() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/v14/clients");
        then.status(201)
            .header("content-type", "application/json")
            .body(r#"{"id":"","type":"permanent"}"#);
    });
    common::mount_full_login_flow(&server);

    assert_encoding(login_and_register(&server).await, "empty client id");
}

/// The rebind `/access` answering with an empty `access_token`.
#[tokio::test]
async fn an_empty_rebound_access_token_is_an_encoding_error() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST)
            .path("/v14/access")
            .query_param("client_id", "new-client-id");
        then.status(200)
            .header("content-type", "application/json")
            .body(
                r#"{"access_token":"","token_type":"Bearer","expires_in":900,"user":"user-uuid"}"#,
            );
    });
    common::mount_full_login_flow(&server);

    assert_encoding(login_and_register(&server).await, "empty rebound token");
}

/// `GET /mls/public-keys` with an empty P-256 removal key used to be
/// persisted into a `debug_assert!` — and would be written into every
/// group's `external_senders` extension.
#[tokio::test]
async fn an_empty_backend_removal_key_is_an_encoding_error() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/v14/mls/public-keys");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"removal":{"ed25519":"ZWQ=","ecdsa_secp256r1_sha256":"","ecdsa_secp384r1_sha384":"cDM4NA==","ecdsa_secp521r1_sha512":"cDUyMQ=="}}"#);
    });
    common::mount_full_login_flow(&server);

    assert_encoding(login_and_register(&server).await, "empty removal key");
}

/// `/self` with an empty `qualified_id.domain` used to build the account's
/// user id through `UserId::new`'s `debug_assert!`s.
#[tokio::test]
async fn an_empty_self_domain_is_an_encoding_error() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/v14/self");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"id":"user-uuid","qualified_id":{"id":"user-uuid","domain":""},"handle":"alice","name":"Alice Example","email":"alice@example.test"}"#);
    });
    common::mount_full_login_flow(&server);

    assert_encoding(login_and_register(&server).await, "empty self domain");
}

// ---- conversation metadata ----

fn conversation(id: &str, group_id_b64: &str) -> String {
    format!(
        r#"{{"qualified_id":{{"id":"{id}","domain":"{DOMAIN}"}},
             "type":0,"protocol":"mls","epoch":1,"group_id":"{group_id_b64}",
             "name":"Room {id}",
             "members":{{
               "self":{{"qualified_id":{{"id":"user-uuid","domain":"{DOMAIN}"}},
                       "conversation_role":"wire_member"}},
               "others":[]
             }}}}"#
    )
}

fn mount_conversations(server: &MockServer, ids: &[&str], found: &[String]) {
    let listed: Vec<String> = ids
        .iter()
        .map(|id| format!(r#"{{"id":"{id}","domain":"{DOMAIN}"}}"#))
        .collect();
    let list_ids_body = format!(
        r#"{{"qualified_conversations":[{}],"has_more":false,"paging_state":"END"}}"#,
        listed.join(",")
    );
    server.mock(move |when, then| {
        when.method(POST).path("/v14/conversations/list-ids");
        then.status(200)
            .header("content-type", "application/json")
            .body(list_ids_body.clone());
    });
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
    server.mock(|when, then| {
        when.method(POST).path("/v14/list-users");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"found":[]}"#);
    });
    common::mount_empty_notifications(server);
}

/// A conversation whose `qualified_id.id` is empty becomes the room id
/// `@example.test` — unaddressable, and asserted by every per-room
/// endpoint that splits it. It is skipped at the metadata boundary.
#[tokio::test]
async fn a_conversation_without_an_id_is_skipped() {
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    let (client, _store) = common::run_login(&server).await;
    mount_conversations(
        &server,
        &["", "95c0bb01-0000-4000-8000-000000000001"],
        &[
            conversation("", "Z3JwLWE="),
            conversation("95c0bb01-0000-4000-8000-000000000001", "Z3JwLWI="),
        ],
    );

    client
        .sync_once(SyncSettings::default())
        .await
        .expect("a malformed conversation is not a sync failure");

    let ids: Vec<String> = client
        .rooms()
        .await
        .iter()
        .map(|r| r.id().to_string())
        .collect();
    assert_eq!(
        ids,
        vec![format!("95c0bb01-0000-4000-8000-000000000001@{DOMAIN}")]
    );
}

/// A conversation whose `group_id` is `""` decoded to an empty MLS group
/// id, which every MLS entry point debug-asserts. It is a room without an
/// MLS group now.
#[tokio::test]
async fn an_empty_group_id_is_no_group() {
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    let (client, _store) = common::run_login(&server).await;
    let room_uuid = "95c0bb02-0000-4000-8000-000000000002";
    mount_conversations(&server, &[room_uuid], &[conversation(room_uuid, "")]);

    client
        .sync_once(SyncSettings::default())
        .await
        .expect("an empty group id is not a sync failure");

    let room = client
        .room(&format!("{room_uuid}@{DOMAIN}"))
        .await
        .expect("the room itself is kept");
    assert_eq!(room.group_id_bytes(), None);
}

/// Wire caps `POST /conversations/list` at 1000 ids. An account in more
/// conversations used to send them all in one request — a
/// `debug_assert!` in debug builds, a rejected metadata sync on every
/// cycle in release. The ids are fetched in chunks of at most 1000.
#[tokio::test]
async fn more_than_1000_conversations_are_listed_in_chunks() {
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    let (client, _store) = common::run_login(&server).await;
    let listed: Vec<String> = (0..1001)
        .map(|n| format!(r#"{{"id":"95c0cc00-0000-4000-8000-{n:012}","domain":"{DOMAIN}"}}"#))
        .collect();
    let list_ids_body = format!(
        r#"{{"qualified_conversations":[{}],"has_more":false,"paging_state":"END"}}"#,
        listed.join(",")
    );
    server.mock(move |when, then| {
        when.method(POST).path("/v14/conversations/list-ids");
        then.status(200)
            .header("content-type", "application/json")
            .body(list_ids_body.clone());
    });
    let within_cap = server.mock(|when, then| {
        when.method(POST)
            .path("/v14/conversations/list")
            .is_true(|req| {
                serde_json::from_slice::<serde_json::Value>(req.body_ref())
                    .ok()
                    .and_then(|v| v.get("qualified_ids")?.as_array().map(Vec::len))
                    .is_some_and(|n| (1..=1000).contains(&n))
            });
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"found":[],"not_found":[],"failed":[]}"#);
    });
    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list");
        then.status(400)
            .header("content-type", "application/json")
            .body(r#"{"code":400,"label":"too-many-ids","message":"at most 1000"}"#);
    });
    server.mock(|when, then| {
        when.method(POST).path("/v14/list-users");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"found":[]}"#);
    });
    common::mount_empty_notifications(&server);

    client
        .sync_once(SyncSettings::default())
        .await
        .expect("1001 conversations sync");

    within_cap.assert_calls(2);
}

/// A conversation whose `team` is `""` used to put the empty id into the
/// team-scoped DELETE path (a `debug_assert!`). An empty team is no team:
/// `delete_room` falls back to the account's own team, as for a missing
/// one.
#[tokio::test]
async fn an_empty_conversation_team_falls_back_to_the_own_team() {
    let server = MockServer::start();
    let room_uuid = "95c0bb03-0000-4000-8000-000000000003";
    server.mock(|when, then| {
        when.method(GET).path("/v14/self");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"id":"user-uuid","qualified_id":{"id":"user-uuid","domain":"example.test"},"handle":"alice","name":"Alice Example","email":"alice@example.test","team":"team-own"}"#);
    });
    common::mount_full_login_flow(&server);
    let (client, _store) = common::run_login(&server).await;
    let with_empty_team =
        conversation(room_uuid, "Z3JwLWM=").replacen(r#""name":"#, r#""team":"","name":"#, 1);
    mount_conversations(&server, &[room_uuid], &[with_empty_team]);
    let delete = server.mock(|when, then| {
        when.method(DELETE)
            .path(format!("/v14/teams/team-own/conversations/{room_uuid}"));
        then.status(200);
    });

    client
        .delete_room(&format!("{room_uuid}@{DOMAIN}"))
        .await
        .expect("delete via the own team");

    delete.assert_calls(1);
}
