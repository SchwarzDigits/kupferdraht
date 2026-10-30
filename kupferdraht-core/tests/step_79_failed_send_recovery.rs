// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 79 — failed-send recovery (TDD red).
//!
//! A `Failed` row (Step 25a) is the wedged local echo of a send that
//! never reached the backend. This step makes it recoverable without a
//! background scheduler: `Timeline::resend(message_id)` re-drives ONE
//! row from its persisted state under the SAME `message_id` (the
//! matrix-rust-sdk `SendHandle::unwedge` shape — the transaction id
//! survives the retry), `Room::failed_sends()` enumerates the rows
//! oldest-first, and `Timeline::resend_failed()` re-drives them in that
//! order, stopping at the first hard error so a stuck message is never
//! overtaken (matrix's strict-order rule). Rows behind the stop stay
//! `Failed` — the pass is re-runnable after the next reconnect.
//!
//! Fixtures mirror `step_89_ephemeral_messages.rs`: the client is
//! welcomed into a fixture peer's group, so sends run real MLS
//! encryption; the peer decrypts what was captured off
//! `POST /v14/mls/messages` and a minimal protobuf reader pins the
//! `GenericMessage` fields (`wire::proto` is crate-private, ADR-0009).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use httpmock::Method::{GET, POST};
use httpmock::{Mock, MockServer, prelude::HttpMockRequest};
use kupferdraht::storage::Store;
use kupferdraht::timeline::{
    DeliveryStatus, Mention, MessagesOptions, TaskDraft, TimelineDiff, TimelineItem,
    TimelineItemKind, TimelineListener,
};
use kupferdraht::{ClientBuilder, SyncSettings};

mod common;

const ROOM_UUID: &str = "7b2e4d10-3c5a-4f8e-9a01-6d4c2b8e0f79";
const DOMAIN: &str = "example.test";
/// b64("grp-resend")
const GROUP_ID_B64: &str = "Z3JwLXJlc2VuZA==";
const GROUP_ID: &[u8] = b"grp-resend";
const BOB: &str = "bob-uuid";
const BOB_IDENTITY: &[u8] = b"bob-uuid:bobdev01@example.test";
const PARENT_ID: &str = "parent-from-bob-0001";

fn room_id() -> String {
    format!("{ROOM_UUID}@{DOMAIN}")
}

fn bob_qualified() -> String {
    format!("{BOB}@{DOMAIN}")
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

fn put_len_delimited(field: u32, bytes: &[u8], out: &mut Vec<u8>) {
    put_varint((u64::from(field) << 3) | 2, out);
    put_varint(bytes.len() as u64, out);
    out.extend_from_slice(bytes);
}

/// `GenericMessage { message_id = 1, content.text = 2 { content = 1 } }`
/// — the peer's parent message a reply quotes.
fn text_generic_message(message_id: &str, body: &str) -> Vec<u8> {
    let mut text = Vec::new();
    put_len_delimited(1, body.as_bytes(), &mut text);
    let mut m = Vec::new();
    put_len_delimited(1, message_id.as_bytes(), &mut m);
    put_len_delimited(2, &text, &mut m);
    m
}

// ---- minimal protobuf reader (what the peer decrypted) ----

#[derive(Debug, Clone)]
enum Field {
    Varint(u64),
    Bytes(Vec<u8>),
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

/// Decode one message level into `(field number, value)` pairs.
/// Fixed-width values (float coordinates) are skipped — no test here
/// reads a `Location`.
fn decode_fields(buf: &[u8]) -> Vec<(u32, Field)> {
    let mut out = Vec::new();
    let mut pos = 0;
    while pos < buf.len() {
        let tag = read_varint(buf, &mut pos);
        let number = u32::try_from(tag >> 3).expect("field number");
        match tag & 7 {
            0 => out.push((number, Field::Varint(read_varint(buf, &mut pos)))),
            1 => pos += 8,
            2 => {
                let len = usize::try_from(read_varint(buf, &mut pos)).expect("len");
                out.push((number, Field::Bytes(buf[pos..pos + len].to_vec())));
                pos += len;
            }
            5 => pos += 4,
            other => panic!("unsupported wire type {other}"),
        }
    }
    out
}

fn bytes_fields(fields: &[(u32, Field)], number: u32) -> Vec<&[u8]> {
    fields
        .iter()
        .filter_map(|(n, f)| match f {
            Field::Bytes(b) if *n == number => Some(b.as_slice()),
            _ => None,
        })
        .collect()
}

fn bytes_field(fields: &[(u32, Field)], number: u32) -> Option<&[u8]> {
    bytes_fields(fields, number).into_iter().next()
}

fn str_field(fields: &[(u32, Field)], number: u32) -> Option<String> {
    bytes_field(fields, number).map(|b| String::from_utf8(b.to_vec()).expect("utf-8"))
}

fn varint_field(fields: &[(u32, Field)], number: u32) -> Option<u64> {
    fields.iter().find_map(|(n, f)| match f {
        Field::Varint(v) if *n == number => Some(*v),
        _ => None,
    })
}

/// `(message_id, top-level fields)` of a decrypted `GenericMessage`.
fn decode_generic_message(plaintext: &[u8]) -> (String, Vec<(u32, Field)>) {
    let fields = decode_fields(plaintext);
    let id = str_field(&fields, 1).expect("GenericMessage.message_id");
    (id, fields)
}

// ---- mocks (Step-69 shapes) ----

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

/// Matches the FIRST `POST /v14/mls/messages` per host only — the
/// "one message goes through, the next one hits the outage" sequence.
/// httpmock picks the earliest-created matching mock, so this mock is
/// mounted before the 500 catch-all.
static FIRST_UPLOAD_SEEN: Mutex<Vec<String>> = Mutex::new(Vec::new());
fn first_mls_upload_only(req: &HttpMockRequest) -> bool {
    if !req.uri_str().contains("/v14/mls/messages") {
        return false;
    }
    let host = host_of(req);
    let mut seen = FIRST_UPLOAD_SEEN.lock().expect("capture lock");
    if seen.contains(&host) {
        return false;
    }
    seen.push(host);
    true
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
                     "name":"Resend",
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
             "time":"2026-09-24T10:00:00.000Z"}}"#
    )
}

fn message_event(data_b64: &str) -> String {
    format!(
        r#"{{"type":"conversation.mls-message-add",
             "qualified_conversation":{{"id":"{ROOM_UUID}","domain":"{DOMAIN}"}},
             "qualified_from":{{"id":"{BOB}","domain":"{DOMAIN}"}},
             "data":"{data_b64}",
             "time":"2026-09-24T10:00:01.000Z"}}"#
    )
}

fn mount_mls_upload_ok(server: &MockServer) -> Mock<'_> {
    server.mock(|when, then| {
        when.method(POST)
            .path("/v14/mls/messages")
            .is_true(capture_mls_upload);
        then.status(201)
            .header("content-type", "application/json")
            .body(r#"{"events":[],"time":"2026-09-24T10:00:02.000Z"}"#);
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
            .body(r#"{"key":"asset-key-79","token":"tok-79","domain":"example.test"}"#);
    })
}

/// Records every `TimelineDiff` — the resend contract is visible as an
/// `UpdateAt` carrying the flipped row.
#[derive(Default)]
struct DiffRecorder {
    diffs: Mutex<Vec<TimelineDiff>>,
}

impl TimelineListener for DiffRecorder {
    fn on_update(&self, diff: TimelineDiff) {
        self.diffs.lock().expect("recorder lock").push(diff);
    }
}

impl DiffRecorder {
    fn updates_for(&self, message_id: &str) -> Vec<TimelineItem> {
        self.diffs
            .lock()
            .expect("recorder lock")
            .iter()
            .filter_map(|d| match d {
                TimelineDiff::UpdateAt { item, .. } if item.item_id == message_id => {
                    Some(item.clone())
                }
                _ => None,
            })
            .collect()
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

struct Fixture {
    client: Arc<kupferdraht::Client>,
    store: Arc<dyn Store>,
    room: Arc<kupferdraht::rooms::Room>,
    timeline: Arc<kupferdraht::Timeline>,
    peer: common::mls::PeerGroup,
    recorder: Arc<DiffRecorder>,
    _subscription: kupferdraht::Subscription,
}

/// httpmock pools its servers (LEARNINGS §5.13): with `--test-threads=1`
/// — and on CI — a later test gets an earlier test's `host:port` back, so
/// the host-keyed capture statics would hand it foreign ciphertexts.
/// A pooled server serves one test at a time, so purging this host's
/// entries at fixture setup is exact.
fn reset_capture_state_for(server: &MockServer) {
    let host = server.address().to_string();
    MLS_UPLOADS
        .lock()
        .expect("capture lock")
        .retain(|(h, _)| *h != host);
    FIRST_UPLOAD_SEEN
        .lock()
        .expect("capture lock")
        .retain(|h| *h != host);
}

/// Login, subscribe the diff recorder, and welcome the client into the
/// fixture peer's group. `extra_messages` are peer-encrypted
/// `GenericMessage` plaintexts delivered after the welcome. The peer is
/// returned so the tests can decrypt what the client uploads.
async fn welcomed_client(server: &MockServer, extra_messages: &[Vec<u8>]) -> Fixture {
    reset_capture_state_for(server);
    let (client, store) = logged_in_client(server).await;
    let recorder = Arc::new(DiffRecorder::default());
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
    let timeline = room.timeline().expect("timeline");
    Fixture {
        client,
        store,
        room,
        timeline,
        peer,
        recorder,
        _subscription: subscription,
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

fn text_body(kind: &TimelineItemKind) -> Option<&str> {
    match kind {
        TimelineItemKind::Text { body, .. } => Some(body),
        _ => None,
    }
}

/// A send whose upload the mounted failing mock rejects: returns the
/// `message_id` of the persisted `Failed` row.
async fn failed_text_send(fx: &Fixture, body: &str) -> String {
    let err = fx
        .timeline
        .send_text(body.into())
        .await
        .expect_err("the failing mock must surface");
    assert!(
        matches!(err, kupferdraht::KupferdrahtError::BackendRejected(_)),
        "expected BackendRejected, got {err:?}"
    );
    let row = items(&fx.client)
        .await
        .into_iter()
        .find(|i| text_body(&i.kind) == Some(body))
        .expect("the failed row persists (Step 25a)");
    assert_eq!(row.delivery_status, DeliveryStatus::Failed);
    // Distinct `sent_at_ms` per row keeps oldest-first deterministic.
    tokio::time::sleep(Duration::from_millis(3)).await;
    row.item_id
}

fn task_draft(title: &str) -> TaskDraft {
    TaskDraft {
        title: title.into(),
        description: None,
        assignee: None,
        due_at_ms: None,
        priority: None,
    }
}

// ---- Timeline::resend ----

#[tokio::test]
async fn resend_flips_a_failed_text_row_to_submitted_with_one_more_upload() {
    let server = MockServer::start();
    let mut fx = welcomed_client(&server, &[]).await;
    let mut failing = mount_mls_upload_failing(&server);
    let id = failed_text_send(&fx, "stuck in the outage").await;
    assert_eq!(failing.calls(), 1);
    let before = item(&fx.client, &id).await;

    failing.delete();
    let ok = mount_mls_upload_ok(&server);
    let handle = fx
        .timeline
        .resend(id.clone())
        .await
        .expect("resend goes through once the backend is back");
    assert_eq!(handle.message_id, id, "the row keeps its message_id");
    assert!(
        handle.sent_at_ms >= before.sent_at_ms,
        "sent_at_ms is re-stamped to the resend instant"
    );
    assert_eq!(ok.calls(), 1, "exactly one more upload");

    let after = item(&fx.client, &id).await;
    assert_eq!(after.delivery_status, DeliveryStatus::Submitted);
    assert_eq!(text_body(&after.kind), Some("stuck in the outage"));
    assert_eq!(after.sent_at_ms, handle.sent_at_ms);
    let updates = fx.recorder.updates_for(&id);
    assert!(
        updates
            .iter()
            .any(|i| i.delivery_status == DeliveryStatus::Submitted),
        "subscribers see the flip as an UpdateAt, got {updates:?}"
    );
    assert!(
        fx.room
            .failed_sends()
            .await
            .expect("failed_sends")
            .is_empty(),
        "nothing left to resend"
    );

    // On the wire: the same message_id, the same body.
    let uploads = captured_mls_uploads_for(&server);
    assert_eq!(uploads.len(), 1);
    let (wire_id, fields) =
        decode_generic_message(&fx.peer.decrypt_application_message(&uploads[0]));
    assert_eq!(wire_id, id);
    let text = decode_fields(bytes_field(&fields, 2).expect("Text content variant"));
    assert_eq!(str_field(&text, 1).as_deref(), Some("stuck in the outage"));
}

#[tokio::test]
async fn resend_keeps_the_quote_and_the_mentions_on_the_wire() {
    let server = MockServer::start();
    let parent = text_generic_message(PARENT_ID, "hello from bob");
    let mut fx = welcomed_client(&server, &[parent]).await;
    let mut failing = mount_mls_upload_failing(&server);

    let reply_err = fx
        .timeline
        .send_reply(PARENT_ID.into(), "quoting you".into())
        .await
        .expect_err("reply upload rejected");
    assert!(matches!(
        reply_err,
        kupferdraht::KupferdrahtError::BackendRejected(_)
    ));
    tokio::time::sleep(Duration::from_millis(3)).await;
    let mention = Mention {
        user_id: bob_qualified(),
        start: 0,
        length: 4,
    };
    let mention_err = fx
        .timeline
        .send_text_with_mentions("@bob ping".into(), vec![mention.clone()])
        .await
        .expect_err("mention upload rejected");
    assert!(matches!(
        mention_err,
        kupferdraht::KupferdrahtError::BackendRejected(_)
    ));
    assert_eq!(failing.calls(), 2);

    let failed = fx.room.failed_sends().await.expect("failed_sends");
    assert_eq!(failed.len(), 2, "both rows are failed sends: {failed:?}");
    let reply_id = failed[0].item_id.clone();
    let mention_id = failed[1].item_id.clone();

    failing.delete();
    let ok = mount_mls_upload_ok(&server);
    fx.timeline
        .resend(reply_id.clone())
        .await
        .expect("resend reply");
    fx.timeline
        .resend(mention_id.clone())
        .await
        .expect("resend mention");
    assert_eq!(ok.calls(), 2);

    let uploads = captured_mls_uploads_for(&server);
    assert_eq!(uploads.len(), 2);
    let (id0, fields0) = decode_generic_message(&fx.peer.decrypt_application_message(&uploads[0]));
    assert_eq!(id0, reply_id);
    let text0 = decode_fields(bytes_field(&fields0, 2).expect("Text"));
    assert_eq!(str_field(&text0, 1).as_deref(), Some("quoting you"));
    let quote = decode_fields(bytes_field(&text0, 5).expect("Text.quote survives the resend"));
    assert_eq!(str_field(&quote, 1).as_deref(), Some(PARENT_ID));
    assert!(
        bytes_field(&quote, 2).is_some(),
        "the quote sha256 is re-derived from the persisted parent"
    );

    let (id1, fields1) = decode_generic_message(&fx.peer.decrypt_application_message(&uploads[1]));
    assert_eq!(id1, mention_id);
    let text1 = decode_fields(bytes_field(&fields1, 2).expect("Text"));
    let mentions = bytes_fields(&text1, 4);
    assert_eq!(mentions.len(), 1, "Text.mentions survives the resend");
    let m = decode_fields(mentions[0]);
    assert_eq!(varint_field(&m, 1), Some(0));
    assert_eq!(varint_field(&m, 2), Some(4));
    let qid = decode_fields(bytes_field(&m, 4).expect("qualified_user_id"));
    assert_eq!(str_field(&qid, 1).as_deref(), Some(BOB));
    assert_eq!(str_field(&qid, 2).as_deref(), Some(DOMAIN));

    // Locally the reply / mention metadata is untouched.
    let reply_row = item(&fx.client, &reply_id).await;
    assert!(matches!(
        &reply_row.kind,
        TimelineItemKind::Text { reply_to: Some(p), .. } if p == PARENT_ID
    ));
    let mention_row = item(&fx.client, &mention_id).await;
    assert!(matches!(
        &mention_row.kind,
        TimelineItemKind::Text { mentions, .. } if mentions == &vec![mention.clone()]
    ));
    assert_eq!(reply_row.delivery_status, DeliveryStatus::Submitted);
    assert_eq!(mention_row.delivery_status, DeliveryStatus::Submitted);
}

#[tokio::test]
async fn resend_rejects_rows_that_are_not_failed_sends() {
    let server = MockServer::start();
    let fx = welcomed_client(&server, &[]).await;
    let ok = mount_mls_upload_ok(&server);
    let sent = fx
        .timeline
        .send_text("delivered fine".into())
        .await
        .expect("send");
    assert_eq!(ok.calls(), 1);

    let err = fx
        .timeline
        .resend(sent.message_id.clone())
        .await
        .expect_err("a submitted row is not a failed send");
    assert!(
        matches!(&err, kupferdraht::KupferdrahtError::InvalidInput(msg) if msg.contains(&sent.message_id)),
        "expected InvalidInput naming the row, got {err:?}"
    );
    let err = fx
        .timeline
        .resend("no-such-row".into())
        .await
        .expect_err("an unknown id is InvalidInput");
    assert!(
        matches!(err, kupferdraht::KupferdrahtError::InvalidInput(_)),
        "{err:?}"
    );
    assert_eq!(ok.calls(), 1, "no upload for a refused resend");
    assert_eq!(
        item(&fx.client, &sent.message_id).await.delivery_status,
        DeliveryStatus::Submitted
    );
}

#[tokio::test]
async fn a_failed_resend_leaves_the_row_failed_for_the_next_attempt() {
    let server = MockServer::start();
    let fx = welcomed_client(&server, &[]).await;
    let mut failing = mount_mls_upload_failing(&server);
    let id = failed_text_send(&fx, "still no network").await;

    let err = fx
        .timeline
        .resend(id.clone())
        .await
        .expect_err("the outage persists");
    assert!(
        matches!(err, kupferdraht::KupferdrahtError::BackendRejected(_)),
        "{err:?}"
    );
    assert_eq!(failing.calls(), 2, "the resend did try once");
    let row = item(&fx.client, &id).await;
    assert_eq!(row.delivery_status, DeliveryStatus::Failed);
    assert_eq!(text_body(&row.kind), Some("still no network"));
    assert!(
        fx.recorder
            .updates_for(&id)
            .iter()
            .all(|i| i.delivery_status == DeliveryStatus::Failed),
        "no UpdateAt claims a delivery that did not happen"
    );

    failing.delete();
    let ok = mount_mls_upload_ok(&server);
    fx.timeline
        .resend(id.clone())
        .await
        .expect("third time lucky");
    assert_eq!(ok.calls(), 1);
    assert_eq!(
        item(&fx.client, &id).await.delivery_status,
        DeliveryStatus::Submitted
    );
}

#[tokio::test]
async fn resend_refuses_a_failed_task_snapshot_and_names_the_recovery() {
    let server = MockServer::start();
    let fx = welcomed_client(&server, &[]).await;
    let mut failing = mount_mls_upload_failing(&server);
    let err = fx
        .timeline
        .send_task(task_draft("ship it"), Some(false))
        .await
        .expect_err("task upload rejected");
    assert!(
        matches!(err, kupferdraht::KupferdrahtError::BackendRejected(_)),
        "{err:?}"
    );
    let failed = fx.room.failed_sends().await.expect("failed_sends");
    assert_eq!(failed.len(), 1, "the failed task row is enumerable");
    assert!(matches!(failed[0].kind, TimelineItemKind::Task(_)));
    let id = failed[0].item_id.clone();

    failing.delete();
    let ok = mount_mls_upload_ok(&server);
    let err = fx
        .timeline
        .resend(id.clone())
        .await
        .expect_err("a stale full-state snapshot must not be replayed");
    assert!(
        matches!(&err, kupferdraht::KupferdrahtError::InvalidInput(msg) if msg.contains("update_task")),
        "the refusal names the recovery, got {err:?}"
    );
    assert_eq!(ok.calls(), 0);
    assert_eq!(
        item(&fx.client, &id).await.delivery_status,
        DeliveryStatus::Failed,
        "the row stays honest"
    );
}

#[tokio::test]
async fn resend_of_a_failed_ephemeral_text_re_wraps_the_ttl_and_starts_the_timer() {
    let server = MockServer::start();
    let mut fx = welcomed_client(&server, &[]).await;
    let mut failing = mount_mls_upload_failing(&server);
    let err = fx
        .timeline
        .send_ephemeral_text("otp 4711".into(), Duration::from_secs(1))
        .await
        .expect_err("ephemeral upload rejected");
    assert!(
        matches!(err, kupferdraht::KupferdrahtError::BackendRejected(_)),
        "{err:?}"
    );
    let failed = fx.room.failed_sends().await.expect("failed_sends");
    assert_eq!(failed.len(), 1);
    let id = failed[0].item_id.clone();
    assert_eq!(
        failed[0].expires_at_ms, None,
        "no timer before delivery (Step 89)"
    );

    failing.delete();
    let ok = mount_mls_upload_ok(&server);
    let handle = fx.timeline.resend(id.clone()).await.expect("resend");
    assert_eq!(ok.calls(), 1);
    let row = item(&fx.client, &id).await;
    assert_eq!(row.delivery_status, DeliveryStatus::Submitted);
    assert_eq!(
        row.expires_at_ms,
        Some(handle.sent_at_ms + 1_000),
        "the timer starts on the delivered resend with the ORIGINAL ttl"
    );

    // On the wire: the Ephemeral wrapper (field 18) with the original
    // TTL — never a plain Text (field 2) that would outlive its purpose.
    let uploads = captured_mls_uploads_for(&server);
    let (wire_id, fields) =
        decode_generic_message(&fx.peer.decrypt_application_message(&uploads[0]));
    assert_eq!(wire_id, id);
    assert!(bytes_field(&fields, 2).is_none(), "no plain Text arm");
    let eph = decode_fields(bytes_field(&fields, 18).expect("Ephemeral arm"));
    assert_eq!(varint_field(&eph, 1), Some(1_000), "expire_after_millis");
    let text = decode_fields(bytes_field(&eph, 2).expect("Ephemeral.text"));
    assert_eq!(str_field(&text, 1).as_deref(), Some("otp 4711"));

    tokio::time::sleep(Duration::from_millis(1_500)).await;
    let expired = item(&fx.client, &id).await;
    assert!(expired.redacted, "the re-armed timer tombstones the row");
    assert_eq!(expired.expires_at_ms, Some(handle.sent_at_ms + 1_000));
}

#[tokio::test]
async fn resend_of_a_failed_asset_reuses_the_uploaded_blob() {
    let server = MockServer::start();
    let mut fx = welcomed_client(&server, &[]).await;
    let assets = mount_asset_upload(&server);
    let mut failing = mount_mls_upload_failing(&server);
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("notes.txt");
    std::fs::write(&path, b"resend me").expect("write file");

    let err = fx
        .timeline
        .send_file(&path, "text/plain")
        .await
        .expect_err("MLS upload rejected after the blob went up");
    assert!(
        matches!(err, kupferdraht::KupferdrahtError::BackendRejected(_)),
        "{err:?}"
    );
    assert_eq!(assets.calls(), 1, "the blob was uploaded");
    let failed = fx.room.failed_sends().await.expect("failed_sends");
    assert_eq!(failed.len(), 1);
    assert!(matches!(failed[0].kind, TimelineItemKind::Asset(_)));
    let id = failed[0].item_id.clone();

    failing.delete();
    let ok = mount_mls_upload_ok(&server);
    let handle = fx.timeline.resend(id.clone()).await.expect("resend asset");
    assert_eq!(handle.message_id, id);
    assert_eq!(ok.calls(), 1);
    assert_eq!(
        assets.calls(),
        1,
        "no second blob upload — the reference is persisted"
    );
    let row = item(&fx.client, &id).await;
    assert_eq!(row.delivery_status, DeliveryStatus::Submitted);
    match &row.kind {
        TimelineItemKind::Asset(a) => assert_eq!(a.asset_id, "asset-key-79"),
        other => panic!("expected an asset row, got {other:?}"),
    }

    // On the wire: the same reference every Wire client needs to fetch it.
    let uploads = captured_mls_uploads_for(&server);
    let (wire_id, fields) =
        decode_generic_message(&fx.peer.decrypt_application_message(&uploads[0]));
    assert_eq!(wire_id, id);
    let asset = decode_fields(bytes_field(&fields, 11).expect("Asset arm"));
    let original = decode_fields(bytes_field(&asset, 1).expect("Asset.original"));
    assert_eq!(str_field(&original, 1).as_deref(), Some("text/plain"));
    assert_eq!(varint_field(&original, 2), Some(9), "plaintext size");
    assert_eq!(str_field(&original, 3).as_deref(), Some("notes.txt"));
    let remote = decode_fields(bytes_field(&asset, 4).expect("Asset.uploaded"));
    assert_eq!(str_field(&remote, 3).as_deref(), Some("asset-key-79"));
    assert_eq!(str_field(&remote, 5).as_deref(), Some("tok-79"));
    assert_eq!(str_field(&remote, 7).as_deref(), Some("example.test"));
    assert_eq!(
        bytes_field(&remote, 1).map(<[u8]>::len),
        Some(32),
        "the raw AES key travels again"
    );
    assert_eq!(bytes_field(&remote, 2).map(<[u8]>::len), Some(32), "sha256");
}

// ---- Room::failed_sends ----

#[tokio::test]
async fn failed_sends_lists_failed_rows_only_oldest_first() {
    let server = MockServer::start();
    let inbound = text_generic_message("bob-says-hi", "hi");
    let fx = welcomed_client(&server, &[inbound]).await;
    let mut ok = mount_mls_upload_ok(&server);
    fx.timeline
        .send_text("went through".into())
        .await
        .expect("send");
    ok.delete();
    let _failing = mount_mls_upload_failing(&server);
    let first = failed_text_send(&fx, "first failure").await;
    let second = failed_text_send(&fx, "second failure").await;

    let failed = fx.room.failed_sends().await.expect("failed_sends");
    let ids: Vec<&str> = failed.iter().map(|i| i.item_id.as_str()).collect();
    assert_eq!(ids, vec![first.as_str(), second.as_str()], "oldest first");
    assert!(
        failed
            .iter()
            .all(|i| i.delivery_status == DeliveryStatus::Failed)
    );
    assert_eq!(
        items(&fx.client).await.len(),
        4,
        "received + submitted rows stay out of the failed view"
    );
}

// ---- Timeline::resend_failed ----

#[tokio::test]
async fn resend_failed_goes_oldest_first_and_stops_at_the_first_hard_error() {
    let server = MockServer::start();
    let fx = welcomed_client(&server, &[]).await;
    let mut failing = mount_mls_upload_failing(&server);
    let f1 = failed_text_send(&fx, "one").await;
    let task_err = fx
        .timeline
        .send_task(task_draft("snapshot"), Some(false))
        .await
        .expect_err("task upload rejected");
    assert!(matches!(
        task_err,
        kupferdraht::KupferdrahtError::BackendRejected(_)
    ));
    tokio::time::sleep(Duration::from_millis(3)).await;
    let f2 = failed_text_send(&fx, "two").await;
    let f3 = failed_text_send(&fx, "three").await;
    let task_id = fx.room.failed_sends().await.expect("failed_sends")[1]
        .item_id
        .clone();
    failing.delete();

    // The first upload succeeds, then the outage is back (creation
    // order = httpmock's matching order).
    let mut first_ok = server.mock(|when, then| {
        when.method(POST)
            .path("/v14/mls/messages")
            .is_true(first_mls_upload_only);
        then.status(201)
            .header("content-type", "application/json")
            .body(r#"{"events":[],"time":"2026-09-24T10:00:03.000Z"}"#);
    });
    let mut then_failing = mount_mls_upload_failing(&server);

    let summary = fx
        .timeline
        .resend_failed()
        .await
        .expect("a pass never errors as a whole");
    let resent: Vec<&str> = summary
        .resent
        .iter()
        .map(|h| h.message_id.as_str())
        .collect();
    assert_eq!(resent, vec![f1.as_str()], "oldest first, one went through");
    assert_eq!(
        summary.skipped,
        vec![task_id.clone()],
        "snapshot rows are skipped, not attempted"
    );
    assert_eq!(
        summary.stopped_at.as_ref().map(|(id, _)| id.as_str()),
        Some(f2.as_str()),
        "the pass stops at the first hard error"
    );
    assert_eq!(summary.remaining, 1, "f3 was not attempted");
    assert_eq!(first_ok.calls(), 1);
    assert_eq!(then_failing.calls(), 1, "f3 was never uploaded");

    assert_eq!(
        item(&fx.client, &f1).await.delivery_status,
        DeliveryStatus::Submitted
    );
    for id in [&task_id, &f2, &f3] {
        assert_eq!(
            item(&fx.client, id).await.delivery_status,
            DeliveryStatus::Failed
        );
    }
    let left: Vec<String> = fx
        .room
        .failed_sends()
        .await
        .expect("failed_sends")
        .into_iter()
        .map(|i| i.item_id)
        .collect();
    assert_eq!(left, vec![task_id.clone(), f2.clone(), f3.clone()]);

    // Re-runnable: once the backend is back the rest goes through in order.
    first_ok.delete();
    then_failing.delete();
    let ok = mount_mls_upload_ok(&server);
    let summary = fx.timeline.resend_failed().await.expect("second pass");
    let resent: Vec<&str> = summary
        .resent
        .iter()
        .map(|h| h.message_id.as_str())
        .collect();
    assert_eq!(resent, vec![f2.as_str(), f3.as_str()]);
    assert_eq!(summary.skipped, vec![task_id.clone()]);
    assert!(summary.stopped_at.is_none());
    assert_eq!(summary.remaining, 0);
    assert_eq!(ok.calls(), 2);
    let left: Vec<String> = fx
        .room
        .failed_sends()
        .await
        .expect("failed_sends")
        .into_iter()
        .map(|i| i.item_id)
        .collect();
    assert_eq!(left, vec![task_id], "only the snapshot row remains");
    assert!(
        fx.store
            .load_timeline_events(&room_id())
            .await
            .expect("rows")
            .iter()
            .filter(|r| r.delivery_status == "failed")
            .count()
            == 1
    );
}

// ---- the reconnect contract is written down ----

fn workspace_file(rel: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

#[test]
fn the_reconnect_contract_is_written_down() {
    let library = workspace_file("docs/library.md");
    assert!(
        library.contains("### Reconnect contract"),
        "docs/library.md must carry the reconnect contract as its own section"
    );
    for needle in ["resend_failed", "failed_sends", "no network observer"] {
        assert!(library.contains(needle), "docs/library.md lacks {needle:?}");
    }
    let bindings = workspace_file("docs/bindings.md");
    for needle in ["resend_failed", "failed_sends", "Reconnect contract"] {
        assert!(
            bindings.contains(needle),
            "docs/bindings.md lacks {needle:?}"
        );
    }
}
