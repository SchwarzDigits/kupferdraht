// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 97a — persist before upload (review blocker B7). TDD red.
//!
//! A row-producing send used to upload first and write its row after:
//! a call cancelled during the upload (a Kotlin coroutine cancel drops
//! the Rust future; a crash) left no row and nothing to resend, and a
//! write failing after a successful upload was reported as a failure —
//! the retry went out twice under a new id. Now the row is written as
//! `DeliveryStatus::Sending` before the upload; a `Sending` row whose
//! send is no longer in flight reads as `Failed` and resends under the
//! same message id (Step 79); an upload that succeeded is `Ok`.
//!
//! The fixture is Step 79's (`step_79_failed_send_recovery.rs`): the
//! client is welcomed into a fixture peer's group, sends run real MLS
//! encryption, and the peer decrypts what was captured off
//! `POST /v14/mls/messages`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use httpmock::Method::{GET, POST};
use httpmock::{Mock, MockServer, prelude::HttpMockRequest};
use kupferdraht::keystore::{InMemoryKeyStore, SecureKeyStore};
use kupferdraht::storage::{StorageProvider, Store};
use kupferdraht::timeline::{
    DeliveryStatus, MessagesOptions, TimelineDiff, TimelineItem, TimelineItemKind, TimelineListener,
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

fn room_id() -> String {
    format!("{ROOM_UUID}@{DOMAIN}")
}

// ---- minimal protobuf writer (proto2 wire format, 72b precedent) ----

// ---- minimal protobuf reader (what the peer decrypted) ----

#[derive(Debug, Clone)]
enum Field {
    Varint,
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
            0 => {
                read_varint(buf, &mut pos);
                out.push((number, Field::Varint));
            }
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

async fn logged_in_client(
    server: &MockServer,
    provider: Option<Arc<dyn StorageProvider>>,
    home: &Path,
    keystore: Arc<dyn SecureKeyStore>,
) -> (Arc<kupferdraht::Client>, Arc<dyn Store>) {
    server.mock(|when, then| {
        when.method(POST)
            .path_includes("/v14/mls/key-packages/self/")
            .is_true(capture_key_package_upload);
        then.status(201);
    });
    common::mount_full_login_flow(server);
    // Through the `Kupferdraht` manager: it persists the session, which the
    // restart case resumes.
    let mut builder = kupferdraht::Kupferdraht::builder()
        .base_dir(home.to_path_buf())
        .keystore(keystore)
        .permit_http();
    if let Some(provider) = provider {
        builder = builder.storage_provider(provider);
    }
    let client = builder
        .build()
        .expect("open the home")
        .login(
            common::server_config(&server.base_url()),
            "alice@example.test".into(),
            "pw".into(),
            None,
            None,
        )
        .await
        .expect("login")
        .register_device()
        .await
        .expect("register device");
    let store = client.store().expect("store");
    (client, store)
}

struct Fixture {
    home: PathBuf,
    keystore: Arc<dyn SecureKeyStore>,
    client: Arc<kupferdraht::Client>,
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
    let (home, keystore) = fresh_home();
    welcomed_client_in(server, extra_messages, home, keystore, None).await
}

/// A leaked temp home and an in-memory keystore.
fn fresh_home() -> (PathBuf, Arc<dyn SecureKeyStore>) {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().to_path_buf();
    let _ = Box::leak(Box::new(dir));
    (home, Arc::new(InMemoryKeyStore::new()))
}

async fn welcomed_client_in(
    server: &MockServer,
    extra_messages: &[Vec<u8>],
    home: PathBuf,
    keystore: Arc<dyn SecureKeyStore>,
    provider: Option<Arc<dyn StorageProvider>>,
) -> Fixture {
    reset_capture_state_for(server);
    let (client, _store) = logged_in_client(server, provider, &home, keystore.clone()).await;
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
        home,
        keystore,
        client,
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

// ---- Step 97a helpers ----

const ACCOUNT: &str = "user-uuid@example.test";

/// An MLS upload that answers only after `delay` (capturing nothing).
fn mount_mls_upload_delayed(server: &MockServer, delay: Duration) -> Mock<'_> {
    server.mock(|when, then| {
        when.method(POST).path("/v14/mls/messages");
        then.status(201)
            .delay(delay)
            .header("content-type", "application/json")
            .body(r#"{"events":[],"time":"2026-09-28T10:00:02.000Z"}"#);
    })
}

/// Store methods that write — a fault armed on them after the upload hits
/// exactly the writes a send does once its upload has succeeded.
fn is_write(method: &str) -> bool {
    [
        "save_", "replace_", "clear_", "update_", "insert_", "set_", "mark_", "apply_", "expire_",
        "upsert_", "delete_",
    ]
    .iter()
    .any(|prefix| method.starts_with(prefix))
}

impl DiffRecorder {
    fn appends_for(&self, message_id: &str) -> Vec<TimelineItem> {
        self.diffs
            .lock()
            .expect("recorder lock")
            .iter()
            .filter_map(|d| match d {
                TimelineDiff::Append { item, .. } if item.item_id == message_id => {
                    Some(item.clone())
                }
                _ => None,
            })
            .collect()
    }
}

fn row_with_body(items: Vec<TimelineItem>, body: &str) -> Option<TimelineItem> {
    items.into_iter().find(|i| text_body(&i.kind) == Some(body))
}

// ---- tests ----

/// The B7 cancel case: the upload never answers and the caller gives up
/// (a Kotlin coroutine cancel drops the future). The row was written
/// before the upload, reads as `Failed` once nothing is in flight, and a
/// resend delivers the same message id.
#[tokio::test]
async fn a_send_cancelled_during_the_upload_leaves_a_failed_row_that_resends_under_its_id() {
    let server = MockServer::start();
    let mut fx = welcomed_client(&server, &[]).await;
    let mut hanging = mount_mls_upload_delayed(&server, Duration::from_secs(30));

    let cancelled = tokio::time::timeout(
        Duration::from_millis(500),
        fx.timeline.send_text("cancel me".into()),
    )
    .await;
    assert!(
        cancelled.is_err(),
        "the send was still waiting for the upload"
    );

    let row = row_with_body(items(&fx.client).await, "cancel me")
        .expect("the row was written before the upload");
    assert_eq!(
        row.delivery_status,
        DeliveryStatus::Failed,
        "no longer in flight → reads as Failed"
    );
    let failed = fx.room.failed_sends().await.expect("failed_sends");
    assert!(failed.iter().any(|i| i.item_id == row.item_id));

    hanging.delete();
    let ok = mount_mls_upload_ok(&server);
    let handle = fx
        .timeline
        .resend(row.item_id.clone())
        .await
        .expect("resend");
    assert_eq!(handle.message_id, row.item_id);
    assert_eq!(ok.calls(), 1);
    assert_eq!(
        item(&fx.client, &row.item_id).await.delivery_status,
        DeliveryStatus::Submitted
    );
    let uploads = captured_mls_uploads_for(&server);
    let (wire_id, _) = decode_generic_message(
        &fx.peer
            .decrypt_application_message(uploads.last().expect("the resend upload")),
    );
    assert_eq!(wire_id, row.item_id, "the same message id on the wire");
}

/// Poll until a row with `body` exists (at most 3 s).
async fn wait_for_row(client: &kupferdraht::Client, body: &str) -> TimelineItem {
    for _ in 0..60 {
        if let Some(row) = row_with_body(items(client).await, body) {
            return row;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("no row `{body}` within 3 s");
}

/// The row exists — and subscribers see it — while the upload is still
/// in flight: `Sending` first, then an update to `Submitted`.
#[tokio::test]
async fn the_row_is_appended_as_sending_while_the_upload_is_in_flight() {
    let server = MockServer::start();
    let fx = welcomed_client(&server, &[]).await;
    let _slow = mount_mls_upload_delayed(&server, Duration::from_secs(5));

    let timeline = fx.timeline.clone();
    let send = tokio::spawn(async move { timeline.send_text("in flight".into()).await });
    // Step 107: wait for the row, not a fixed sleep — the upload answers
    // only after 5 s, so the row is seen while it is in flight.
    let row = wait_for_row(&fx.client, "in flight").await;
    assert_eq!(row.delivery_status, DeliveryStatus::Sending);
    assert!(
        fx.recorder
            .appends_for(&row.item_id)
            .iter()
            .any(|i| i.delivery_status == DeliveryStatus::Sending),
        "subscribers see the Append before the upload answers"
    );

    let handle = send.await.expect("join").expect("send");
    assert_eq!(handle.message_id, row.item_id);
    assert_eq!(
        item(&fx.client, &row.item_id).await.delivery_status,
        DeliveryStatus::Submitted
    );
    assert!(
        fx.recorder
            .updates_for(&row.item_id)
            .iter()
            .any(|i| i.delivery_status == DeliveryStatus::Submitted),
        "the flip to Submitted is an UpdateAt"
    );
}

/// The B7 duplicate case: the upload succeeds, then every write fails.
/// Used to return an error — the user retries under a new id and the
/// message goes out twice. Now `Ok`; the row stays `Sending`, reads as
/// `Failed`, and a resend reuses the id (receivers dedupe on it).
#[tokio::test]
async fn a_write_failing_after_a_successful_upload_is_ok_and_the_row_resends_under_its_id() {
    let server = MockServer::start();
    let (home, keystore) = fresh_home();
    let provider = Arc::new(common::faulty_store::FaultyProvider::new(
        home.clone(),
        keystore.clone(),
    ));
    let fx = welcomed_client_in(&server, &[], home, keystore, Some(provider.clone())).await;
    let uploaded = Arc::new(AtomicBool::new(false));
    let flag = uploaded.clone();
    let mut accepted = server.mock(move |when, then| {
        when.method(POST)
            .path("/v14/mls/messages")
            .is_true(move |_| {
                flag.store(true, Ordering::SeqCst);
                true
            });
        then.status(201)
            .header("content-type", "application/json")
            .body(r#"{"events":[],"time":"2026-09-28T10:00:02.000Z"}"#);
    });
    let seen = uploaded.clone();
    provider.faulty().fail_when(Box::new(move |method| {
        seen.load(Ordering::SeqCst) && is_write(method)
    }));

    let handle = fx
        .timeline
        .send_text("sent, then the disk failed".into())
        .await
        .expect("an upload that succeeded is Ok");
    provider.faulty().heal();
    assert_eq!(accepted.calls(), 1);

    let row = item(&fx.client, &handle.message_id).await;
    assert_eq!(
        row.delivery_status,
        DeliveryStatus::Failed,
        "the status update was lost: the row stayed Sending and reads Failed"
    );
    accepted.delete();
    let _ok = mount_mls_upload_ok(&server);
    let again = fx
        .timeline
        .resend(handle.message_id.clone())
        .await
        .expect("resend");
    assert_eq!(again.message_id, handle.message_id, "the same id");
}

/// After a restart nothing is in flight: a `Sending` row the previous
/// process left behind reads as `Failed` and is listed for resend.
#[tokio::test]
async fn an_abandoned_sending_row_reads_failed_after_a_restart() {
    let server = MockServer::start();
    let fx = welcomed_client(&server, &[]).await;
    let _hanging = mount_mls_upload_delayed(&server, Duration::from_secs(30));
    let _ = tokio::time::timeout(
        Duration::from_millis(500),
        fx.timeline.send_text("left behind".into()),
    )
    .await;
    let (home, keystore) = (fx.home.clone(), fx.keystore.clone());
    drop(fx);

    let client = ClientBuilder::new()
        .permit_http()
        .base_dir(home)
        .keystore(keystore)
        .account(ACCOUNT)
        .resume()
        .await
        .expect("resume");
    let row =
        row_with_body(items(&client).await, "left behind").expect("the row survived the restart");
    assert_eq!(row.delivery_status, DeliveryStatus::Failed);
    client.sync_metadata().await.expect("reload the room list");
    let room = client.room(&room_id()).await.expect("room cached");
    assert!(
        room.failed_sends()
            .await
            .expect("failed_sends")
            .iter()
            .any(|i| i.item_id == row.item_id)
    );
}

/// A file: the blob upload succeeds, the MLS upload is cancelled. The row
/// and its attachment were written before the MLS upload, so the resend
/// reuses the uploaded blob.
#[tokio::test]
async fn a_file_send_cancelled_during_the_mls_upload_resends_without_a_second_blob_upload() {
    let server = MockServer::start();
    let fx = welcomed_client(&server, &[]).await;
    let assets = mount_asset_upload(&server);
    let mut hanging = mount_mls_upload_delayed(&server, Duration::from_secs(30));
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("notes.txt");
    std::fs::write(&path, b"resend me").expect("write file");

    let cancelled = tokio::time::timeout(
        Duration::from_millis(1500),
        fx.timeline.send_file(&path, "text/plain"),
    )
    .await;
    assert!(
        cancelled.is_err(),
        "the send was waiting for the MLS upload"
    );
    assert_eq!(assets.calls(), 1, "the blob went up");

    let failed = fx.room.failed_sends().await.expect("failed_sends");
    assert_eq!(
        failed.len(),
        1,
        "the file row was written before the MLS upload"
    );
    assert!(matches!(failed[0].kind, TimelineItemKind::Asset(_)));
    let id = failed[0].item_id.clone();

    hanging.delete();
    let ok = mount_mls_upload_ok(&server);
    let handle = fx.timeline.resend(id.clone()).await.expect("resend");
    assert_eq!(handle.message_id, id);
    assert_eq!(ok.calls(), 1);
    assert_eq!(assets.calls(), 1, "no second blob upload");
}

/// Step 107 (review of Step 97a): the lookup by id settles an abandoned
/// send like every other reader — `Failed`, not the raw `Sending`.
#[tokio::test]
async fn find_message_by_id_reads_an_abandoned_send_as_failed() {
    let server = MockServer::start();
    let fx = welcomed_client(&server, &[]).await;
    let _hanging = mount_mls_upload_delayed(&server, Duration::from_secs(30));
    let _ = tokio::time::timeout(
        Duration::from_millis(500),
        fx.timeline.send_text("abandoned".into()),
    )
    .await;
    let row = row_with_body(items(&fx.client).await, "abandoned").expect("the row");

    let found = fx
        .client
        .find_message_by_id(&row.item_id)
        .await
        .expect("lookup")
        .expect("stored");

    assert_eq!(found.delivery_status, DeliveryStatus::Failed);
}

/// Step 107 (review of Step 97a): while a resend's upload is in flight
/// the row is being sent — not listed as failed (a second resend would
/// upload it twice), and a concurrent resend of it is refused.
#[tokio::test]
async fn a_resend_in_flight_is_not_listed_as_failed() {
    let server = MockServer::start();
    let fx = welcomed_client(&server, &[]).await;
    let mut hanging = mount_mls_upload_delayed(&server, Duration::from_secs(30));
    let _ = tokio::time::timeout(
        Duration::from_millis(500),
        fx.timeline.send_text("resend me".into()),
    )
    .await;
    let row = row_with_body(items(&fx.client).await, "resend me").expect("the row");
    hanging.delete();
    let _slow = mount_mls_upload_delayed(&server, Duration::from_secs(5));

    let timeline = fx.timeline.clone();
    let id = row.item_id.clone();
    let resend = tokio::spawn(async move { timeline.resend(id).await });
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !resend.is_finished(),
        "the resend's upload is still in flight"
    );

    let failed = fx.room.failed_sends().await.expect("failed_sends");
    assert!(
        failed.iter().all(|i| i.item_id != row.item_id),
        "a row being resent is not a failed send"
    );
    let again = fx.timeline.resend(row.item_id.clone()).await;
    assert!(
        matches!(again, Err(kupferdraht::KupferdrahtError::InvalidInput(_))),
        "a second resend while the first is in flight is refused: {again:?}"
    );
    resend.abort();
}
