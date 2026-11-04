// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 130a — idempotency keys (TDD red).
//!
//! `Timeline::send_keyed(key, content)` sends once per key: the same key
//! answers the stored outcome — the handle of a sent message or reaction,
//! a resend under the same id for a send that failed — instead of reaching
//! the backend a second time (matrix-rust-sdk: `Room::send(content)
//! .with_transaction_id(txn_id)`). A message's key is written with its
//! `Sending` row, a reaction's with its reaction set; keys live 28 days.
//!
//! Fixtures mirror `step_79_failed_send_recovery.rs`: the client is
//! welcomed into a fixture peer's group, so sends run real MLS
//! encryption, and every ciphertext that reaches `POST /v14/mls/messages`
//! is captured — the upload count is what a key promises.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use httpmock::Method::{GET, POST};
use httpmock::{Mock, MockServer, prelude::HttpMockRequest};
use kupferdraht::storage::{SEND_KEY_RETENTION_MS, SendKeyRecord, Store, TimelineEventRecord};
use kupferdraht::timeline::{DeliveryStatus, MessagesOptions, TimelineItem, TimelineItemKind};
use kupferdraht::{ClientBuilder, KupferdrahtError, MessageHandle, OutgoingContent, SyncSettings};

mod common;

const ROOM_UUID: &str = "5d1c8a42-7e3b-4c09-b6f1-2a9e4d7c0b13";
const DOMAIN: &str = "example.test";
/// b64("grp-keyed")
const GROUP_ID_B64: &str = "Z3JwLWtleWVk";
const GROUP_ID: &[u8] = b"grp-keyed";
const BOB: &str = "bob-uuid";
const BOB_IDENTITY: &[u8] = b"bob-uuid:bobdev01@example.test";
const PARENT_ID: &str = "parent-from-bob-130a";

fn room_id() -> String {
    format!("{ROOM_UUID}@{DOMAIN}")
}

// ---- minimal protobuf writer / reader (Step 79 precedent) ----

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

fn put_len_delimited(field: u32, bytes: &[u8], out: &mut Vec<u8>) {
    put_varint((u64::from(field) << 3) | 2, out);
    put_varint(bytes.len() as u64, out);
    out.extend_from_slice(bytes);
}

/// `GenericMessage { message_id = 1, content.text = 2 { content = 1 } }`
/// — the peer's message a reply quotes and a reaction targets.
fn text_generic_message(message_id: &str, body: &str) -> Vec<u8> {
    let mut text = Vec::new();
    put_len_delimited(1, body.as_bytes(), &mut text);
    let mut m = Vec::new();
    put_len_delimited(1, message_id.as_bytes(), &mut m);
    put_len_delimited(2, &text, &mut m);
    m
}

fn read_varint(buf: &[u8], pos: &mut usize) -> u64 {
    let mut value = 0u64;
    let mut shift = 0;
    loop {
        let byte = buf[*pos];
        *pos += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return value;
        }
        shift += 7;
    }
}

/// `GenericMessage.message_id` (field 1) of a decrypted upload.
fn wire_message_id(plaintext: &[u8]) -> String {
    let mut pos = 0;
    while pos < plaintext.len() {
        let tag = read_varint(plaintext, &mut pos);
        match tag & 7 {
            0 => {
                read_varint(plaintext, &mut pos);
            }
            1 => pos += 8,
            2 => {
                let len = usize::try_from(read_varint(plaintext, &mut pos)).expect("len");
                if tag >> 3 == 1 {
                    return String::from_utf8(plaintext[pos..pos + len].to_vec()).expect("utf8");
                }
                pos += len;
            }
            5 => pos += 4,
            other => panic!("unsupported wire type {other}"),
        }
    }
    panic!("GenericMessage without message_id")
}

// ---- mocks (Step-79 shapes) ----

fn host_of(req: &HttpMockRequest) -> String {
    req.headers()
        .get("host")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
        .unwrap_or_default()
}

static UPLOADED_KPS: Mutex<Vec<(String, Vec<String>)>> = Mutex::new(Vec::new());
fn capture_key_package_upload(req: &HttpMockRequest) -> bool {
    let host = host_of(req);
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

/// Every ciphertext that reached the success mock, per host, in order.
static MLS_UPLOADS: Mutex<Vec<(String, Vec<u8>)>> = Mutex::new(Vec::new());
fn capture_mls_upload(req: &HttpMockRequest) -> bool {
    if !req.uri_str().contains("/v14/mls/messages") {
        return false;
    }
    MLS_UPLOADS
        .lock()
        .expect("capture lock")
        .push((host_of(req), req.body_ref().to_vec()));
    true
}

fn captured_mls_uploads_for(server: &MockServer) -> Vec<Vec<u8>> {
    let host = server.address().to_string();
    MLS_UPLOADS
        .lock()
        .expect("capture lock")
        .iter()
        .filter(|(h, _)| *h == host)
        .map(|(_, body)| body.clone())
        .collect()
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
                     "name":"Keyed",
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
        r#"{{"notifications":[{{"id":"9100e858-8eae-11ef-0000-0000130acafe","payload":[{payload}]}}],"has_more":false}}"#
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
             "time":"2026-11-04T10:00:00.000Z"}}"#
    )
}

fn message_event(data_b64: &str) -> String {
    format!(
        r#"{{"type":"conversation.mls-message-add",
             "qualified_conversation":{{"id":"{ROOM_UUID}","domain":"{DOMAIN}"}},
             "qualified_from":{{"id":"{BOB}","domain":"{DOMAIN}"}},
             "data":"{data_b64}",
             "time":"2026-11-04T10:00:01.000Z"}}"#
    )
}

fn mount_mls_upload_ok(server: &MockServer) -> Mock<'_> {
    server.mock(|when, then| {
        when.method(POST)
            .path("/v14/mls/messages")
            .is_true(capture_mls_upload);
        then.status(201)
            .header("content-type", "application/json")
            .body(r#"{"events":[],"time":"2026-11-04T10:00:02.000Z"}"#);
    })
}

/// The success mock, answering only after `delay` — a send stays in
/// flight that long.
fn mount_mls_upload_slow(server: &MockServer, delay: Duration) -> Mock<'_> {
    server.mock(move |when, then| {
        when.method(POST)
            .path("/v14/mls/messages")
            .is_true(capture_mls_upload);
        then.status(201)
            .delay(delay)
            .header("content-type", "application/json")
            .body(r#"{"events":[],"time":"2026-11-04T10:00:02.000Z"}"#);
    })
}

fn mount_mls_upload_failing(server: &MockServer) -> Mock<'_> {
    server.mock(|when, then| {
        when.method(POST).path("/v14/mls/messages");
        then.status(500)
            .header("content-type", "application/json")
            .body(r#"{"label":"server-error","message":"boom"}"#);
    })
}

fn mount_asset_upload(server: &MockServer) -> Mock<'_> {
    server.mock(|when, then| {
        when.method(POST).path("/v14/assets");
        then.status(201)
            .header("content-type", "application/json")
            .body(r#"{"key":"asset-key-130a","token":"tok-130a","domain":"example.test"}"#);
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

struct Fixture {
    client: Arc<kupferdraht::Client>,
    store: Arc<dyn Store>,
    timeline: Arc<kupferdraht::Timeline>,
    peer: common::mls::PeerGroup,
}

/// httpmock pools its servers (LEARNINGS §5.13): purge this host's
/// captured uploads at fixture setup.
fn reset_capture_state_for(server: &MockServer) {
    let host = server.address().to_string();
    MLS_UPLOADS
        .lock()
        .expect("capture lock")
        .retain(|(h, _)| *h != host);
}

/// Login and welcome the client into the fixture peer's group, with Bob's
/// text message `PARENT_ID` delivered after the welcome.
async fn welcomed_client(server: &MockServer) -> Fixture {
    reset_capture_state_for(server);
    let (client, store) = logged_in_client(server).await;
    mount_room(server);
    let mut peer = common::mls::PeerGroup::new(GROUP_ID, BOB_IDENTITY);
    let b64 = |bytes: Vec<u8>| base64::engine::general_purpose::STANDARD.encode(bytes);
    let welcome = b64(peer.welcome_over_key_package(&captured_key_package_for(server)));
    let parent = text_generic_message(PARENT_ID, "hello from bob");
    let events = vec![
        welcome_event(&welcome),
        message_event(&b64(peer.application_message(&parent))),
    ];
    mount_notifications_page(server, &events);
    client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync");
    let room = client.room(&room_id()).await.expect("room cached");
    let timeline = room.timeline().expect("timeline");
    Fixture {
        client,
        store,
        timeline,
        peer,
    }
}

async fn items(client: &kupferdraht::Client) -> Vec<TimelineItem> {
    client
        .messages(&room_id(), &MessagesOptions::all())
        .await
        .expect("messages")
}

async fn item(client: &kupferdraht::Client, message_id: &str) -> TimelineItem {
    items(client)
        .await
        .into_iter()
        .find(|i| i.item_id == message_id)
        .unwrap_or_else(|| panic!("row {message_id} missing"))
}

/// The rows whose text body is `body`.
async fn text_rows(client: &kupferdraht::Client, body: &str) -> Vec<TimelineItem> {
    items(client)
        .await
        .into_iter()
        .filter(|i| matches!(&i.kind, TimelineItemKind::Text { body: b, .. } if b == body))
        .collect()
}

fn text(body: &str) -> OutgoingContent {
    OutgoingContent::Text {
        body: body.into(),
        mentions: Vec::new(),
    }
}

fn thumbs_up() -> OutgoingContent {
    OutgoingContent::Reaction {
        item_id: PARENT_ID.into(),
        emoji: "👍".into(),
    }
}

async fn reacted(client: &kupferdraht::Client) -> bool {
    item(client, PARENT_ID)
        .await
        .reactions
        .iter()
        .any(|group| group.emoji == "👍")
}

async fn send(fx: &Fixture, key: &str, content: OutgoingContent) -> MessageHandle {
    fx.timeline
        .send_keyed(key.into(), content)
        .await
        .unwrap_or_else(|e| panic!("keyed send {key}: {e:?}"))
}

fn assert_invalid_input(result: Result<MessageHandle, KupferdrahtError>, what: &str) {
    match result {
        Err(KupferdrahtError::InvalidInput(_)) => {}
        other => panic!("{what}: expected InvalidInput, got {other:?}"),
    }
}

// ---- a message key ----

#[tokio::test]
async fn a_keyed_text_uploads_once_and_the_same_key_answers_the_same_handle() {
    let server = MockServer::start();
    let fx = welcomed_client(&server).await;
    let ok = mount_mls_upload_ok(&server);

    let first = send(&fx, "key-text-1", text("hello once")).await;
    let again = send(&fx, "key-text-1", text("hello once")).await;

    assert_eq!(again, first, "the same key answers the stored handle");
    assert_eq!(ok.calls(), 1, "the backend sees one upload");
    let rows = text_rows(&fx.client, "hello once").await;
    assert_eq!(rows.len(), 1, "one row");
    assert_eq!(rows[0].item_id, first.message_id);
    assert_eq!(rows[0].delivery_status, DeliveryStatus::Submitted);
    let key = fx
        .store
        .send_key("key-text-1")
        .await
        .expect("send_key")
        .expect("the key and its outcome are stored");
    assert_eq!(key.room_id, room_id());
    assert_eq!(key.kind, "message");
    assert_eq!(key.message_id, first.message_id);
    assert_eq!(key.sent_at_ms, first.sent_at_ms);
    assert!(
        key.created_at_ms >= first.sent_at_ms - 1_000,
        "dated at the send"
    );
}

#[tokio::test]
async fn two_keys_send_twice() {
    let server = MockServer::start();
    let fx = welcomed_client(&server).await;
    let ok = mount_mls_upload_ok(&server);

    let a = send(&fx, "key-a", text("twice on purpose")).await;
    let b = send(&fx, "key-b", text("twice on purpose")).await;

    assert_ne!(a.message_id, b.message_id);
    assert_eq!(ok.calls(), 2, "a new key re-runs");
    assert_eq!(text_rows(&fx.client, "twice on purpose").await.len(), 2);
}

#[tokio::test]
async fn a_known_key_with_a_failed_row_resends_under_the_same_id() {
    let server = MockServer::start();
    let mut fx = welcomed_client(&server).await;
    let mut failing = mount_mls_upload_failing(&server);

    let err = fx
        .timeline
        .send_keyed("key-retry".into(), text("through the outage"))
        .await
        .expect_err("the failing mock must surface");
    assert!(
        matches!(err, KupferdrahtError::BackendRejected(_)),
        "expected BackendRejected, got {err:?}"
    );
    let failed = text_rows(&fx.client, "through the outage").await;
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].delivery_status, DeliveryStatus::Failed);
    let id = failed[0].item_id.clone();

    failing.delete();
    let ok = mount_mls_upload_ok(&server);
    let handle = send(&fx, "key-retry", text("through the outage")).await;

    assert_eq!(handle.message_id, id, "the retry keeps the message id");
    assert_eq!(ok.calls(), 1, "exactly one upload reached the backend");
    let rows = text_rows(&fx.client, "through the outage").await;
    assert_eq!(rows.len(), 1, "no second row");
    assert_eq!(rows[0].delivery_status, DeliveryStatus::Submitted);
    let uploads = captured_mls_uploads_for(&server);
    assert_eq!(uploads.len(), 1);
    assert_eq!(
        wire_message_id(&fx.peer.decrypt_application_message(&uploads[0])),
        id,
        "receivers see the id of the failed row"
    );
}

#[tokio::test]
async fn a_key_whose_send_is_in_flight_is_invalid_input() {
    let server = MockServer::start();
    let fx = welcomed_client(&server).await;
    let slow = mount_mls_upload_slow(&server, Duration::from_millis(600));

    let timeline = fx.timeline.clone();
    let first = tokio::spawn(async move {
        timeline
            .send_keyed("key-busy".into(), text("still uploading"))
            .await
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    let second = fx
        .timeline
        .send_keyed("key-busy".into(), text("still uploading"))
        .await;

    assert_invalid_input(second, "a second call while the first is in flight");
    first
        .await
        .expect("join")
        .expect("the first call goes through");
    assert_eq!(slow.calls(), 1);
}

/// The leader-change case (ADR-0026): the old leader wrote the `Sending`
/// row and its key, then died mid-upload. The new leader holds the row
/// and no in-flight entry; the re-issued call re-drives it under its id.
#[tokio::test]
async fn an_orphaned_sending_row_is_re_driven_under_its_id() {
    let server = MockServer::start();
    let mut fx = welcomed_client(&server).await;
    let ok = mount_mls_upload_ok(&server);
    let at = kupferdraht::now_ms();
    let mut row = outgoing_row("orphan-130a", at);
    row.body = "left behind by the old leader".into();
    row.delivery_status = "sending".into();
    let written = fx
        .store
        .insert_timeline_event_keyed(
            &row,
            &[],
            None,
            &message_key("key-orphan", "orphan-130a", at),
        )
        .await
        .expect("keyed write");
    assert!(written);

    let handle = send(&fx, "key-orphan", text("left behind by the old leader")).await;

    assert_eq!(handle.message_id, "orphan-130a", "the row keeps its id");
    assert_eq!(ok.calls(), 1, "one upload");
    assert_eq!(
        item(&fx.client, "orphan-130a").await.delivery_status,
        DeliveryStatus::Submitted
    );
    assert_eq!(
        text_rows(&fx.client, "left behind by the old leader")
            .await
            .len(),
        1,
        "no second row"
    );
    let uploads = captured_mls_uploads_for(&server);
    assert_eq!(
        wire_message_id(&fx.peer.decrypt_application_message(&uploads[0])),
        "orphan-130a"
    );
}

#[tokio::test]
async fn a_key_of_another_room_is_invalid_input() {
    let server = MockServer::start();
    let fx = welcomed_client(&server).await;
    let ok = mount_mls_upload_ok(&server);
    let at = kupferdraht::now_ms();
    let mut key = message_key("key-elsewhere", "row-elsewhere", at);
    key.room_id = format!("0f0e0d0c-0b0a-4908-8706-050403020100@{DOMAIN}");
    fx.store
        .insert_timeline_event_keyed(&outgoing_row("row-elsewhere", at), &[], None, &key)
        .await
        .expect("keyed write");

    assert_invalid_input(
        fx.timeline
            .send_keyed("key-elsewhere".into(), text("wrong room"))
            .await,
        "a key that belongs to another room",
    );
    assert_eq!(ok.calls(), 0);
}

// ---- a reaction key ----

#[tokio::test]
async fn a_keyed_reaction_is_applied_once_and_a_new_key_toggles_it_off() {
    let server = MockServer::start();
    let fx = welcomed_client(&server).await;
    let ok = mount_mls_upload_ok(&server);

    let first = send(&fx, "key-react-1", thumbs_up()).await;
    let again = send(&fx, "key-react-1", thumbs_up()).await;

    assert_eq!(again, first, "the same key answers the stored handle");
    assert_eq!(ok.calls(), 1, "a known key never toggles again");
    assert!(reacted(&fx.client).await, "the reaction stays on");
    let key = fx
        .store
        .send_key("key-react-1")
        .await
        .expect("send_key")
        .expect("the reaction's key is stored");
    assert_eq!(key.kind, "reaction");
    assert_eq!(key.message_id, first.message_id);

    send(&fx, "key-react-2", thumbs_up()).await;
    assert_eq!(ok.calls(), 2, "a new key toggles");
    assert!(!reacted(&fx.client).await, "the second toggle removes it");
}

// ---- every keyed kind ----

#[tokio::test]
async fn reply_knock_location_and_file_each_answer_their_key_once() {
    let server = MockServer::start();
    let fx = welcomed_client(&server).await;
    let assets = mount_asset_upload(&server);
    let ok = mount_mls_upload_ok(&server);
    let contents = [
        (
            "key-reply",
            OutgoingContent::Reply {
                parent_id: PARENT_ID.into(),
                body: "a reply".into(),
            },
        ),
        ("key-knock", OutgoingContent::Knock { hot_knock: false }),
        (
            "key-location",
            OutgoingContent::Location {
                latitude: 52.52,
                longitude: 13.405,
                name: Some("Berlin".into()),
                zoom: Some(12),
            },
        ),
        (
            "key-file",
            OutgoingContent::File {
                bytes: b"keyed file".to_vec(),
                filename: "notes.txt".into(),
                mime_type: "text/plain".into(),
            },
        ),
    ];

    for (key, content) in contents {
        let first = send(&fx, key, content.clone()).await;
        let again = send(&fx, key, content).await;
        assert_eq!(
            again, first,
            "{key}: the same key answers the stored handle"
        );
        assert_eq!(
            item(&fx.client, &first.message_id).await.delivery_status,
            DeliveryStatus::Submitted,
            "{key}: one submitted row"
        );
    }
    assert_eq!(ok.calls(), 4, "one upload per key");
    assert_eq!(assets.calls(), 1, "the file's blob went up once");
}

// ---- refused keys ----

#[tokio::test]
async fn a_key_of_another_kind_is_invalid_input() {
    let server = MockServer::start();
    let fx = welcomed_client(&server).await;
    let ok = mount_mls_upload_ok(&server);

    send(&fx, "key-shared", text("first use")).await;
    let reused = fx
        .timeline
        .send_keyed("key-shared".into(), thumbs_up())
        .await;

    assert_invalid_input(reused, "a message key reused for a reaction");
    assert_eq!(ok.calls(), 1, "nothing more reached the backend");
    assert!(!reacted(&fx.client).await, "no reaction was sent");
}

#[tokio::test]
async fn an_empty_or_oversized_key_is_invalid_input() {
    let server = MockServer::start();
    let fx = welcomed_client(&server).await;
    let ok = mount_mls_upload_ok(&server);

    assert_invalid_input(
        fx.timeline.send_keyed(String::new(), text("no key")).await,
        "an empty key",
    );
    assert_invalid_input(
        fx.timeline
            .send_keyed("k".repeat(129), text("long key"))
            .await,
        "a 129-character key",
    );
    assert_eq!(ok.calls(), 0, "a refused key sends nothing");

    send(&fx, &"k".repeat(128), text("longest key")).await;
    assert_eq!(ok.calls(), 1, "128 characters are a key");
}

// ---- the 28-day sweep ----

fn outgoing_row(message_id: &str, at_ms: i64) -> TimelineEventRecord {
    TimelineEventRecord {
        message_id: message_id.into(),
        room_id: room_id(),
        sender_user_id: format!("user-uuid@{DOMAIN}"),
        sender_device_id: "dev130a".into(),
        kind: "text".into(),
        body: format!("row {message_id}"),
        sent_at_ms: at_ms,
        delivery_status: "submitted".into(),
        ..TimelineEventRecord::default()
    }
}

fn message_key(key: &str, message_id: &str, at_ms: i64) -> SendKeyRecord {
    SendKeyRecord {
        idempotency_key: key.into(),
        room_id: room_id(),
        kind: "message".into(),
        message_id: message_id.into(),
        sent_at_ms: at_ms,
        created_at_ms: at_ms,
    }
}

async fn keyed_write(store: &Arc<dyn Store>, key: &str, message_id: &str, at_ms: i64) {
    let written = store
        .insert_timeline_event_keyed(
            &outgoing_row(message_id, at_ms),
            &[],
            None,
            &message_key(key, message_id, at_ms),
        )
        .await
        .expect("keyed write");
    assert!(written, "{message_id} is a new row");
}

#[tokio::test]
async fn the_store_forgets_a_key_28_days_after_the_next_keyed_write() {
    let server = MockServer::start();
    let fx = welcomed_client(&server).await;
    let t0: i64 = 1_800_000_000_000;

    keyed_write(&fx.store, "k-old", "m-old", t0).await;
    assert_eq!(
        fx.store.send_key("k-old").await.expect("send_key"),
        Some(message_key("k-old", "m-old", t0)),
        "a keyed write stores its key"
    );

    keyed_write(&fx.store, "k-edge", "m-edge", t0 + SEND_KEY_RETENTION_MS).await;
    assert!(
        fx.store
            .send_key("k-old")
            .await
            .expect("send_key")
            .is_some(),
        "a key exactly 28 days old still answers"
    );

    keyed_write(&fx.store, "k-new", "m-new", t0 + SEND_KEY_RETENTION_MS + 1).await;
    assert_eq!(
        fx.store.send_key("k-old").await.expect("send_key"),
        None,
        "a key older than 28 days is swept by the next keyed write"
    );
    assert!(
        fx.store
            .send_key("k-edge")
            .await
            .expect("send_key")
            .is_some()
    );
    assert!(
        fx.store
            .send_key("k-new")
            .await
            .expect("send_key")
            .is_some()
    );
}

#[test]
fn the_retention_is_28_days() {
    assert_eq!(SEND_KEY_RETENTION_MS, 28 * 24 * 60 * 60 * 1000);
}
