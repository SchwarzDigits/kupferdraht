// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 69 — absorb `conversation.mls-welcome` (TDD red phase).
//!
//! Pins `docs/plan/69-welcome-absorption.md`: the sync drain joins
//! the group at the Welcome's epoch instead of logging + skipping
//! (the Step-6 leftover that costs the receiver the initiator's
//! first message — observations §67, gap-B.24). The Welcome is a
//! real openmls artifact addressed to the test client: a fn-pointer
//! matcher captures the key packages the client uploads at
//! `register_device` (keyed by the mock server's Host so parallel
//! tests stay isolated), and the `common::mls` fixture peer builds
//! a path-less Add-commit over one of them.

use std::sync::{Arc, Mutex};

use base64::Engine as _;
use httpmock::Method::{GET, POST};
use httpmock::{Mock, MockServer, prelude::HttpMockRequest};
use kupferdraht::storage::Store;
use kupferdraht::{ClientBuilder, SyncSettings};

mod common;

const ROOM_UUID: &str = "4b1ff12e-52a2-4b58-9bf6-1b40e64ae7a1";
const DOMAIN: &str = "example.test";
/// b64("grp-wl") — the welcomed conversation's MLS group id.
const GROUP_ID_B64: &str = "Z3JwLXds";
const PEER_IDENTITY: &[u8] = b"bob-uuid:bobdev01@example.test";

/// Key packages captured from `POST /v14/mls/key-packages/self/{c}`,
/// keyed by the request's Host header (fn-pointer matchers cannot
/// capture, so the state lives in a static). Host is NOT unique
/// across the process: httpmock pools its servers, so a test that
/// starts after another finished can receive the same `host:port`
/// — the lookup therefore takes the **most recent** entry per host
/// (each test uploads exactly once during its server's lifetime).
/// Live-bitten 2026-08-17: green locally (all three tests
/// concurrent on distinct servers), red on the 2-core CI runner
/// (third test on a recycled server picked up a foreign key
/// package → `NoMatchingKeyPackage` → `MlsGroupMissing`).
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

/// One uploaded key package (TLS bytes) of the client talking to
/// `server` — the artifact the fixture peer's Welcome encrypts to.
fn captured_key_package_for(server: &MockServer) -> Vec<u8> {
    let host = server.address().to_string();
    let guard = UPLOADED_KPS.lock().expect("capture lock");
    // Most recent upload for this host — see `UPLOADED_KPS`.
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

/// Rooms metadata listing the welcomed conversation (epoch 1 — the
/// Add-commit established it server-side).
fn mount_rooms_with_group(server: &MockServer) {
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
    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list");
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(
                r#"{{"found":[{{
                     "qualified_id":{{"id":"{ROOM_UUID}","domain":"{DOMAIN}"}},
                     "type":0,"protocol":"mls","epoch":1,"group_id":"{GROUP_ID_B64}",
                     "name":"W",
                     "members":{{
                       "self":{{"qualified_id":{{"id":"user-uuid","domain":"{DOMAIN}"}}}},
                       "others":[{{"qualified_id":{{"id":"bob-uuid","domain":"{DOMAIN}"}}}}]
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

/// The notifications queue: one frame whose payload is `events`
/// (pre-rendered JSON objects).
fn mount_notifications_page(server: &MockServer, events: &[String]) {
    let payload = events.join(",");
    let body = format!(
        r#"{{"notifications":[{{"id":"9100e858-8eae-11ef-0000-000000000001","payload":[{payload}]}}],"has_more":false}}"#
    );
    server.mock(move |when, then| {
        when.method(GET).path("/v14/notifications");
        then.status(200)
            .header("content-type", "application/json")
            .body(body.clone());
    });
}

/// Spec shape (backend-spec §12.2): `data` is a **bare base64
/// string** — the one event whose `data` is not an object.
fn welcome_event(data_b64: &str) -> String {
    format!(
        r#"{{"type":"conversation.mls-welcome",
             "qualified_conversation":{{"id":"{ROOM_UUID}","domain":"{DOMAIN}"}},
             "qualified_from":{{"id":"bob-uuid","domain":"{DOMAIN}"}},
             "data":"{data_b64}",
             "time":"2026-08-14T10:00:00.000Z"}}"#
    )
}

/// The external-join surface — a welcome-absorbing client must NOT
/// touch it (0 hits: the Welcome itself carries everything).
fn mount_groupinfo_guard(server: &MockServer) -> Mock<'_> {
    server.mock(|when, then| {
        when.method(GET)
            .path(format!("/v14/conversations/{DOMAIN}/{ROOM_UUID}/groupinfo"));
        then.status(200)
            .header("content-type", "message/mls")
            .body(common::mls::group_info_tls(b"grp-wl", PEER_IDENTITY));
    })
}

fn mount_message_send(server: &MockServer) -> Mock<'_> {
    server.mock(|when, then| {
        when.method(POST).path("/v14/mls/messages");
        then.status(201)
            .header("content-type", "application/json")
            .body(r#"{"events":[],"time":"2026-08-14T10:00:01.000Z"}"#);
    })
}

/// Login with the key-package capture registered FIRST (httpmock:
/// first-registered mock wins) so `register_device`'s upload flows
/// through the capturing matcher.
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

/// A real Welcome addressed to this client, base64 for the event.
fn welcome_b64_for(server: &MockServer) -> String {
    let kp_tls = captured_key_package_for(server);
    let welcome = common::mls::welcome_over_key_package(b"grp-wl", PEER_IDENTITY, &kp_tls);
    base64::engine::general_purpose::STANDARD.encode(welcome)
}

#[tokio::test]
async fn welcome_joins_the_group_before_any_message() {
    let server = MockServer::start();
    let (client, _store) = logged_in_client(&server).await;
    mount_rooms_with_group(&server);
    let gi = mount_groupinfo_guard(&server);
    let send = mount_message_send(&server);
    mount_notifications_page(&server, &[welcome_event(&welcome_b64_for(&server))]);

    client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync_once absorbs the welcome");

    let room_id = format!("{ROOM_UUID}@{DOMAIN}");
    let room = client.room(&room_id).await.expect("room cached after sync");
    let timeline = room.timeline().expect("timeline");
    timeline
        .send_text("post-welcome".into())
        .await
        .expect("welcomed device sends without join_room — the group is local");
    assert_eq!(send.calls(), 1, "the message went out");
    assert_eq!(
        gi.calls(),
        0,
        "absorbing a Welcome must not fall back to the external-join surface"
    );
}

#[tokio::test]
async fn welcome_redelivery_is_skipped_not_an_error() {
    let server = MockServer::start();
    let (client, _store) = logged_in_client(&server).await;
    mount_rooms_with_group(&server);
    let gi = mount_groupinfo_guard(&server);
    let send = mount_message_send(&server);
    // The broker redelivers: the SAME Welcome twice in one page. The
    // second absorb finds the group already in storage → skip.
    let ev = welcome_event(&welcome_b64_for(&server));
    mount_notifications_page(&server, &[ev.clone(), ev]);

    client
        .sync_once(SyncSettings::default())
        .await
        .expect("redelivered welcome must not fail the drain");

    let room_id = format!("{ROOM_UUID}@{DOMAIN}");
    let room = client.room(&room_id).await.expect("room cached after sync");
    let timeline = room.timeline().expect("timeline");
    timeline
        .send_text("after-redelivery".into())
        .await
        .expect("group joined exactly once and usable");
    assert_eq!(send.calls(), 1);
    assert_eq!(gi.calls(), 0, "no recovery churn from a redelivery");
}

#[tokio::test]
async fn malformed_welcome_warns_and_the_drain_continues() {
    let server = MockServer::start();
    let (client, _store) = logged_in_client(&server).await;
    mount_rooms_with_group(&server);
    let gi = mount_groupinfo_guard(&server);
    let send = mount_message_send(&server);
    // Garbage first — the drain must warn + continue, and the valid
    // Welcome later in the SAME page must still be absorbed.
    mount_notifications_page(
        &server,
        &[
            welcome_event("!!!not-base64!!!"),
            welcome_event(&welcome_b64_for(&server)),
        ],
    );

    client
        .sync_once(SyncSettings::default())
        .await
        .expect("a malformed welcome must not fail the drain");

    let room_id = format!("{ROOM_UUID}@{DOMAIN}");
    let room = client.room(&room_id).await.expect("room cached after sync");
    let timeline = room.timeline().expect("timeline");
    timeline
        .send_text("after-garbage".into())
        .await
        .expect("the valid welcome after the garbage one was absorbed");
    assert_eq!(send.calls(), 1);
    assert_eq!(
        gi.calls(),
        0,
        "garbage wire data must not be able to trigger join/recovery churn"
    );
}
