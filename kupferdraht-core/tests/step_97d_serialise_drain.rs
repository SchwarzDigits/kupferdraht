// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 97d — serialise the drain per account; defer, don't heal, around
//! own pending commits (ADR-0031, review blocker B8). TDD red.
//!
//! The out-of-order healing (`recover_room` → discard + external rejoin)
//! fires today whenever an inbound MLS message's epoch does not match the
//! local group — and loses that message. Three ways to get there without
//! any real gap:
//!
//! 1. two `Client`s on one account drain the same queue at once and
//!    process the same commit twice;
//! 2. a message from the next epoch arrives while an own commit is
//!    uploading (staged, not yet merged) — in the same `Client`, or in
//!    another one on the account;
//! 3. an old commit is delivered again.
//!
//! A rejoin is observable as `GET …/groupinfo` (the external-join
//! entry); the mock answers 404, so the healing fails loudly instead of
//! joining. A fixture peer (`common::mls::PeerGroup`, bob) owns the MLS
//! group; carol is a second member the client can remove.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use httpmock::Method::{GET, POST};
use httpmock::{Mock, MockServer, prelude::HttpMockRequest};
use kupferdraht::keystore::{InMemoryKeyStore, SecureKeyStore};
use kupferdraht::storage::Store;
use kupferdraht::{Client, ClientBuilder, EventHandler, Kupferdraht, Message, SyncSettings};

mod common;

const ROOM_UUID: &str = "97d0aa31-77aa-4bde-9d10-223344556677";
const DOMAIN: &str = "example.test";
/// b64("grp-97d") — the room's MLS group id.
const GROUP_ID_B64: &str = "Z3JwLTk3ZA==";
const GROUP_ID: &[u8] = b"grp-97d";
const BOB: &str = "bob-uuid";
const BOB_IDENTITY: &[u8] = b"bob-uuid:bobdev01@example.test";
const CAROL_IDENTITY: &[u8] = b"carol-uuid:caroldev1@example.test";
const CAROL_USER: &str = "carol-uuid@example.test";
/// How long the commit-bundle upload takes — the window a next-epoch
/// message arrives in.
const UPLOAD_DELAY: Duration = Duration::from_millis(1500);

fn room_id() -> String {
    format!("{ROOM_UUID}@{DOMAIN}")
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

// ---- minimal protobuf writer (proto2 wire format, the Step-72b precedent) ----

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

/// `GenericMessage { message_id = 1, text = 2 { content = 1 } }`.
fn text_generic_message(message_id: &str, content: &str) -> Vec<u8> {
    let mut text = Vec::new();
    put_len_delimited(1, content.as_bytes(), &mut text);
    let mut m = Vec::new();
    put_len_delimited(1, message_id.as_bytes(), &mut m);
    put_len_delimited(2, &text, &mut m);
    m
}

// ---- the queue ----

/// One queued notification: its id and the events it carries.
struct Notif {
    id: &'static str,
    events: Vec<String>,
}

fn welcome_event(welcome: &[u8]) -> String {
    format!(
        r#"{{"type":"conversation.mls-welcome",
             "qualified_conversation":{{"id":"{ROOM_UUID}","domain":"{DOMAIN}"}},
             "qualified_from":{{"id":"{BOB}","domain":"{DOMAIN}"}},
             "data":"{}","time":"2026-09-28T10:00:00.000Z"}}"#,
        b64(welcome)
    )
}

/// A `conversation.mls-message-add` from bob — a commit or an
/// application message.
fn mls_event(tls: &[u8]) -> String {
    format!(
        r#"{{"type":"conversation.mls-message-add",
             "qualified_conversation":{{"id":"{ROOM_UUID}","domain":"{DOMAIN}"}},
             "qualified_from":{{"id":"{BOB}","domain":"{DOMAIN}"}},
             "from_client":"bobdev01",
             "data":"{}","time":"2026-09-28T10:00:01.000Z"}}"#,
        b64(tls)
    )
}

fn page_body(notifs: &[Notif]) -> String {
    let items: Vec<String> = notifs
        .iter()
        .map(|n| format!(r#"{{"id":"{}","payload":[{}]}}"#, n.id, n.events.join(",")))
        .collect();
    format!(
        r#"{{"notifications":[{}],"has_more":false}}"#,
        items.join(",")
    )
}

fn since(req: &HttpMockRequest) -> Option<String> {
    req.query_params_map().get("since").cloned()
}

/// Answer `GET /notifications` for one `since` cursor (`None` = no
/// cursor yet) with `notifs`, after `delay`.
fn mount_page(
    server: &MockServer,
    cursor: Option<&'static str>,
    notifs: &[Notif],
    delay: Duration,
) {
    let body = page_body(notifs);
    server.mock(move |when, then| {
        when.method(GET)
            .path("/v14/notifications")
            .is_true(move |req| since(req).as_deref() == cursor);
        then.status(200)
            .delay(delay)
            .header("content-type", "application/json")
            .body(body.clone());
    });
}

// ---- mocks ----

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

/// A key package this test's client uploaded — the most recent upload
/// to this server: httpmock pools servers, so an earlier test may have
/// used the same address (LEARNINGS §5.13).
fn client_key_package(server: &MockServer) -> Vec<u8> {
    let host = server.address().to_string();
    let guard = UPLOADED_KPS.lock().expect("capture lock");
    let (_, kps) = guard
        .iter()
        .rev()
        .find(|(h, _)| *h == host)
        .expect("register_device uploaded key packages");
    base64::engine::general_purpose::STANDARD
        .decode(kps.first().expect("one key package"))
        .expect("base64 key package")
}

fn mount_room(server: &MockServer) {
    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list-ids");
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(
                r#"{{"qualified_conversations":[{{"id":"{ROOM_UUID}","domain":"{DOMAIN}"}}],
                     "has_more":false,"paging_state":"END"}}"#
            ));
    });
    let body = format!(
        r#"{{"found":[{{
             "qualified_id":{{"id":"{ROOM_UUID}","domain":"{DOMAIN}"}},
             "type":0,"protocol":"mls","epoch":2,"group_id":"{GROUP_ID_B64}",
             "name":"Serialised drain",
             "members":{{
               "self":{{"qualified_id":{{"id":"user-uuid","domain":"{DOMAIN}"}},
                       "conversation_role":"wire_admin"}},
               "others":[
                 {{"qualified_id":{{"id":"{BOB}","domain":"{DOMAIN}"}},"conversation_role":"wire_admin"}},
                 {{"qualified_id":{{"id":"carol-uuid","domain":"{DOMAIN}"}},"conversation_role":"wire_member"}}
               ]
             }}
           }}], "not_found":[], "failed":[]}}"#
    );
    server.mock(move |when, then| {
        when.method(POST).path("/v14/conversations/list");
        then.status(200)
            .header("content-type", "application/json")
            .body(body.clone());
    });
    server.mock(|when, then| {
        when.method(POST).path("/v14/list-users");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"found":[]}"#);
    });
}

/// The external-join entry of a rejoin — answered 404 so a heal fails
/// loudly; its call count is what the tests assert on.
fn mount_rejoin_probe(server: &MockServer) -> Mock<'_> {
    server.mock(|when, then| {
        when.method(GET).path_includes("/groupinfo");
        then.status(404)
            .header("content-type", "application/json")
            .body(r#"{"code":404,"label":"mls-missing-group-info","message":"x"}"#);
    })
}

/// `POST /v14/mls/commit-bundles`: remember the bundle, answer 201 after
/// [`UPLOAD_DELAY`].
fn mount_slow_commit_upload(server: &MockServer) -> Arc<Mutex<Option<Vec<u8>>>> {
    let captured = Arc::new(Mutex::new(None));
    let sink = captured.clone();
    server.mock(move |when, then| {
        when.method(POST)
            .path("/v14/mls/commit-bundles")
            .is_true(move |req| {
                *sink.lock().expect("bundle lock") = Some(req.body_ref().to_vec());
                true
            });
        then.status(201)
            .delay(UPLOAD_DELAY)
            .header("content-type", "application/json")
            .body(r#"{"events":[],"time":"2026-09-28T10:00:02.000Z"}"#);
    });
    captured
}

/// Wait until the client's commit bundle reached the mock, return the
/// commit segment.
async fn uploaded_commit(captured: &Arc<Mutex<Option<Vec<u8>>>>) -> Vec<u8> {
    for _ in 0..500 {
        if let Some(bundle) = captured.lock().expect("bundle lock").clone() {
            let segments = common::mls::bundle_segments(&bundle);
            return segments.first().expect("a commit segment").1.clone();
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the commit bundle never reached the server");
}

// ---- clients ----

struct Account {
    home: tempfile::TempDir,
    keystore: Arc<dyn SecureKeyStore>,
}

async fn logged_in_client(server: &MockServer) -> (Arc<Client>, Account) {
    server.mock(|when, then| {
        when.method(POST)
            .path_includes("/v14/mls/key-packages/self/")
            .is_true(capture_key_package_upload);
        then.status(201);
    });
    common::mount_full_login_flow(server);
    let home = tempfile::tempdir().expect("tempdir");
    let keystore: Arc<dyn SecureKeyStore> = Arc::new(InMemoryKeyStore::new());
    let app = Kupferdraht::open(home.path().to_path_buf(), keystore.clone())
        .expect("open kupferdraht")
        .permit_http();
    let client = app
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
    (client, Account { home, keystore })
}

/// A second, independent `Client` on the same account — what a second
/// process looks like from the databases' side (the Step-85 precedent).
async fn second_client(first: &Arc<Client>, account: &Account) -> Arc<Client> {
    let user = first.user_id().expect("user id").as_str().to_string();
    ClientBuilder::new()
        .permit_http()
        .base_dir(account.home.path().to_path_buf())
        .keystore(account.keystore.clone())
        .account(user.as_str())
        .resume()
        .await
        .expect("resume a second client on the account")
}

/// Bob's group with carol in it, and the Welcome that adds the client.
fn bob_group_with_carol_and_welcome(server: &MockServer) -> (common::mls::PeerGroup, Vec<u8>) {
    let mut bob = common::mls::PeerGroup::new_mixed_plaintext(GROUP_ID, BOB_IDENTITY);
    let carol = common::mls::PeerDevice::new(CAROL_IDENTITY);
    let _ = bob.add_member_commit(&carol.key_package_tls());
    let welcome = bob.welcome_over_key_package(&client_key_package(server));
    (bob, welcome)
}

#[derive(Default)]
struct MessageRecorder {
    ids: Mutex<Vec<String>>,
}

impl EventHandler for MessageRecorder {
    fn on_message(&self, message: Message) {
        self.ids.lock().expect("ids").push(message.message_id);
    }
    fn on_room_invited(&self, _room_id: String) {}
    fn on_sync_error(&self, _description: String) {}
}

impl MessageRecorder {
    fn count(&self, message_id: &str) -> usize {
        self.ids
            .lock()
            .expect("ids")
            .iter()
            .filter(|id| *id == message_id)
            .count()
    }
}

async fn drain(client: &Client) {
    client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync_once");
}

async fn cursor(store: &Arc<dyn Store>) -> Option<String> {
    store
        .load_sync_state()
        .await
        .expect("sync state")
        .and_then(|s| s.last_event_id)
}

// ---- tests ----

/// Gap 1: two `Client`s on one account drain the same page at once. Only
/// one may process it; the other sees the moved cursor and refetches.
/// Before: the commit ran twice, the second time at an old epoch → heal.
#[tokio::test]
async fn two_clients_on_one_account_process_each_notification_once() {
    let server = MockServer::start();
    let (a, account) = logged_in_client(&server).await;
    mount_room(&server);
    let rejoin = mount_rejoin_probe(&server);
    let (mut bob, welcome) = bob_group_with_carol_and_welcome(&server);
    let dave = common::mls::PeerDevice::new(b"dave-uuid:davedev1@example.test");
    let commit = bob.add_member_commit(&dave.key_package_tls());
    let message = bob.application_message(&text_generic_message("m-97d-1", "once"));
    let queue = [
        Notif {
            id: "n1",
            events: vec![welcome_event(&welcome)],
        },
        Notif {
            id: "n2",
            events: vec![mls_event(&commit)],
        },
        Notif {
            id: "n3",
            events: vec![mls_event(&message)],
        },
    ];
    mount_page(&server, None, &queue, Duration::from_millis(300));
    mount_page(&server, Some("n1"), &queue[1..], Duration::ZERO);
    mount_page(&server, Some("n2"), &queue[2..], Duration::ZERO);
    mount_page(&server, Some("n3"), &[], Duration::ZERO);
    let b = second_client(&a, &account).await;
    let recorder = Arc::new(MessageRecorder::default());
    a.add_event_handler(recorder.clone());
    b.add_event_handler(recorder.clone());

    tokio::join!(drain(&a), drain(&b));

    assert_eq!(rejoin.calls(), 0, "no rejoin — nothing was processed twice");
    assert_eq!(
        recorder.count("m-97d-1"),
        1,
        "the message is dispatched exactly once across both clients"
    );
}

/// Gap 2, one `Client`: bob follows the client's own commit while its
/// upload is still in flight and writes at the new epoch. The drain must
/// wait for the merge and decrypt it — not heal around it.
#[tokio::test]
async fn a_next_epoch_message_during_an_own_commit_upload_waits_for_the_merge() {
    let server = MockServer::start();
    let (client, _account) = logged_in_client(&server).await;
    mount_room(&server);
    let rejoin = mount_rejoin_probe(&server);
    let (mut bob, welcome) = bob_group_with_carol_and_welcome(&server);
    mount_page(
        &server,
        None,
        &[Notif {
            id: "n1",
            events: vec![welcome_event(&welcome)],
        }],
        Duration::ZERO,
    );
    let recorder = Arc::new(MessageRecorder::default());
    client.add_event_handler(recorder.clone());
    drain(&client).await;
    let captured = mount_slow_commit_upload(&server);

    let kicker = client.clone();
    let kick = tokio::spawn(async move {
        kicker
            .kick_members(&room_id(), &[CAROL_USER.to_string()])
            .await
            .map(|_| ())
    });
    bob.process_commit(&uploaded_commit(&captured).await);
    let message = bob.application_message(&text_generic_message("m-97d-2", "after"));
    mount_page(
        &server,
        Some("n1"),
        &[Notif {
            id: "n2",
            events: vec![mls_event(&message)],
        }],
        Duration::ZERO,
    );
    drain(&client).await;
    kick.await.expect("kick task").expect("the kick succeeds");

    assert_eq!(rejoin.calls(), 0, "no rejoin around the own commit");
    assert_eq!(
        recorder.count("m-97d-2"),
        1,
        "the next-epoch message is decrypted after the merge"
    );
}

/// Gap 2 across `Client`s: client A's commit is uploading; client B on
/// the same account meets bob's next-epoch message. B has no flow of its
/// own to wait for, so it stops before the message (cursor stays) instead
/// of healing — and processes it on a later drain, after A merged.
#[tokio::test]
async fn another_clients_pending_commit_defers_the_message_instead_of_healing() {
    let server = MockServer::start();
    let (a, account) = logged_in_client(&server).await;
    mount_room(&server);
    let rejoin = mount_rejoin_probe(&server);
    let (mut bob, welcome) = bob_group_with_carol_and_welcome(&server);
    mount_page(
        &server,
        None,
        &[Notif {
            id: "n1",
            events: vec![welcome_event(&welcome)],
        }],
        Duration::ZERO,
    );
    drain(&a).await;
    let b = second_client(&a, &account).await;
    let b_store = b.store().expect("b store");
    let recorder = Arc::new(MessageRecorder::default());
    b.add_event_handler(recorder.clone());
    let captured = mount_slow_commit_upload(&server);

    let kicker = a.clone();
    let kick = tokio::spawn(async move {
        kicker
            .kick_members(&room_id(), &[CAROL_USER.to_string()])
            .await
            .map(|_| ())
    });
    bob.process_commit(&uploaded_commit(&captured).await);
    let message = bob.application_message(&text_generic_message("m-97d-3", "deferred"));
    mount_page(
        &server,
        Some("n1"),
        &[Notif {
            id: "n2",
            events: vec![mls_event(&message)],
        }],
        Duration::ZERO,
    );
    drain(&b).await;

    assert_eq!(
        rejoin.calls(),
        0,
        "no rejoin around another client's commit"
    );
    assert_eq!(
        cursor(&b_store).await.as_deref(),
        Some("n1"),
        "the drain stops before the deferred message"
    );
    assert_eq!(recorder.count("m-97d-3"), 0, "not decrypted yet");

    kick.await.expect("kick task").expect("the kick succeeds");
    drain(&b).await;
    assert_eq!(
        recorder.count("m-97d-3"),
        1,
        "after the merge a later drain decrypts it"
    );
    assert_eq!(rejoin.calls(), 0);
}

/// H4, in part: a commit delivered again (older than the group's epoch)
/// is skipped, not healed around.
#[tokio::test]
async fn a_redelivered_old_commit_is_skipped_without_a_rejoin() {
    let server = MockServer::start();
    let (client, _account) = logged_in_client(&server).await;
    mount_room(&server);
    let rejoin = mount_rejoin_probe(&server);
    let (mut bob, welcome) = bob_group_with_carol_and_welcome(&server);
    let dave = common::mls::PeerDevice::new(b"dave-uuid:davedev1@example.test");
    let commit = bob.add_member_commit(&dave.key_package_tls());
    let message = bob.application_message(&text_generic_message("m-97d-4", "still fine"));
    mount_page(
        &server,
        None,
        &[
            Notif {
                id: "n1",
                events: vec![welcome_event(&welcome)],
            },
            Notif {
                id: "n2",
                events: vec![mls_event(&commit)],
            },
            Notif {
                id: "n3",
                events: vec![mls_event(&commit)],
            },
            Notif {
                id: "n4",
                events: vec![mls_event(&message)],
            },
        ],
        Duration::ZERO,
    );
    let recorder = Arc::new(MessageRecorder::default());
    client.add_event_handler(recorder.clone());

    drain(&client).await;

    assert_eq!(rejoin.calls(), 0, "an old commit is skipped, not healed");
    assert_eq!(
        recorder.count("m-97d-4"),
        1,
        "the group stays usable at the current epoch"
    );
}

/// Found by the Step-97d smoke (two `sync` processes on one home): a
/// store transaction that reads, then writes, started `DEFERRED` — when
/// another process committed while it waited for the write lock, SQLite
/// failed it at once (`database is locked`, a stale WAL snapshot)
/// instead of waiting (`busy_timeout`). Writing transactions start
/// `IMMEDIATE`.
#[tokio::test]
async fn a_read_then_write_transaction_waits_for_another_writer() {
    use kupferdraht::storage::SqliteStore;
    use kupferdraht::storage::db_key::generate_db_key;
    use std::time::Instant;

    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("kupferdraht.db");
    let key = generate_db_key();
    let writer_store = SqliteStore::open(&path, &key).expect("writer store");
    let sweeping_store = SqliteStore::open(&path, &key).expect("sweeping store");
    let (begun_tx, begun_rx) = std::sync::mpsc::channel::<()>();
    let writer = std::thread::spawn(move || {
        let conn = writer_store.lock().expect("lock");
        conn.execute_batch(
            "BEGIN IMMEDIATE; INSERT INTO sync_state (id, last_event_id) VALUES (1, 'n-other');",
        )
        .expect("write under the lock");
        begun_tx.send(()).expect("announce");
        std::thread::sleep(Duration::from_millis(300));
        conn.execute_batch("COMMIT").expect("commit");
    });
    begun_rx.recv().expect("the writer holds the write lock");

    let started = Instant::now();
    sweeping_store
        .expire_ephemeral_messages(1)
        .await
        .expect("the read-then-write sweep waits instead of failing `database is locked`");
    assert!(
        started.elapsed() >= Duration::from_millis(200),
        "the sweep actually waited for the writer"
    );
    writer.join().expect("writer thread");
}
