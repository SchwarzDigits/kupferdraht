// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 106 — heal a room only on a confirmed gap
//! (`docs/plan/archive/106-narrow-healing.md`, ADR-0034 d.1, review finding H4).
//!
//! Bob advances the group's epoch without the client seeing the commit,
//! then sends a message at the new epoch. The client cannot decrypt it.
//! Whether that is a real gap is the backend's call: its conversation
//! record at the client's epoch means nothing was missed — no rejoin; at
//! Bob's epoch the room heals (discard + external rejoin, observable as
//! the `groupinfo` fetch).

use std::sync::{Arc, Mutex};

use base64::Engine as _;
use httpmock::Method::{GET, POST};
use httpmock::{Mock, MockServer, prelude::HttpMockRequest};
use kupferdraht::{ClientBuilder, SyncSettings};

mod common;

use common::mls::{PeerDevice, PeerGroup};

const DOMAIN: &str = "example.test";
const ROOM_UUID: &str = "106a0a31-77aa-4bde-9d10-223344556677";
/// base64("grp-106")
const GROUP_ID_B64: &str = "Z3JwLTEwNg==";
const GROUP_ID: &[u8] = b"grp-106";
const SELF: &str = "user-uuid";
const BOB: &str = "bob-uuid";

static UPLOADED_KPS: Mutex<Vec<(String, Vec<String>)>> = Mutex::new(Vec::new());

fn capture_key_package_upload(req: &HttpMockRequest) -> bool {
    let host = req
        .headers()
        .get("host")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
        .unwrap_or_default();
    if let Ok(v) = serde_json::from_slice::<serde_json::Value>(req.body_ref())
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

/// The client's most recent key-package upload on this server's host
/// (httpmock pools servers — LEARNINGS §5.13).
fn captured_key_package(server: &MockServer) -> Vec<u8> {
    let host = server.address().to_string();
    let guard = UPLOADED_KPS.lock().expect("capture lock");
    let (_, kps) = guard
        .iter()
        .rev()
        .find(|(h, _)| *h == host)
        .expect("register_device uploaded key packages");
    base64::engine::general_purpose::STANDARD
        .decode(kps.first().expect("a key package"))
        .expect("uploaded key package is base64")
}

async fn logged_in_client(server: &MockServer) -> Arc<kupferdraht::Client> {
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

/// The room ids plus the conversation record, the latter at `epoch` —
/// the backend's word on the group's current epoch.
fn mount_rooms(server: &MockServer, epoch: u64) -> Mock<'_> {
    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list-ids");
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(
                r#"{{"qualified_conversations":[{{"id":"{ROOM_UUID}","domain":"{DOMAIN}"}}],
                     "has_more":false,"paging_state":"END"}}"#
            ));
    });
    server.mock(|when, then| {
        when.method(POST).path("/v14/list-users");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"found":[]}"#);
    });
    server.mock(move |when, then| {
        when.method(POST).path("/v14/conversations/list");
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(
                r#"{{"found":[{{"qualified_id":{{"id":"{ROOM_UUID}","domain":"{DOMAIN}"}},
                     "type":0,"protocol":"mls","epoch":{epoch},"group_id":"{GROUP_ID_B64}",
                     "name":"Healing",
                     "members":{{
                       "self":{{"qualified_id":{{"id":"{SELF}","domain":"{DOMAIN}"}},
                               "conversation_role":"wire_member"}},
                       "others":[{{"qualified_id":{{"id":"{BOB}","domain":"{DOMAIN}"}},
                                  "conversation_role":"wire_member"}}]
                     }}}}],"not_found":[],"failed":[]}}"#
            ));
    })
}

/// A rejoin fetches the group info first; answering 404 keeps the
/// attempt observable without completing it.
fn mount_group_info(server: &MockServer) -> Mock<'_> {
    server.mock(|when, then| {
        when.method(GET).path_includes("/groupinfo");
        then.status(404)
            .header("content-type", "application/json")
            .body(r#"{"code":404,"label":"mls-missing-group-info","message":"none"}"#);
    })
}

fn mount_page<'a>(server: &'a MockServer, notif_id: &str, event: &str) -> Mock<'a> {
    let body = format!(
        r#"{{"notifications":[{{"id":"{notif_id}","payload":[{event}]}}],"has_more":false}}"#
    );
    server.mock(move |when, then| {
        when.method(GET).path("/v14/notifications");
        then.status(200)
            .header("content-type", "application/json")
            .body(body.clone());
    })
}

fn event(kind: &str, data: &[u8]) -> String {
    format!(
        r#"{{"type":"{kind}",
             "qualified_conversation":{{"id":"{ROOM_UUID}","domain":"{DOMAIN}"}},
             "qualified_from":{{"id":"{BOB}","domain":"{DOMAIN}"}},
             "data":"{}",
             "time":"2026-09-29T10:00:00.000Z"}}"#,
        base64::engine::general_purpose::STANDARD.encode(data)
    )
}

/// `GenericMessage { message_id, text { content } }`, hand-encoded.
fn text_message(message_id: &str, content: &str) -> Vec<u8> {
    fn field(tag: u8, bytes: &[u8], out: &mut Vec<u8>) {
        out.push(tag);
        out.push(u8::try_from(bytes.len()).expect("short field"));
        out.extend_from_slice(bytes);
    }
    let mut text = Vec::new();
    field(0x0a, content.as_bytes(), &mut text);
    let mut message = Vec::new();
    field(0x0a, message_id.as_bytes(), &mut message);
    field(0x12, &text, &mut message);
    message
}

/// The client joins at epoch 1; Bob then moves the group to epoch 2
/// (adding his second device — a commit the client never receives) and
/// sends a message there. Returns the client and the group-info mock
/// after the second sync, with the backend reporting `backend_epoch`.
async fn gap_scenario(
    server: &MockServer,
    backend_epoch: u64,
) -> (Arc<kupferdraht::Client>, usize) {
    let client = logged_in_client(server).await;
    let mut rooms = mount_rooms(server, 1);
    let group_info = mount_group_info(server);
    let mut bob = PeerGroup::new(GROUP_ID, b"bob-uuid:bobdev01@example.test");
    let (_commit, welcome) = bob.add_member(&captured_key_package(server));
    let mut first = mount_page(
        server,
        "106a0000-8eae-11ef-0000-000000000001",
        &event("conversation.mls-welcome", &welcome),
    );
    client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync_once (welcome)");

    let bob2 = PeerDevice::new(b"bob-uuid:bobdev02@example.test");
    let _unseen_commit = bob.add_member(&bob2.key_package_tls());
    assert_eq!(bob.epoch(), 2, "bob moved on without the client");
    let message = bob.application_message(&text_message("m-future", "from epoch 2"));
    first.delete();
    rooms.delete();
    let _rooms = mount_rooms(server, backend_epoch);
    let _second = mount_page(
        server,
        "106a0000-8eae-11ef-0000-000000000002",
        &event("conversation.mls-message-add", &message),
    );
    client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync_once (future-epoch message)");
    let calls = group_info.calls();
    (client, calls)
}

#[tokio::test]
async fn a_future_epoch_the_backend_does_not_confirm_is_not_healed() {
    let server = MockServer::start();
    let (_client, rejoins) = gap_scenario(&server, 1).await;
    assert_eq!(
        rejoins, 0,
        "the backend is still at the client's epoch — no discard + rejoin"
    );
}

#[tokio::test]
async fn a_gap_the_backend_confirms_is_healed() {
    let server = MockServer::start();
    let (_client, rejoins) = gap_scenario(&server, 2).await;
    assert!(
        rejoins >= 1,
        "the backend is ahead — the room rejoins (group info fetched)"
    );
}
