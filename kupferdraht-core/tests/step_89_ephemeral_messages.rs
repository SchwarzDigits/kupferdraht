// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 89 — ephemeral messages: send + receive toleration (TDD red).
//!
//! Send side: `Timeline::send_ephemeral_text(body, ttl)` wraps the
//! `Text` in Wire's `Ephemeral` content variant (`GenericMessage`
//! field 18, `expire_after_millis` + inner `Text`), persists the row
//! with `expires_at_ms = sent_at_ms + ttl`, and self-redacts the own
//! copy once the TTL elapses. Receive side: an inbound
//! `Ephemeral(Text)` surfaces as an ordinary `Text` item / `Message`
//! decorated with `expires_at_ms`, fires `on_message`, and
//! self-redacts locally after the TTL (V1a: local tombstone only —
//! no delete-for-everyone is sent back, see the step doc). V1 timer
//! semantics: the countdown starts at dispatch (headless clients
//! have no viewport), i.e. never later than an official client's
//! view-triggered start. Non-`Text` inner arms decode to the
//! tolerated-unknown path without losing the rest of the frame.
//!
//! Fixtures mirror `step_75_task_list_marker.rs`: the client is
//! welcomed into a fixture peer's group; inbound payloads are
//! hand-emitted protobuf (`wire::proto` is crate-private, ADR-0009);
//! outbound sends are counted as `POST /v14/mls/messages` hits — the
//! field-18 payload shape is pinned in `wire/proto.rs` unit tests
//! (MLS ciphertext is opaque here).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use httpmock::Method::{GET, POST};
use httpmock::{Mock, MockServer, prelude::HttpMockRequest};
use kupferdraht::storage::{Store, TimelineEventRecord};
use kupferdraht::timeline::{
    DeliveryStatus, MessagesOptions, TimelineDiff, TimelineItemKind, TimelineListener,
};
use kupferdraht::{ClientBuilder, EventHandler, Message, MessageBody, SyncSettings};

mod common;

const ROOM_UUID: &str = "5c0f1a2e-7d3b-4e6a-9f10-2b8c4d6e8f01";
const DOMAIN: &str = "example.test";
/// b64("grp-ephemeral")
const GROUP_ID_B64: &str = "Z3JwLWVwaGVtZXJhbA==";
const GROUP_ID: &[u8] = b"grp-ephemeral";
const BOB: &str = "bob-uuid";
const BOB_IDENTITY: &[u8] = b"bob-uuid:bobdev01@example.test";

fn room_id() -> String {
    format!("{ROOM_UUID}@{DOMAIN}")
}

// ---- minimal protobuf writer (proto2 wire format, 72b precedent) ----

fn put_varint(mut v: u64, out: &mut Vec<u8>) {
    loop {
        let byte = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(byte);
            break;
        }
        out.push(byte | 0x80);
    }
}

fn put_tag(field: u32, wire_type: u8, out: &mut Vec<u8>) {
    put_varint((u64::from(field) << 3) | u64::from(wire_type), out);
}

fn put_len_delimited(field: u32, bytes: &[u8], out: &mut Vec<u8>) {
    put_tag(field, 2, out);
    put_varint(bytes.len() as u64, out);
    out.extend_from_slice(bytes);
}

fn put_uint(field: u32, value: u64, out: &mut Vec<u8>) {
    put_tag(field, 0, out);
    put_varint(value, out);
}

/// `Text { content = 1 }`.
fn text_payload(body: &str) -> Vec<u8> {
    let mut text = Vec::new();
    put_len_delimited(1, body.as_bytes(), &mut text);
    text
}

/// `GenericMessage { message_id = 1, content.text = 2 }` — a plain
/// text, the preservation baseline.
fn text_generic_message(message_id: &str, body: &str) -> Vec<u8> {
    let mut m = Vec::new();
    put_len_delimited(1, message_id.as_bytes(), &mut m);
    put_len_delimited(2, &text_payload(body), &mut m);
    m
}

/// `GenericMessage { message_id = 1, content.ephemeral = 18 }` with
/// `Ephemeral { expire_after_millis = 1, content.text = 2 }` — the
/// shape an official Wire client emits for a timed text.
fn ephemeral_text_generic_message(message_id: &str, body: &str, ttl_ms: u64) -> Vec<u8> {
    let mut eph = Vec::new();
    put_uint(1, ttl_ms, &mut eph);
    put_len_delimited(2, &text_payload(body), &mut eph);
    let mut m = Vec::new();
    put_len_delimited(1, message_id.as_bytes(), &mut m);
    put_len_delimited(18, &eph, &mut m);
    m
}

/// `Ephemeral { expire_after_millis = 1, content.knock = 4 }` with
/// `Knock { hot_knock = 1 }` — an inner arm Kupferdraht does not declare
/// in V1; must decode to the tolerated-unknown path.
fn ephemeral_knock_generic_message(message_id: &str, ttl_ms: u64) -> Vec<u8> {
    let mut knock = Vec::new();
    put_uint(1, 0, &mut knock);
    let mut eph = Vec::new();
    put_uint(1, ttl_ms, &mut eph);
    put_len_delimited(4, &knock, &mut eph);
    let mut m = Vec::new();
    put_len_delimited(1, message_id.as_bytes(), &mut m);
    put_len_delimited(18, &eph, &mut m);
    m
}

// ---- mocks (Step-69 shapes) ----

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
    // Most recent per host — httpmock pools servers (LEARNINGS §5.13).
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

fn mount_room(server: &MockServer) {
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
    server.mock(move |when, then| {
        when.method(POST).path("/v14/conversations/list");
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(
                r#"{{"found":[{{
                     "qualified_id":{{"id":"{ROOM_UUID}","domain":"{DOMAIN}"}},
                     "type":0,"protocol":"mls","epoch":1,"group_id":"{GROUP_ID_B64}",
                     "name":"Ephemeral",
                     "members":{{
                       "self":{{"qualified_id":{{"id":"user-uuid","domain":"{DOMAIN}"}},
                               "conversation_role":"wire_member"}},
                       "others":[
                         {{"qualified_id":{{"id":"{BOB}","domain":"{DOMAIN}"}},
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

fn mount_notifications_page(server: &MockServer, events: &[String]) {
    let payload = events.join(",");
    let body = format!(
        r#"{{"notifications":[{{"id":"9100e858-8eae-11ef-0000-0000075cafe1","payload":[{payload}]}}],"has_more":false}}"#
    );
    server.mock(move |when, then| {
        when.method(GET).path("/v14/notifications");
        then.status(200)
            .header("content-type", "application/json")
            .body(body.clone());
    });
}

fn welcome_event(data_b64: &str) -> String {
    format!(
        r#"{{"type":"conversation.mls-welcome",
             "qualified_conversation":{{"id":"{ROOM_UUID}","domain":"{DOMAIN}"}},
             "qualified_from":{{"id":"{BOB}","domain":"{DOMAIN}"}},
             "data":"{data_b64}",
             "time":"2026-09-15T10:00:00.000Z"}}"#
    )
}

fn message_event(data_b64: &str) -> String {
    format!(
        r#"{{"type":"conversation.mls-message-add",
             "qualified_conversation":{{"id":"{ROOM_UUID}","domain":"{DOMAIN}"}},
             "qualified_from":{{"id":"{BOB}","domain":"{DOMAIN}"}},
             "data":"{data_b64}",
             "time":"2026-09-15T10:00:01.000Z"}}"#
    )
}

fn mount_message_send(server: &MockServer) -> Mock<'_> {
    server.mock(|when, then| {
        when.method(POST).path("/v14/mls/messages");
        then.status(201)
            .header("content-type", "application/json")
            .body(r#"{"events":[],"time":"2026-09-15T10:00:02.000Z"}"#);
    })
}

/// Records every consumer-facing signal: `on_message`, `on_sync_error`,
/// and every `TimelineDiff` — the ephemeral contract is visible on all
/// three (TTL metadata on the message and the Append, the tombstone
/// on an UpdateAt, silence on the tolerated arm).
#[derive(Default)]
struct Recorder {
    messages: Mutex<Vec<Message>>,
    errors: Mutex<Vec<String>>,
    diffs: Mutex<Vec<TimelineDiff>>,
}

impl EventHandler for Recorder {
    fn on_message(&self, message: Message) {
        self.messages.lock().expect("recorder lock").push(message);
    }
    fn on_room_invited(&self, _room_id: String) {}
    fn on_sync_error(&self, description: String) {
        self.errors.lock().expect("recorder lock").push(description);
    }
}

impl TimelineListener for Recorder {
    fn on_update(&self, diff: TimelineDiff) {
        self.diffs.lock().expect("recorder lock").push(diff);
    }
}

impl Recorder {
    fn messages(&self) -> Vec<Message> {
        self.messages.lock().expect("recorder lock").clone()
    }
    fn errors(&self) -> Vec<String> {
        self.errors.lock().expect("recorder lock").clone()
    }
    fn diffs(&self) -> Vec<TimelineDiff> {
        self.diffs.lock().expect("recorder lock").clone()
    }
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

/// Login, register the recorder on both observer surfaces, and
/// welcome the client into the fixture group. `extra_messages` are
/// peer-encrypted `GenericMessage` plaintexts delivered after the
/// welcome, in slice order. The returned `Subscription` keeps the
/// timeline listener alive for the test's lifetime.
async fn welcomed_client(
    server: &MockServer,
    extra_messages: &[Vec<u8>],
) -> (
    Arc<kupferdraht::Client>,
    Arc<dyn Store>,
    Arc<kupferdraht::rooms::Room>,
    Arc<Recorder>,
    kupferdraht::Subscription,
) {
    let (client, store) = logged_in_client(server).await;
    let recorder = Arc::new(Recorder::default());
    client.add_event_handler(recorder.clone());
    let subscription = client.add_timeline_listener(recorder.clone());
    mount_room(server);
    let mut peer = common::mls::PeerGroup::new(GROUP_ID, BOB_IDENTITY);
    let b64 = |bytes: Vec<u8>| base64::engine::general_purpose::STANDARD.encode(bytes);
    let welcome = b64(peer.welcome_over_key_package(&captured_key_package_for(server)));
    let mut events = vec![welcome_event(&welcome)];
    for plaintext in extra_messages {
        events.push(message_event(&b64(peer.application_message(plaintext))));
    }
    mount_notifications_page(server, &events);
    client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync");
    let room = client.room(&room_id()).await.expect("room cached");
    (client, store, room, recorder, subscription)
}

async fn items(client: &kupferdraht::Client) -> Vec<kupferdraht::timeline::TimelineItem> {
    client
        .messages(&room_id(), &MessagesOptions::all())
        .await
        .expect("messages")
}

fn text_body(kind: &TimelineItemKind) -> Option<&str> {
    match kind {
        TimelineItemKind::Text { body, .. } => Some(body),
        _ => None,
    }
}

// ---- send side ----

#[tokio::test]
async fn send_ephemeral_text_uploads_once_and_persists_the_expiry() {
    let server = MockServer::start();
    let (client, _store, room, recorder, _sub) = welcomed_client(&server, &[]).await;
    let send = mount_message_send(&server);

    let handle = room
        .timeline()
        .expect("timeline")
        .send_ephemeral_text(
            "Your one-time code: 123456".into(),
            Duration::from_secs(300),
        )
        .await
        .expect("send ephemeral text");
    assert_eq!(send.calls(), 1, "exactly one MLS upload");

    let all = items(&client).await;
    let item = all
        .iter()
        .find(|i| i.item_id == handle.message_id)
        .expect("own row persisted");
    assert_eq!(
        text_body(&item.kind),
        Some("Your one-time code: 123456"),
        "the own copy stays a readable Text item until the TTL elapses"
    );
    assert_eq!(
        item.expires_at_ms,
        Some(handle.sent_at_ms + 300_000),
        "the sender's timer starts at send: expires_at = sent_at + ttl"
    );
    assert!(!item.redacted, "not redacted before the TTL");

    // Subscribers must not need a reload to render a countdown. Since
    // Step 97a the Append arrives before the upload (as `Sending`, no
    // timer yet — only a delivered send gets one); the UpdateAt that
    // settles the send carries the expiry.
    let diffs = recorder.diffs();
    let appended = diffs
        .iter()
        .find_map(|d| match d {
            TimelineDiff::Append { item, .. } if item.item_id == handle.message_id => Some(item),
            _ => None,
        })
        .expect("Append diff for the own send");
    assert_eq!(appended.delivery_status, DeliveryStatus::Sending);
    let settled = diffs
        .iter()
        .rev()
        .find_map(|d| match d {
            TimelineDiff::UpdateAt { item, .. } if item.item_id == handle.message_id => Some(item),
            _ => None,
        })
        .expect("UpdateAt diff settling the own send");
    assert_eq!(settled.delivery_status, DeliveryStatus::Submitted);
    assert_eq!(settled.expires_at_ms, Some(handle.sent_at_ms + 300_000));
}

#[tokio::test]
async fn send_ephemeral_text_rejects_ttl_outside_the_wire_range() {
    // Official clients bound the timer to 1 s … 1 year; a zero TTL
    // would be a message that is deleted before it is read, and an
    // over-long one is not renderable. Both fail client-side before
    // anything is built or uploaded.
    let server = MockServer::start();
    let (_client, _store, room, _recorder, _sub) = welcomed_client(&server, &[]).await;
    let send = mount_message_send(&server);
    let timeline = room.timeline().expect("timeline");

    for ttl in [Duration::ZERO, Duration::from_secs(366 * 24 * 60 * 60)] {
        let err = timeline
            .send_ephemeral_text("x".into(), ttl)
            .await
            .expect_err("out-of-range ttl must be rejected");
        assert!(
            matches!(&err, kupferdraht::KupferdrahtError::InvalidInput(msg) if msg.contains("ttl")),
            "expected InvalidInput naming the ttl for {ttl:?}, got {err:?}"
        );
    }
    assert_eq!(send.calls(), 0, "nothing is uploaded for a rejected ttl");
}

#[tokio::test]
async fn sent_ephemeral_text_self_redacts_after_ttl() {
    // The sender's own copy expires on the local timer too (an
    // official sender obfuscates its copy at TTL; Kupferdraht tombstones
    // it — the same local-only redaction as the receive side).
    let server = MockServer::start();
    let (client, _store, room, recorder, _sub) = welcomed_client(&server, &[]).await;
    let _send = mount_message_send(&server);

    let handle = room
        .timeline()
        .expect("timeline")
        .send_ephemeral_text("gone soon".into(), Duration::from_secs(1))
        .await
        .expect("send ephemeral text");
    tokio::time::sleep(Duration::from_millis(1_700)).await;

    let all = items(&client).await;
    let item = all
        .iter()
        .find(|i| i.item_id == handle.message_id)
        .expect("row stays as a tombstone");
    assert!(item.redacted, "own ephemeral row is redacted after the TTL");
    assert!(matches!(item.kind, TimelineItemKind::Redacted));
    assert_eq!(
        item.expires_at_ms,
        Some(handle.sent_at_ms + 1_000),
        "the expiry stamp survives the tombstone (distinguishes expiry from a delete)"
    );
    let updated = recorder.diffs().into_iter().any(|d| {
        matches!(d, TimelineDiff::UpdateAt { item, .. }
            if item.item_id == handle.message_id && item.redacted)
    });
    assert!(
        updated,
        "the expiry emits an UpdateAt carrying the tombstone"
    );
}

fn mount_message_send_failing(server: &MockServer) -> Mock<'_> {
    server.mock(|when, then| {
        when.method(POST).path("/v14/mls/messages");
        then.status(500)
            .header("content-type", "application/json")
            .body(r#"{"label":"server-error","message":"boom"}"#);
    })
}

#[tokio::test]
async fn failed_ephemeral_send_keeps_the_body_without_a_timer() {
    // An official sender starts its own countdown only once the send
    // reached the backend. A failed upload therefore persists the
    // usual `Failed` row (Step 25a) WITHOUT an expiry stamp — the body
    // stays available for a later resend instead of silently
    // tombstoning a message nobody ever received.
    let server = MockServer::start();
    let (client, _store, room, _recorder, _sub) = welcomed_client(&server, &[]).await;
    let failing = mount_message_send_failing(&server);

    let err = room
        .timeline()
        .expect("timeline")
        .send_ephemeral_text("never left".into(), Duration::from_secs(1))
        .await
        .expect_err("a 500 must surface");
    assert!(
        matches!(err, kupferdraht::KupferdrahtError::BackendRejected(_)),
        "expected BackendRejected, got {err:?}"
    );
    assert_eq!(failing.calls(), 1);

    tokio::time::sleep(Duration::from_millis(1_500)).await;
    let all = items(&client).await;
    let failed = all
        .iter()
        .find(|i| text_body(&i.kind) == Some("never left"))
        .expect("the failed row keeps its body");
    assert_eq!(
        failed.delivery_status,
        kupferdraht::timeline::DeliveryStatus::Failed
    );
    assert_eq!(
        failed.expires_at_ms, None,
        "no timer for a send that never left"
    );
    assert!(!failed.redacted);
}

// ---- receive side ----

#[tokio::test]
async fn inbound_ephemeral_text_surfaces_with_expiry_and_notifies() {
    let server = MockServer::start();
    let (client, store, _room, recorder, _sub) = welcomed_client(
        &server,
        &[ephemeral_text_generic_message(
            "eph-m1",
            "Your one-time code: 654321",
            60_000,
        )],
    )
    .await;

    // on_message fires exactly once, as a Text body with TTL metadata.
    let messages = recorder.messages();
    assert_eq!(messages.len(), 1, "one on_message for the ephemeral text");
    let message = &messages[0];
    assert_eq!(message.message_id, "eph-m1");
    assert_eq!(
        message.body,
        MessageBody::Text("Your one-time code: 654321".into()),
        "an ephemeral text is still a Text body — the TTL is a decoration"
    );
    assert_eq!(
        message.expires_at_ms,
        Some(message.received_at_ms + 60_000),
        "V1 timer starts at dispatch: expires_at = received_at + ttl"
    );
    assert!(recorder.errors().is_empty(), "no sync error");

    // Persisted with the TTL, projected as Text + expires_at_ms.
    let rows = store
        .load_timeline_events(&room_id())
        .await
        .expect("load rows");
    let row = rows
        .iter()
        .find(|r| r.message_id == "eph-m1")
        .expect("row persisted");
    assert_eq!(row.kind, "text");
    assert_eq!(row.expires_at_ms, Some(row.sent_at_ms + 60_000));

    let all = items(&client).await;
    let item = all
        .iter()
        .find(|i| i.item_id == "eph-m1")
        .expect("timeline item");
    assert_eq!(text_body(&item.kind), Some("Your one-time code: 654321"));
    assert_eq!(item.expires_at_ms, Some(item.sent_at_ms + 60_000));
    assert!(!item.redacted);

    // The live Append carries the TTL metadata too.
    let appended = recorder
        .diffs()
        .into_iter()
        .find_map(|d| match d {
            TimelineDiff::Append { item, .. } if item.item_id == "eph-m1" => Some(item),
            _ => None,
        })
        .expect("Append diff");
    assert_eq!(appended.expires_at_ms, item.expires_at_ms);
}

#[tokio::test]
async fn inbound_ephemeral_unknown_arm_is_tolerated_without_losing_the_frame() {
    // An `Ephemeral(Knock)` (inner arm 4, undeclared in V1) must be
    // logged and discarded at the content layer — never an error, and
    // never at the envelope layer: the plain text behind it in the
    // same frame still lands.
    let server = MockServer::start();
    let (client, _store, _room, recorder, _sub) = welcomed_client(
        &server,
        &[
            ephemeral_knock_generic_message("eph-knock", 30_000),
            text_generic_message("plain-after", "still here"),
        ],
    )
    .await;

    assert!(
        recorder.errors().is_empty(),
        "a tolerated arm is not a sync error: {:?}",
        recorder.errors()
    );
    let all = items(&client).await;
    assert!(
        !all.iter().any(|i| i.item_id == "eph-knock"),
        "the undeclared inner arm produces no row"
    );
    let plain = all
        .iter()
        .find(|i| i.item_id == "plain-after")
        .expect("the following plain text in the same frame survives");
    assert_eq!(text_body(&plain.kind), Some("still here"));
    assert_eq!(plain.expires_at_ms, None);
    assert_eq!(recorder.messages().len(), 1, "only the plain text notifies");
}

#[tokio::test]
async fn inbound_ephemeral_text_self_redacts_after_ttl() {
    let server = MockServer::start();
    let (client, _store, _room, recorder, _sub) = welcomed_client(
        &server,
        &[ephemeral_text_generic_message("eph-short", "blink", 400)],
    )
    .await;

    let before = items(&client).await;
    let live = before
        .iter()
        .find(|i| i.item_id == "eph-short")
        .expect("row present right after dispatch");
    assert!(!live.redacted, "readable until the TTL elapses");

    tokio::time::sleep(Duration::from_millis(1_200)).await;

    let after = items(&client).await;
    let item = after
        .iter()
        .find(|i| i.item_id == "eph-short")
        .expect("the row stays as a tombstone (reply chains keep resolving)");
    assert!(item.redacted, "self-redacted after the TTL");
    assert!(matches!(item.kind, TimelineItemKind::Redacted));
    assert_eq!(
        item.expires_at_ms,
        Some(item.sent_at_ms + 400),
        "expiry stamp survives the tombstone"
    );
    let updated = recorder.diffs().into_iter().any(|d| {
        matches!(d, TimelineDiff::UpdateAt { item, .. }
            if item.item_id == "eph-short" && item.redacted)
    });
    assert!(
        updated,
        "the expiry emits an UpdateAt carrying the tombstone"
    );
    assert!(recorder.errors().is_empty());
}

#[tokio::test]
async fn overdue_ephemerals_are_swept_on_resume() {
    // Restart safety: a row whose TTL elapsed while the process was
    // down has no timer — the resume path (`load_rooms_from_store`)
    // must sweep it before any consumer can read the body. A row
    // still in the future stays readable.
    let server = MockServer::start();
    let (client, store) = logged_in_client(&server).await;
    let now = kupferdraht::now_ms();
    let seed = |id: &str, expires_at_ms: Option<i64>| TimelineEventRecord {
        message_id: id.into(),
        room_id: room_id(),
        sender_user_id: format!("{BOB}@{DOMAIN}"),
        kind: "text".into(),
        body: "secret".into(),
        sent_at_ms: now - 10_000,
        delivery_status: "received".into(),
        expires_at_ms,
        ..TimelineEventRecord::default()
    };
    store
        .insert_timeline_event(&seed("overdue", Some(now - 1_000)))
        .await
        .expect("seed overdue");
    store
        .insert_timeline_event(&seed("pending", Some(now + 3_600_000)))
        .await
        .expect("seed pending");
    store
        .insert_timeline_event(&seed("plain", None))
        .await
        .expect("seed plain");

    client
        .load_rooms_from_store()
        .await
        .expect("resume hydration");

    let all = items(&client).await;
    let find = |id: &str| all.iter().find(|i| i.item_id == id).expect(id).clone();
    let overdue = find("overdue");
    assert!(overdue.redacted, "an overdue row is swept on resume");
    assert!(matches!(overdue.kind, TimelineItemKind::Redacted));
    assert_eq!(overdue.expires_at_ms, Some(now - 1_000));
    let pending = find("pending");
    assert!(!pending.redacted, "a future expiry stays readable");
    assert_eq!(text_body(&pending.kind), Some("secret"));
    let plain = find("plain");
    assert!(!plain.redacted);
    assert_eq!(plain.expires_at_ms, None);
}

// ---- V1b: the receiver broadcasts the expiry as a delete-for-everyone ----

#[tokio::test]
async fn expired_inbound_ephemeral_broadcasts_a_delete_for_everyone() {
    // Official clients: the RECEIVER enforces expiry network-wide by
    // sending a `MessageDelete` for the expired message (the sender
    // only marks its own copy and waits for exactly this delete to
    // purge it). Kupferdraht as receiver mirrors that: after the local
    // tombstone, one MLS upload carries the delete.
    let server = MockServer::start();
    let (client, _store, _room, recorder, _sub) = welcomed_client(
        &server,
        &[ephemeral_text_generic_message(
            "eph-v1b",
            "gone for everyone",
            400,
        )],
    )
    .await;
    let send = mount_message_send(&server);
    assert_eq!(send.calls(), 0, "nothing is uploaded before the TTL");

    tokio::time::sleep(Duration::from_millis(1_200)).await;

    assert_eq!(
        send.calls(),
        1,
        "exactly one MLS upload: the MessageDelete broadcast for the expired inbound row"
    );
    let all = items(&client).await;
    let item = all
        .iter()
        .find(|i| i.item_id == "eph-v1b")
        .expect("tombstone stays");
    assert!(item.redacted);
    assert!(matches!(item.kind, TimelineItemKind::Redacted));
    assert!(
        recorder.errors().is_empty(),
        "the broadcast is not a sync error"
    );
}

#[tokio::test]
async fn expired_own_ephemeral_sends_no_delete() {
    // The SENDER side stays local-only (official senders wait for the
    // receiver's delete): the only upload is the original send.
    let server = MockServer::start();
    let (client, _store, room, _recorder, _sub) = welcomed_client(&server, &[]).await;
    let send = mount_message_send(&server);

    let handle = room
        .timeline()
        .expect("timeline")
        .send_ephemeral_text("own copy".into(), Duration::from_secs(1))
        .await
        .expect("send ephemeral text");
    assert_eq!(send.calls(), 1);
    tokio::time::sleep(Duration::from_millis(1_700)).await;

    assert_eq!(
        send.calls(),
        1,
        "an expired OWN ephemeral must not broadcast a delete"
    );
    let all = items(&client).await;
    let item = all
        .iter()
        .find(|i| i.item_id == handle.message_id)
        .expect("own tombstone");
    assert!(item.redacted);
}

#[tokio::test]
async fn failed_expiry_delete_keeps_the_local_tombstone() {
    // Best-effort broadcast (the Step-25b delivery-receipt precedent):
    // a rejected upload must neither resurrect the body nor surface
    // as a sync error — the local tombstone is the contract, the
    // network delete is the courtesy toward the sender.
    let server = MockServer::start();
    let (client, _store, _room, recorder, _sub) = welcomed_client(
        &server,
        &[ephemeral_text_generic_message(
            "eph-v1b-fail",
            "still gone locally",
            400,
        )],
    )
    .await;
    let failing = mount_message_send_failing(&server);

    tokio::time::sleep(Duration::from_millis(1_200)).await;

    assert_eq!(failing.calls(), 1, "the broadcast was attempted once");
    let all = items(&client).await;
    let item = all
        .iter()
        .find(|i| i.item_id == "eph-v1b-fail")
        .expect("row present");
    assert!(
        item.redacted,
        "local tombstone survives the failed broadcast"
    );
    assert!(matches!(item.kind, TimelineItemKind::Redacted));
    assert!(
        recorder.errors().is_empty(),
        "best-effort: no on_sync_error"
    );
}

// ---- preservation ----

#[tokio::test]
async fn plain_text_round_trip_carries_no_expiry() {
    // `send_text` and inbound plain `Text` are untouched by the step:
    // no TTL, no timer, no tombstone.
    let server = MockServer::start();
    let (client, _store, room, recorder, _sub) =
        welcomed_client(&server, &[text_generic_message("plain-in", "hello")]).await;
    let _send = mount_message_send(&server);

    let handle = room
        .timeline()
        .expect("timeline")
        .send_text("hello back".into())
        .await
        .expect("plain send");

    let all = items(&client).await;
    let inbound = all
        .iter()
        .find(|i| i.item_id == "plain-in")
        .expect("inbound plain text");
    assert_eq!(inbound.expires_at_ms, None);
    assert_eq!(text_body(&inbound.kind), Some("hello"));
    let outbound = all
        .iter()
        .find(|i| i.item_id == handle.message_id)
        .expect("outbound plain text");
    assert_eq!(outbound.expires_at_ms, None);
    assert_eq!(recorder.messages()[0].expires_at_ms, None);
}
