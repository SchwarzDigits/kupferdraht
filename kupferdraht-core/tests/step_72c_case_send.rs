// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 72c — case send + invite re-broadcast (TDD red).
//!
//! Completes ADR-0018: `Room::set_case` patches the fold, bumps
//! `revision`, sends the full snapshot (client-side admin gate), and
//! `invite_members` re-broadcasts the current snapshot right after
//! the Add-commit — the MUST that closes the MLS no-history late-join
//! gap. Fixtures mirror `step_72b_case_fold.rs`: the client is
//! welcomed into a fixture peer's group so it can encrypt, and the
//! metadata mock controls the self member's `conversation_role`.

use std::sync::{Arc, Mutex};

use base64::Engine as _;
use httpmock::Method::{GET, POST};
use httpmock::{Mock, MockServer, prelude::HttpMockRequest};
use kupferdraht::storage::Store;
use kupferdraht::timeline::{CasePatch, CaseState};
use kupferdraht::{ClientBuilder, SyncSettings};

mod common;

const ROOM_UUID: &str = "77aa31c5-52a2-4b58-9bf6-1b40e64aecc1";
const DOMAIN: &str = "example.test";
/// b64("grp-casesend")
const GROUP_ID_B64: &str = "Z3JwLWNhc2VzZW5k";
const BOB_IDENTITY: &[u8] = b"bob-uuid:bobdev01@example.test";
const DAVE: &str = "dave-uuid";

static UPLOADED_KPS: Mutex<Vec<(String, Vec<String>)>> = Mutex::new(Vec::new());

fn capture_key_package_upload(req: &HttpMockRequest) -> bool {
    let host = req
        .headers()
        .get("host")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
        .unwrap_or_default();
    let body = req.body_ref();
    if let Ok(v) = serde_json::from_slice::<serde_json::Value>(body)
        && let Some(arr) = v.get("key_packages").and_then(serde_json::Value::as_array)
    {
        let kps: Vec<String> = arr
            .iter()
            .filter_map(|s| s.as_str().map(String::from))
            .collect();
        UPLOADED_KPS.lock().expect("capture lock").push((host, kps));
    }
    true
}

fn captured_key_package_for(server: &MockServer) -> Vec<u8> {
    let host = server.address().to_string();
    let guard = UPLOADED_KPS.lock().expect("capture lock");
    let (_, kps) = guard
        .iter()
        .rev()
        .find(|(h, _)| *h == host)
        .expect("register_device uploaded key packages");
    let first = kps.first().expect("at least one key package uploaded");
    base64::engine::general_purpose::STANDARD
        .decode(first)
        .expect("uploaded key package is base64")
}

/// Metadata: self carries `self_role`; bob (the fixture peer) is the
/// only other member — dave is the invite's CLAIM target, not a
/// member (review fix 2026-08-21).
fn mount_room(server: &MockServer, self_role: &str) {
    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list-ids");
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(
                r#"{{"qualified_conversations":[
                     {{"id":"{ROOM_UUID}","domain":"{DOMAIN}"}}
                   ],"has_more":false,"paging_state":"END"}}"#
            ));
    });
    let self_role = self_role.to_string();
    server.mock(move |when, then| {
        when.method(POST).path("/v14/conversations/list");
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(
                r#"{{"found":[{{
                     "qualified_id":{{"id":"{ROOM_UUID}","domain":"{DOMAIN}"}},
                     "type":0,"protocol":"mls","epoch":1,"group_id":"{GROUP_ID_B64}",
                     "name":"CaseSend",
                     "members":{{
                       "self":{{"qualified_id":{{"id":"user-uuid","domain":"{DOMAIN}"}},
                               "conversation_role":"{self_role}"}},
                       "others":[
                         {{"qualified_id":{{"id":"bob-uuid","domain":"{DOMAIN}"}},
                          "conversation_role":"wire_admin"}}
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
}

fn mount_welcome_page(server: &MockServer, peer: &mut common::mls::PeerGroup) {
    let welcome = base64::engine::general_purpose::STANDARD
        .encode(peer.welcome_over_key_package(&captured_key_package_for(server)));
    let body = format!(
        r#"{{"notifications":[{{"id":"9100e858-8eae-11ef-0000-0000000cafe0","payload":[
             {{"type":"conversation.mls-welcome",
               "qualified_conversation":{{"id":"{ROOM_UUID}","domain":"{DOMAIN}"}},
               "qualified_from":{{"id":"bob-uuid","domain":"{DOMAIN}"}},
               "data":"{welcome}",
               "time":"2026-08-21T14:00:00.000Z"}}
           ]}}],"has_more":false}}"#
    );
    server.mock(move |when, then| {
        when.method(GET).path("/v14/notifications");
        then.status(200)
            .header("content-type", "application/json")
            .body(body.clone());
    });
}

fn mount_message_send(server: &MockServer) -> Mock<'_> {
    server.mock(|when, then| {
        when.method(POST).path("/v14/mls/messages");
        then.status(201)
            .header("content-type", "application/json")
            .body(r#"{"events":[],"time":"2026-08-21T14:00:01.000Z"}"#);
    })
}

/// Claim + commit-bundle mocks for the invite of dave.
fn mount_invite_machinery(server: &MockServer) -> Mock<'_> {
    server.mock(|when, then| {
        when.method(POST)
            .path(format!("/v14/mls/key-packages/claim/{DOMAIN}/user-uuid"));
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"key_packages":[]}"#);
    });
    let dave_kp = common::mls::key_package_b64(b"dave-uuid:davedev01@example.test");
    server.mock(move |when, then| {
        when.method(POST)
            .path(format!("/v14/mls/key-packages/claim/{DOMAIN}/{DAVE}"));
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(
                r#"{{"key_packages":[{{"user":"{DAVE}","domain":"{DOMAIN}","client":"davedev01",
                     "key_package":"{dave_kp}","key_package_ref":"cmVm"}}]}}"#
            ));
    });
    server.mock(|when, then| {
        when.method(POST).path("/v14/mls/commit-bundles");
        then.status(201)
            .header("content-type", "application/json")
            .body(r#"{"events":[],"time":"2026-08-21T14:00:02.000Z"}"#);
    })
}

async fn logged_in_client(server: &MockServer) -> (Arc<kupferdraht::Client>, Arc<dyn Store>) {
    server.mock(|when, then| {
        when.method(POST)
            .path_includes("/v14/mls/key-packages/self/")
            .is_true(capture_key_package_upload);
        then.status(201);
    });
    common::mount_full_login_flow(server);
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().to_path_buf();
    let _ = Box::leak(Box::new(dir));
    let keystore: Arc<dyn kupferdraht::keystore::SecureKeyStore> =
        Arc::new(kupferdraht::keystore::InMemoryKeyStore::new());
    let client = ClientBuilder::new()
        .server(common::server_config(&server.base_url()))
        .permit_http()
        .base_dir(path)
        .keystore(keystore)
        .login("alice@example.test".into(), "pw".into(), None)
        .await
        .expect("login")
        .register_device()
        .await
        .expect("register device");
    let store = client.store().expect("store");
    (client, store)
}

/// Login + welcome into the fixture group so the client can encrypt.
async fn welcomed_client(
    server: &MockServer,
    self_role: &str,
) -> (Arc<kupferdraht::Client>, Arc<kupferdraht::rooms::Room>) {
    let (client, _store) = logged_in_client(server).await;
    mount_room(server, self_role);
    let mut peer = common::mls::PeerGroup::new(b"grp-casesend", BOB_IDENTITY);
    mount_welcome_page(server, &mut peer);
    client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync");
    let room = client
        .room(&format!("{ROOM_UUID}@{DOMAIN}"))
        .await
        .expect("room cached");
    (client, room)
}

#[tokio::test]
async fn set_case_creates_updates_and_folds_locally() {
    let server = MockServer::start();
    let (_client, room) = welcomed_client(&server, "wire_admin").await;
    let send = mount_message_send(&server);

    // Create.
    let created = room
        .set_case(CasePatch {
            title: Some("Building permit, 1 Sample Street".into()),
            state: Some(CaseState::Active),
            external_ref: Some("REF 12/2026-K".into()),
            ..CasePatch::default()
        })
        .await
        .expect("set_case creates");
    assert_eq!(created.revision, 1, "first snapshot is revision 1");
    assert_eq!(created.state, CaseState::Active);
    assert!(!created.case_ref.is_empty(), "a case_ref was generated");
    assert_eq!(send.calls(), 1, "one full snapshot went out");

    // Update: patch one field, everything else survives.
    let updated = room
        .set_case(CasePatch {
            state: Some(CaseState::Waiting),
            state_detail: Some("awaiting a statement".into()),
            ..CasePatch::default()
        })
        .await
        .expect("set_case updates");
    assert_eq!(updated.revision, 2, "update bumps the revision");
    assert_eq!(updated.state, CaseState::Waiting);
    assert_eq!(
        updated.title.as_deref(),
        Some("Building permit, 1 Sample Street"),
        "unpatched core fields survive"
    );
    assert_eq!(updated.case_ref, created.case_ref, "identity is stable");
    assert_eq!(send.calls(), 2);

    // The local fold sees the send immediately.
    let folded = room
        .case()
        .await
        .expect("case read")
        .expect("own sends fold locally");
    assert_eq!(folded.revision, 2);
    assert_eq!(folded.state, CaseState::Waiting);
}

#[tokio::test]
async fn set_case_rejects_non_admin_sender_client_side() {
    let server = MockServer::start();
    let (_client, room) = welcomed_client(&server, "wire_member").await;
    let send = mount_message_send(&server);

    let err = room
        .set_case(CasePatch {
            title: Some("hijack".into()),
            ..CasePatch::default()
        })
        .await
        .expect_err("a non-admin must be rejected before any traffic");
    assert!(
        matches!(err, kupferdraht::KupferdrahtError::InvalidInput(_)),
        "expected InvalidInput, got {err:?}"
    );
    assert_eq!(send.calls(), 0, "nothing went out");
    assert!(
        room.case().await.expect("case read").is_none(),
        "no local fold from a rejected send"
    );
}

#[tokio::test]
async fn invite_rebroadcasts_the_current_snapshot() {
    let server = MockServer::start();
    let (client, room) = welcomed_client(&server, "wire_admin").await;
    let send = mount_message_send(&server);
    let commit = mount_invite_machinery(&server);

    room.set_case(CasePatch {
        title: Some("Fall X".into()),
        ..CasePatch::default()
    })
    .await
    .expect("set_case");
    assert_eq!(send.calls(), 1);

    client
        .invite_members(&room.id(), &[format!("{DAVE}@{DOMAIN}")])
        .await
        .expect("invite");
    assert_eq!(commit.calls(), 1, "the Add-commit went out");
    assert_eq!(
        send.calls(),
        2,
        "the invite re-broadcast the current snapshot (ADR-0018 MUST)"
    );
    let folded = room
        .case()
        .await
        .expect("case read")
        .expect("case still folded");
    assert_eq!(
        folded.revision, 1,
        "a re-broadcast repeats the snapshot — it is not an update"
    );
}

#[tokio::test]
async fn invite_without_case_sends_no_snapshot() {
    let server = MockServer::start();
    let (client, room) = welcomed_client(&server, "wire_admin").await;
    let send = mount_message_send(&server);
    let commit = mount_invite_machinery(&server);

    client
        .invite_members(&room.id(), &[format!("{DAVE}@{DOMAIN}")])
        .await
        .expect("invite");
    assert_eq!(commit.calls(), 1);
    assert_eq!(send.calls(), 0, "no case, no re-broadcast");
    let _ = room;
}
