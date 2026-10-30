// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 2 cycle 2d — rooms + room_members persistence.
//!
//! Drives `Client::sync_metadata` with a `ClientBuilder::store(...)`
//! attached and then:
//!
//! * inspects the `kupferdraht.db` tables directly to confirm the
//!   snapshot landed;
//! * re-hydrates a fresh `Client` via `Client::load_rooms_from_store`
//!   and confirms the cache matches what `sync_metadata` produced.

use std::sync::Arc;

use httpmock::Method::POST;
use httpmock::MockServer;
use kupferdraht::keystore::{InMemoryKeyStore, SecureKeyStore};
use kupferdraht::{ClientBuilder, RoomType, SyncSettings};
use tempfile::TempDir;

mod common;

fn mount_sync_metadata_mocks(server: &MockServer) {
    common::mount_empty_notifications(server);
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
            .body(
                r#"{"found":[
                     {
                       "qualified_id":{"id":"dm-ab","domain":"example.test"},
                       "type":2,
                       "protocol":"mls",
                       "epoch":5,
                       "group_id":"Z3JwLWFh",
                       "name":"",
                       "members":{
                         "self":{"qualified_id":{"id":"user-uuid","domain":"example.test"}},
                         "others":[{"qualified_id":{"id":"bob","domain":"example.test"}}]
                       }
                     }
                   ], "not_found":[], "failed":[]}"#,
            );
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
}

async fn logged_in_client(
    server: &MockServer,
    base_dir: &TempDir,
    keystore: Arc<dyn SecureKeyStore>,
) -> std::sync::Arc<kupferdraht::Client> {
    common::mount_full_login_flow(server);
    ClientBuilder::new()
        .server(common::server_config(&server.base_url()))
        .permit_http()
        .base_dir(base_dir.path().to_path_buf())
        .keystore(keystore)
        .login("alice@example.test".into(), "pw".into(), None)
        .await
        .expect("login")
        .register_device()
        .await
        .expect("register device")
}

#[tokio::test]
async fn sync_metadata_persists_rooms_into_store() {
    let dir = tempfile::tempdir().expect("tempdir");
    let keystore: Arc<dyn SecureKeyStore> = Arc::new(InMemoryKeyStore::new());
    let server = MockServer::start();
    let client = logged_in_client(&server, &dir, keystore).await;
    let store = client.store().expect("client store");
    mount_sync_metadata_mocks(&server);

    client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync");

    // Read directly from the Store trait — same path the CLI uses.
    let rows = store.load_rooms().await.expect("load_rooms");
    assert_eq!(rows.len(), 1);
    let (room, members) = &rows[0];
    assert_eq!(room.room_id, "dm-ab@example.test");
    assert_eq!(room.room_type, "direct_message");
    assert_eq!(room.epoch, 5);
    assert_eq!(
        room.counterpart_user_id.as_deref(),
        Some("bob@example.test")
    );
    assert_eq!(members.len(), 2);
    // self first (ORDER BY is_self DESC)
    assert!(members[0].is_self);
    assert_eq!(members[0].user_id, "user-uuid@example.test");
    assert!(!members[1].is_self);
    assert_eq!(members[1].user_id, "bob@example.test");
    assert_eq!(members[1].display_name.as_deref(), Some("Bob Example"));
    assert_eq!(members[1].handle.as_deref(), Some("bobx"));
}

#[tokio::test]
async fn sync_metadata_replaces_prior_snapshot_in_store() {
    let dir = tempfile::tempdir().expect("tempdir");
    let keystore: Arc<dyn SecureKeyStore> = Arc::new(InMemoryKeyStore::new());
    let server = MockServer::start();
    let client = logged_in_client(&server, &dir, keystore).await;
    let store = client.store().expect("client store");
    mount_sync_metadata_mocks(&server);

    client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync #1");
    assert_eq!(store.load_rooms().await.expect("load #1").len(), 1);

    // Second sync — same endpoints, same data: after the replace
    // there must still be exactly one row, not a duplicate.
    client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync #2");
    let rows = store.load_rooms().await.expect("load #2");
    assert_eq!(rows.len(), 1, "replace_rooms must drop duplicates");
}

#[tokio::test]
async fn load_rooms_from_store_hydrates_in_memory_cache() {
    let server = MockServer::start();

    // Both logins share the same kupferdraht-home + keystore so the
    // per-account database key (stored in the keystore under
    // `account-<id>`) is reproducible across the simulated process
    // restart. Without this, the second `unlock` would generate a
    // fresh key and fail to decrypt the rooms persisted by the first
    // client.
    let home = tempfile::tempdir().expect("tempdir for kupferdraht_home");
    let keystore: Arc<dyn SecureKeyStore> = Arc::new(InMemoryKeyStore::new());

    common::mount_full_login_flow(&server);
    let client = ClientBuilder::new()
        .server(common::server_config(&server.base_url()))
        .permit_http()
        .base_dir(home.path().to_path_buf())
        .keystore(keystore.clone())
        .login("alice@example.test".into(), "pw".into(), None)
        .await
        .expect("login")
        .register_device()
        .await
        .expect("register device");
    mount_sync_metadata_mocks(&server);

    client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync");
    // Deliberately *do not* touch the in-memory cache via sync
    // for the second client — instead login afresh, hydrate from
    // store, and check the cache matches.
    //
    // A second login against the same mocks works because the
    // login-flow mocks are permissive (they don't assert counts).
    let fresh_client = ClientBuilder::new()
        .server(common::server_config(&server.base_url()))
        .permit_http()
        .base_dir(home.path().to_path_buf())
        .keystore(keystore)
        .login("alice@example.test".into(), "pw".into(), None)
        .await
        .expect("second login reuses mocks")
        .register_device()
        .await
        .expect("register device");

    // Before hydrate: empty.
    assert!(fresh_client.rooms().await.is_empty());
    // After hydrate: one DM with counterpart.
    fresh_client.load_rooms_from_store().await.expect("hydrate");
    let rooms = fresh_client.rooms().await;
    assert_eq!(rooms.len(), 1);
    assert_eq!(rooms[0].id(), "dm-ab@example.test");
    assert_eq!(rooms[0].room_type(), RoomType::DirectMessage);
    let cp = rooms[0]
        .counterpart()
        .expect("DM counterpart persisted across sync + hydrate");
    assert_eq!(cp.user_id, "bob@example.test");
    assert_eq!(cp.display_name.as_deref(), Some("Bob Example"));
    assert_eq!(cp.handle.as_deref(), Some("bobx"));
}

#[tokio::test]
async fn load_rooms_from_store_errors_when_no_store_attached() {
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    let client = ClientBuilder::new()
        .server(common::server_config(&server.base_url()))
        .permit_http()
        .login("alice@example.test".into(), "pw".into(), None)
        .await
        .expect("login without store")
        .register_device()
        .await
        .expect("register device");
    let err = client
        .load_rooms_from_store()
        .await
        .expect_err("no-store client must reject hydrate");
    assert!(
        matches!(err, kupferdraht::KupferdrahtError::InvalidInput(_)),
        "expected InvalidInput, got {err:?}"
    );
}
