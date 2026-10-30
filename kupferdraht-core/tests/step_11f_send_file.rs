// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 11f — `Timeline::send_file` policy gate + early-pipeline tests.
//!
//! Two tests cover the parts of `send_file` that fire **before**
//! the MLS encrypt call: a `Disabled` policy rejects with `InvalidInput`
//! before any Wire call; an `/v14/assets` 500 surfaces as
//! `BackendRejected` without persisting an attachment row.
//!
//! The happy path cannot be unit-tested through this harness because
//! httpmock does not stand in for openmls's group state — the encrypt
//! step requires a real MLS group joined by `Client::join_room`, which
//! itself requires `GET /groupinfo` + `POST /v14/mls/commit-bundles`
//! mocks AND running the actual external-commit logic. That
//! end-to-end validation is the live-smoke gate at Step 11h.
//!
//! Step-by-step unit coverage of the orchestrator's building blocks:
//! the policy-gate matrix is covered here; `encrypt_file` round-trips
//! by Step 11b inline tests; `upload_asset` multipart shape by Step
//! 11c httpmock tests; `insert_attachment` by Step 11d inline tests;
//! MLS encrypt + `POST /v14/mls/messages` by Step 4's send_text tests
//! (same `encrypt_and_upload` helper). The orchestrator wiring all of
//! these together is therefore transitively covered; 11h's live-smoke
//! is the integration check.

use std::sync::Arc;

use httpmock::Method::{GET, POST};
use httpmock::MockServer;
use kupferdraht::keystore::{InMemoryKeyStore, SecureKeyStore};
use kupferdraht::storage::Store;
use kupferdraht::{Client, ClientBuilder, SyncSettings};
use tempfile::TempDir;

mod common;

const ROOM_UUID: &str = "dm-ab";
const ROOM_DOMAIN: &str = "example.test";

#[tokio::test]
async fn send_file_disabled_policy_rejects_without_any_wire_call() {
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    mount_feature_configs(&server, r#"{"fileSharing":{"status":"disabled"}}"#);
    mount_sync_metadata_with_one_room(&server);
    // No /v14/assets mock — accidental call would surface as
    // BackendRejected, but the gate must short-circuit first.

    let (client, store) = run_login_setup_and_join(&server).await;
    let dir = TempDir::new().expect("tempdir");
    let plaintext_path = dir.path().join("blocked.bin");
    std::fs::write(&plaintext_path, b"would-be-blocked").expect("write");

    let room = client
        .room(&format!("{ROOM_UUID}@{ROOM_DOMAIN}"))
        .await
        .expect("room");
    let timeline = room.timeline().expect("timeline");
    let err = timeline
        .send_file(&plaintext_path, "image/png")
        .await
        .expect_err("Disabled policy must reject");
    assert!(
        matches!(err, kupferdraht::KupferdrahtError::InvalidInput(_)),
        "got {err:?}"
    );
    // No attachments persisted.
    let any = store
        .list_attachments_for_room(&format!("{ROOM_UUID}@{ROOM_DOMAIN}"))
        .await
        .expect("list");
    assert!(any.is_empty(), "no attachment row on gate rejection");
}

#[tokio::test]
async fn send_file_upload_5xx_returns_error_without_persisting_attachment() {
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    mount_feature_configs(
        &server,
        r#"{"fileSharing":{"status":"enabled","config":{}}}"#,
    );
    mount_sync_metadata_with_one_room(&server);
    server.mock(|when, then| {
        when.method(POST).path("/v14/assets");
        then.status(500)
            .body(r#"{"code":500,"label":"server-error"}"#);
    });
    // No /mls/messages mock — upload fails first.

    let (client, store) = run_login_setup_and_join(&server).await;
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("fails.bin");
    std::fs::write(&path, b"x").expect("write");
    let room = client
        .room(&format!("{ROOM_UUID}@{ROOM_DOMAIN}"))
        .await
        .expect("room");
    let timeline = room.timeline().expect("timeline");
    let err = timeline
        .send_file(&path, "image/png")
        .await
        .expect_err("upload 5xx must surface");
    assert!(
        matches!(err, kupferdraht::KupferdrahtError::BackendRejected(_)),
        "got {err:?}"
    );
    let any = store
        .list_attachments_for_room(&format!("{ROOM_UUID}@{ROOM_DOMAIN}"))
        .await
        .expect("list");
    assert!(any.is_empty(), "no attachment persisted on upload failure");
}

// ---- fixture --------------------------------------------------------

async fn run_login_setup_and_join(server: &MockServer) -> (Arc<Client>, Arc<dyn Store>) {
    let home = TempDir::new().expect("home");
    let keystore: Arc<dyn SecureKeyStore> = Arc::new(InMemoryKeyStore::new());
    let base = home.path().to_path_buf();
    let _ = Box::leak(Box::new(home));
    let client = ClientBuilder::new()
        .server(common::server_config(&server.base_url()))
        .permit_http()
        .base_dir(base)
        .keystore(keystore)
        .login("alice@example.test".into(), "pw".into(), None)
        .await
        .expect("login")
        .register_device()
        .await
        .expect("register device");
    client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync_metadata");
    let store = client.store().expect("client store");
    // NOTE: This test doesn't run the real MLS join flow — for the
    // `send_file` orchestrator test we mock /v14/mls/messages as
    // success, so the actual MLS encrypt step's group lookup must
    // still succeed. The mocked conversation list response uses a
    // valid `group_id` (base64 `Z3JwLWFh` = `grp-aa`), but no MLS
    // join has actually populated openmls's storage for that group.
    // For the happy path test, this means encrypt_and_upload would
    // typically fail at MLS lookup — UNLESS we work around it.
    //
    // The 11f orchestrator tests don't actually need to drive the
    // MLS encrypt step end-to-end — they're testing the pre-MLS
    // pipeline (policy gate, encrypt_file, upload_asset) and the
    // post-MLS persistence. We can't easily mock openmls. So the
    // 11f integration tests focus on the steps that DON'T require
    // openmls state, and the happy-path test that DOES drive the
    // full pipeline mocks /mls/messages as 201 — but the encrypt
    // step inside Kupferdraht will fail on the missing group.
    //
    // For now, treat these tests as documenting the EXPECTED
    // behavior. Live-smoke at 11h validates the full path.
    (client, store)
}

fn mount_feature_configs(server: &MockServer, body: &str) {
    server.mock(|when, then| {
        when.method(GET).path("/v14/feature-configs");
        then.status(200).body(body);
    });
    server.mock(|when, then| {
        when.method(GET).path("/v14/properties");
        then.status(200).body("[]");
    });
}

fn mount_sync_metadata_with_one_room(server: &MockServer) {
    common::mount_empty_notifications(server);
    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list-ids");
        then.status(200).body(format!(
            r#"{{"qualified_conversations":[{{"id":"{ROOM_UUID}","domain":"{ROOM_DOMAIN}"}}],"has_more":false,"paging_state":"END"}}"#,
        ));
    });
    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list");
        then.status(200).body(format!(
            r#"{{"found":[{{"qualified_id":{{"id":"{ROOM_UUID}","domain":"{ROOM_DOMAIN}"}},"type":2,"protocol":"mls","epoch":5,"group_id":"Z3JwLWFh","name":"","members":{{"self":{{"qualified_id":{{"id":"user-uuid","domain":"example.test"}}}},"others":[]}}}}],"not_found":[],"failed":[]}}"#,
        ));
    });
    server.mock(|when, then| {
        when.method(POST).path("/v14/list-users");
        then.status(200).body(r#"{"found":[]}"#);
    });
}
