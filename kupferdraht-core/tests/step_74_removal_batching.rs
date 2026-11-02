// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 74 — batch external-removal commits (TDD red phase).
//!
//! Pins `docs/plan/74-batch-external-removal-commits.md`: the Step-28
//! auto-commit no longer fires per received `ProposalQueued` event
//! (which guarantees N−1 `mls-commit-missing-references` rejections
//! per N-deep backlog — observations §69/§70); rooms with queued
//! external Removes are collected during the dispatch cycle and
//! committed once at its end, failed flushes stay pending and retry
//! on the next cycle, and the expected mid-backlog
//! `mls-commit-missing-references` rejection is silent (no
//! `on_sync_error` spam for a self-resolving condition).
//!
//! Harness: the Step-69/72 family — key packages captured at
//! `register_device`, a `PeerGroup` fixture whose group carries a
//! fixture-held `external_senders` key welcomes the client in and
//! forges backend-style Remove proposals.

use std::sync::{Arc, Mutex};

use base64::Engine as _;
use httpmock::Method::{GET, POST};
use httpmock::{Mock, MockServer, prelude::HttpMockRequest};
use kupferdraht::storage::Store;
use kupferdraht::sync::{EventHandler, Message};
use kupferdraht::{ClientBuilder, SyncSettings};

mod common;

const ROOM_UUID: &str = "7c31d2a0-9b1e-4f6d-8a54-3f2ab9e01c74";
const DOMAIN: &str = "example.test";
/// b64("grp-74") — the backlogged conversation's MLS group id.
const GROUP_ID: &[u8] = b"grp-74";
const GROUP_ID_B64: &str = "Z3JwLTc0";
const PEER_IDENTITY: &[u8] = b"bob-uuid:bobdev74@example.test";
const CAROL_IDENTITY: &[u8] = b"carol-uuid:caroldev74@example.test";
/// Deterministic leaf layout (openmls assigns in add order):
/// creator = 0, welcomed client = 1, Carol = 2.
const PEER_LEAF: u32 = 0;
const CLIENT_LEAF: u32 = 1;
const CAROL_LEAF: u32 = 2;

// ── key-package capture (LEARNINGS §5.13: static keyed by Host,
// take the most recent entry — httpmock pools host:port) ─────────

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

// ── mounts ───────────────────────────────────────────────────────

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
                     "name":"B25",
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

/// One notification frame whose payload is `events` (pre-rendered
/// JSON). One frame = one dispatch cycle for `sync_once`'s drain.
fn mount_notifications_page(server: &MockServer, events: &[String]) {
    let payload = events.join(",");
    let body = format!(
        r#"{{"notifications":[{{"id":"9100e858-8eae-11ef-0000-000000000074","payload":[{payload}]}}],"has_more":false}}"#
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
             "qualified_from":{{"id":"bob-uuid","domain":"{DOMAIN}"}},
             "data":"{data_b64}",
             "time":"2026-08-24T10:00:00.000Z"}}"#
    )
}

fn message_event(data_b64: &str) -> String {
    format!(
        r#"{{"type":"conversation.mls-message-add",
             "qualified_conversation":{{"id":"{ROOM_UUID}","domain":"{DOMAIN}"}},
             "qualified_from":{{"id":"bob-uuid","domain":"{DOMAIN}"}},
             "data":"{data_b64}",
             "time":"2026-08-24T10:00:01.000Z"}}"#
    )
}

/// The removal-commit upload surface — hit counts pin the batching.
fn mount_commit_bundles<'a>(server: &'a MockServer, status: u16, body: &str) -> Mock<'a> {
    let body = body.to_string();
    server.mock(move |when, then| {
        when.method(POST).path("/v14/mls/commit-bundles");
        then.status(status)
            .header("content-type", "application/json")
            .body(body.clone());
    })
}

fn mount_commit_bundles_ok(server: &MockServer) -> Mock<'_> {
    mount_commit_bundles(
        server,
        201,
        r#"{"events":[],"time":"2026-08-24T10:00:02.000Z"}"#,
    )
}

/// The recovery/external-join surface — a converging backlog must
/// never churn through it (0 hits).
fn mount_groupinfo_guard(server: &MockServer) -> Mock<'_> {
    server.mock(|when, then| {
        when.method(GET)
            .path(format!("/v14/conversations/{DOMAIN}/{ROOM_UUID}/groupinfo"));
        then.status(200)
            .header("content-type", "message/mls")
            .body(common::mls::group_info_tls(GROUP_ID, PEER_IDENTITY));
    })
}

// ── error-recording handler ──────────────────────────────────────

#[derive(Default)]
struct ErrorRecorder {
    errors: Mutex<Vec<String>>,
}

impl ErrorRecorder {
    fn errors(&self) -> Vec<String> {
        self.errors.lock().expect("errors lock").clone()
    }
}

impl EventHandler for ErrorRecorder {
    fn on_message(&self, _message: Message) {}
    fn on_room_invited(&self, _room_id: String) {}
    fn on_sync_error(&self, description: String) {
        self.errors.lock().expect("errors lock").push(description);
    }
}

// ── shared setup ─────────────────────────────────────────────────

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

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Build the fixture peer, welcome the client in, add Carol, and
/// return the peer plus the pre-rendered welcome + Add-commit
/// events. After both events are processed the client sits at the
/// peer's epoch with leaves {peer 0, client 1, Carol 2}.
fn peer_with_client_and_carol(server: &MockServer) -> (common::mls::PeerGroup, Vec<String>) {
    let mut peer = common::mls::PeerGroup::new_with_external_sender(GROUP_ID, PEER_IDENTITY);
    let welcome = peer.welcome_over_key_package(&captured_key_package_for(server));
    let carol_kp = base64::engine::general_purpose::STANDARD
        .decode(common::mls::key_package_b64(CAROL_IDENTITY))
        .expect("carol kp b64");
    let add_commit = peer.add_member_commit(&carol_kp);
    let events = vec![
        welcome_event(&b64(&welcome)),
        message_event(&b64(&add_commit)),
    ];
    (peer, events)
}

// ── tests ────────────────────────────────────────────────────────

/// A backlog of two external Removes arriving in one drain produces
/// exactly ONE commit-bundle upload (covering both), no consumer
/// errors, and no recovery churn. Today's per-proposal auto-commit
/// uploads after the first proposal, so the second (same-epoch)
/// proposal lands on an already-advanced group — churn and errors
/// the batch model must not have.
#[tokio::test]
async fn backlog_drain_commits_once_covering_all_proposals() {
    let server = MockServer::start_async().await;
    mount_rooms_with_group(&server);
    let gi = mount_groupinfo_guard(&server);
    let bundles = mount_commit_bundles_ok(&server);
    let (client, _store) = logged_in_client(&server).await;
    let recorder = Arc::new(ErrorRecorder::default());
    client.add_event_handler(recorder.clone());

    let (peer, mut events) = peer_with_client_and_carol(&server);
    events.push(message_event(&b64(
        &peer.external_remove_proposal(PEER_LEAF)
    )));
    events.push(message_event(&b64(
        &peer.external_remove_proposal(CAROL_LEAF)
    )));
    mount_notifications_page(&server, &events);

    client
        .sync_once(SyncSettings::default())
        .await
        .expect("drain with removal backlog");

    assert_eq!(
        bundles.calls(),
        1,
        "one flush commit must cover the whole backlog"
    );
    assert!(
        recorder.errors().is_empty(),
        "a converging backlog must be silent, got: {:?}",
        recorder.errors()
    );
    assert_eq!(gi.calls(), 0, "no recovery churn from batched removals");
}

/// Wire rejecting the flush commit with `mls-commit-missing-references`
/// is the EXPECTED mid-backlog outcome on the push path (frames
/// arrive one by one): it must not surface `on_sync_error`, the room
/// stays pending, and the next cycle's flush converges.
#[tokio::test]
async fn missing_references_rejection_is_silent_and_converges() {
    let server = MockServer::start_async().await;
    mount_rooms_with_group(&server);
    let gi = mount_groupinfo_guard(&server);
    let mut reject = mount_commit_bundles(
        &server,
        400,
        r#"{"code":400,"label":"mls-commit-missing-references","message":"The commit is not referencing all pending proposals"}"#,
    );
    let (client, _store) = logged_in_client(&server).await;
    let recorder = Arc::new(ErrorRecorder::default());
    client.add_event_handler(recorder.clone());

    let (peer, mut events) = peer_with_client_and_carol(&server);
    events.push(message_event(&b64(
        &peer.external_remove_proposal(PEER_LEAF)
    )));
    events.push(message_event(&b64(
        &peer.external_remove_proposal(CAROL_LEAF)
    )));
    mount_notifications_page(&server, &events);

    client
        .sync_once(SyncSettings::default())
        .await
        .expect("drain survives the rejected flush");
    assert_eq!(
        reject.calls(),
        1,
        "one batched flush attempt in the first cycle (not one per proposal)"
    );
    assert!(
        recorder.errors().is_empty(),
        "mls-commit-missing-references is expected mid-backlog and must be silent, got: {:?}",
        recorder.errors()
    );

    // Wire accepts on the next cycle; the room stayed pending, so the
    // flush retries without any new proposal arriving.
    reject.delete();
    let accept = mount_commit_bundles_ok(&server);
    client
        .sync_once(SyncSettings::default())
        .await
        .expect("second drain retries the pending flush");
    assert_eq!(
        accept.calls(),
        1,
        "the pending room must retry its flush on the next cycle"
    );
    assert!(
        recorder.errors().is_empty(),
        "convergence must stay silent, got: {:?}",
        recorder.errors()
    );
    assert_eq!(gi.calls(), 0, "no recovery churn across the retry");
}

/// A transient upload failure (500) is a real error: it surfaces via
/// `on_sync_error` exactly once — and the room stays pending, so the
/// next cycle retries and converges.
#[tokio::test]
async fn transient_flush_failure_surfaces_once_and_retries() {
    let server = MockServer::start_async().await;
    mount_rooms_with_group(&server);
    let mut fail = mount_commit_bundles(&server, 500, r#"{"message":"upstream hiccup"}"#);
    let (client, _store) = logged_in_client(&server).await;
    let recorder = Arc::new(ErrorRecorder::default());
    client.add_event_handler(recorder.clone());

    let (peer, mut events) = peer_with_client_and_carol(&server);
    events.push(message_event(&b64(
        &peer.external_remove_proposal(PEER_LEAF)
    )));
    events.push(message_event(&b64(
        &peer.external_remove_proposal(CAROL_LEAF)
    )));
    mount_notifications_page(&server, &events);

    client
        .sync_once(SyncSettings::default())
        .await
        .expect("drain survives the failed flush");
    assert_eq!(
        fail.calls(),
        1,
        "one batched flush attempt, not one per proposal"
    );
    assert_eq!(
        recorder.errors().len(),
        1,
        "a transient flush failure surfaces exactly once, got: {:?}",
        recorder.errors()
    );

    fail.delete();
    let accept = mount_commit_bundles_ok(&server);
    client
        .sync_once(SyncSettings::default())
        .await
        .expect("second drain retries the pending flush");
    assert_eq!(
        accept.calls(),
        1,
        "the pending room must retry its flush on the next cycle"
    );
    assert_eq!(
        recorder.errors().len(),
        1,
        "the successful retry must not add errors, got: {:?}",
        recorder.errors()
    );
}

/// A backlog whose Removes include our own leaf keeps Step 28's
/// self-removal semantics at flush time: no upload, local group
/// discarded, room dropped from the cache.
#[tokio::test]
async fn self_removal_in_backlog_discards_group_and_room() {
    let server = MockServer::start_async().await;
    mount_rooms_with_group(&server);
    let bundles = mount_commit_bundles_ok(&server);
    let (client, _store) = logged_in_client(&server).await;
    let recorder = Arc::new(ErrorRecorder::default());
    client.add_event_handler(recorder.clone());

    let (peer, mut events) = peer_with_client_and_carol(&server);
    events.push(message_event(&b64(
        &peer.external_remove_proposal(CLIENT_LEAF)
    )));
    mount_notifications_page(&server, &events);

    client
        .sync_once(SyncSettings::default())
        .await
        .expect("drain with self-removal");

    assert_eq!(
        bundles.calls(),
        0,
        "a self-removal must never be self-committed"
    );
    let rooms = client.rooms().await;
    assert!(
        !rooms.iter().any(|r| r.id().to_string().contains(ROOM_UUID)),
        "the room must be dropped from the cache after a self-removal"
    );
}

// ── Step 132 item 13: the flush queue survives a restart, the send path heals ──
//
// Step 74 keeps a room whose flush the backend refused in the client's
// memory only; after a restart the Remove proposal still sits in the
// room's MLS state and every send there fails `PendingProposal` until
// another Remove arrives (former open.md §73, found by the Step-117
// browser smoke). Two layers (decided 2026-11-02): the queue is rebuilt
// at client start from the groups' stored pending proposals, and a send
// that meets a pending proposal flushes the room first and retries once.

/// A login whose home and keystore the caller keeps, so a second client
/// can be resumed over the same persistence.
async fn logged_in_client_at(
    server: &MockServer,
    path: &std::path::Path,
    keystore: Arc<dyn kupferdraht::keystore::SecureKeyStore>,
) -> Arc<kupferdraht::Client> {
    server.mock(|when, then| {
        when.method(POST)
            .path_includes("/v14/mls/key-packages/self/")
            .is_true(capture_key_package_upload);
        then.status(201);
    });
    common::mount_full_login_flow(server);
    // The manager's login persists the session a `ClientBuilder::resume`
    // finds (the Step 118 pattern).
    let app = kupferdraht::Kupferdraht::open(path.to_path_buf(), keystore)
        .expect("open kupferdraht")
        .permit_http();
    app.login(
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
    .expect("register device")
}

fn mount_message_send(server: &MockServer) -> Mock<'_> {
    server.mock(|when, then| {
        when.method(POST).path("/v14/mls/messages");
        then.status(201)
            .header("content-type", "application/json")
            .body(r#"{"events":[],"time":"2026-11-02T10:00:01.000Z"}"#);
    })
}

/// Leave a client with a queued external Remove whose flush the backend
/// refused with a 500 (the shape observations §92 records right after an
/// external commit): the proposal sits in the room's MLS state, the retry
/// only in the client's memory. Returns the account id for a resume.
async fn account_with_a_refused_flush(
    server: &MockServer,
    path: &std::path::Path,
    keystore: Arc<dyn kupferdraht::keystore::SecureKeyStore>,
) -> String {
    mount_rooms_with_group(server);
    let _gi = mount_groupinfo_guard(server);
    let mut refused = mount_commit_bundles(
        server,
        500,
        r#"{"code":500,"label":"server-error","message":"boom"}"#,
    );
    let client = logged_in_client_at(server, path, keystore).await;
    let (peer, mut events) = peer_with_client_and_carol(server);
    events.push(message_event(&b64(
        &peer.external_remove_proposal(CAROL_LEAF)
    )));
    mount_notifications_page(server, &events);
    client
        .sync_once(SyncSettings::default())
        .await
        .expect("drain with a refused flush");
    assert_eq!(
        refused.calls(),
        1,
        "the flush was attempted once and refused"
    );
    refused.delete();
    let account_id = client.user_id().expect("user id").as_str().to_string();
    drop(client);
    account_id
}

async fn resumed_client(
    path: &std::path::Path,
    keystore: Arc<dyn kupferdraht::keystore::SecureKeyStore>,
    account_id: String,
) -> Arc<kupferdraht::Client> {
    ClientBuilder::new()
        .permit_http()
        .base_dir(path.to_path_buf())
        .keystore(keystore)
        .account(account_id)
        .resume()
        .await
        .expect("resume over the same home")
}

#[tokio::test]
async fn restart_rebuilds_the_removal_flush_queue() {
    let server = MockServer::start_async().await;
    let dir = tempfile::tempdir().expect("tempdir");
    let keystore: Arc<dyn kupferdraht::keystore::SecureKeyStore> =
        Arc::new(kupferdraht::keystore::InMemoryKeyStore::new());
    let account_id = account_with_a_refused_flush(&server, dir.path(), keystore.clone()).await;

    let accepted = mount_commit_bundles_ok(&server);
    let resumed = resumed_client(dir.path(), keystore, account_id).await;
    resumed
        .sync_once(SyncSettings::default())
        .await
        .expect("first cycle after the restart");
    assert_eq!(
        accepted.calls(),
        1,
        "the queue rebuilt from the stored pending proposal must flush on the first cycle"
    );
}

#[tokio::test]
async fn send_commits_a_stored_pending_removal_first() {
    let server = MockServer::start_async().await;
    let dir = tempfile::tempdir().expect("tempdir");
    let keystore: Arc<dyn kupferdraht::keystore::SecureKeyStore> =
        Arc::new(kupferdraht::keystore::InMemoryKeyStore::new());
    let account_id = account_with_a_refused_flush(&server, dir.path(), keystore.clone()).await;

    let accepted = mount_commit_bundles_ok(&server);
    let sent = mount_message_send(&server);
    let resumed = resumed_client(dir.path(), keystore, account_id).await;
    let room = resumed
        .room_or_load(&format!("{ROOM_UUID}@{DOMAIN}"))
        .await
        .expect("the room is in the store");
    room.timeline()
        .expect("timeline")
        .send_text("after the restart".into())
        .await
        .expect("a send over a stored pending proposal commits it first, then sends");
    assert_eq!(accepted.calls(), 1, "one commit covers the stored proposal");
    assert_eq!(sent.calls(), 1, "the message goes out after the commit");
}
