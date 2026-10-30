// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 95a — the sender of an inbound message is the MLS-authenticated
//! leaf credential, not the Wire envelope (ADR-0027). TDD red.
//!
//! A fixture peer (`common::mls::PeerGroup`) invites the client via
//! Welcome and sends application messages through the full receive
//! pipe (MLS decrypt → `GenericMessage` → persist → dispatch). The
//! `conversation.mls-message-add` envelope is written by the test and
//! is free to lie about `qualified_from` / `from_client` — exactly what
//! a compromised backend can do. The leaf that signed the ciphertext is
//! bob's (`bob-uuid:bobdev01@example.test`, Wire's W-4 shape).
//!
//! `wire::proto` is crate-private (ADR-0009), so the test hand-emits
//! the protobuf bytes (the Step-72b precedent).

use std::sync::{Arc, Mutex};

use base64::Engine as _;
use httpmock::Method::{GET, POST};
use httpmock::{MockServer, prelude::HttpMockRequest};
use kupferdraht::storage::Store;
use kupferdraht::{ClientBuilder, SyncSettings};
use kupferdraht::{EventHandler, Message};

mod common;

const ROOM_UUID: &str = "95a0aa31-77aa-4bde-9d10-223344556677";
const DOMAIN: &str = "example.test";
/// b64("grp-95a") — the room's MLS group id.
const GROUP_ID_B64: &str = "Z3JwLTk1YQ==";
const GROUP_ID: &[u8] = b"grp-95a";
const BOB: &str = "bob-uuid";
const CAROL: &str = "carol-uuid";
const BOB_IDENTITY: &[u8] = b"bob-uuid:bobdev01@example.test";

fn room_id() -> String {
    format!("{ROOM_UUID}@{DOMAIN}")
}

// ---- minimal protobuf writer (proto2 wire format) ----

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

/// `GenericMessage { message_id = 1, text = 2 { content = 1 } }`.
fn text_generic_message(message_id: &str, content: &str) -> Vec<u8> {
    let mut text = Vec::new();
    put_len_delimited(1, content.as_bytes(), &mut text);
    let mut m = Vec::new();
    put_len_delimited(1, message_id.as_bytes(), &mut m);
    put_len_delimited(2, &text, &mut m);
    m
}

/// `GenericMessage { message_id = 1, task = 262000 }` with the minimal
/// `TaskPayload { task_id = 1, title = 2, status = 3 }` (mode a).
fn task_generic_message(message_id: &str, task_id: &str, title: &str) -> Vec<u8> {
    let mut payload = Vec::new();
    put_len_delimited(1, task_id.as_bytes(), &mut payload);
    put_len_delimited(2, title.as_bytes(), &mut payload);
    put_uint(3, 0 /* OPEN */, &mut payload);
    let mut m = Vec::new();
    put_len_delimited(1, message_id.as_bytes(), &mut m);
    put_len_delimited(262_000, &payload, &mut m);
    m
}

/// `GenericMessage { message_id = 1, case = 262001 }` with the minimal
/// `CasePayload { revision = 1, case_ref = 2, title = 4, state = 6 }`.
fn case_generic_message(message_id: &str, title: &str) -> Vec<u8> {
    let mut payload = Vec::new();
    put_uint(1, 1, &mut payload);
    put_len_delimited(2, b"case-1", &mut payload);
    put_len_delimited(4, title.as_bytes(), &mut payload);
    put_uint(6, 1 /* ACTIVE */, &mut payload);
    let mut m = Vec::new();
    put_len_delimited(1, message_id.as_bytes(), &mut m);
    put_len_delimited(262_001, &payload, &mut m);
    m
}

// ---- mocks (Step-69 / 72b shapes) ----

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

/// Room metadata listing bob and carol with the given roles.
fn mount_room(server: &MockServer, bob_role: &str, carol_role: &str) {
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
    let body = format!(
        r#"{{"found":[{{
             "qualified_id":{{"id":"{ROOM_UUID}","domain":"{DOMAIN}"}},
             "type":0,"protocol":"mls","epoch":1,"group_id":"{GROUP_ID_B64}",
             "name":"Sender auth",
             "members":{{
               "self":{{"qualified_id":{{"id":"user-uuid","domain":"{DOMAIN}"}},
                       "conversation_role":"wire_admin"}},
               "others":[
                 {{"qualified_id":{{"id":"{BOB}","domain":"{DOMAIN}"}},
                   "conversation_role":"{bob_role}"}},
                 {{"qualified_id":{{"id":"{CAROL}","domain":"{DOMAIN}"}},
                   "conversation_role":"{carol_role}"}}
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

fn mount_notifications_page(server: &MockServer, events: &[String]) {
    let payload = events.join(",");
    let body = format!(
        r#"{{"notifications":[{{"id":"95a0e858-8eae-11ef-0000-00000000cafe","payload":[{payload}]}}],"has_more":false}}"#
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
             "time":"2026-09-26T10:00:00.000Z"}}"#
    )
}

/// The envelope the backend writes around a ciphertext. `from` is the
/// claimed `(user uuid, client id)`; `None` omits both fields.
fn message_event(data_b64: &str, from: EnvelopeFrom<'_>) -> String {
    let from_fields = from.map_or_else(String::new, |(user, client)| {
        format!(
            r#""qualified_from":{{"id":"{user}","domain":"{DOMAIN}"}},
               "from_client":"{client}","#
        )
    });
    format!(
        r#"{{"type":"conversation.mls-message-add",
             "qualified_conversation":{{"id":"{ROOM_UUID}","domain":"{DOMAIN}"}},
             {from_fields}
             "data":"{data_b64}",
             "time":"2026-09-26T10:00:01.000Z"}}"#
    )
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

/// The envelope's claimed `(user uuid, client id)`; `None` omits both.
type EnvelopeFrom<'a> = Option<(&'a str, &'a str)>;

/// Welcome for this client + one application message per `(plaintext,
/// envelope sender)`, all in one notifications page.
fn mount_welcome_and_messages(
    server: &MockServer,
    peer: &mut common::mls::PeerGroup,
    messages: &[(Vec<u8>, EnvelopeFrom<'_>)],
) {
    let b64 = |bytes: Vec<u8>| base64::engine::general_purpose::STANDARD.encode(bytes);
    let welcome = b64(peer.welcome_over_key_package(&captured_key_package_for(server)));
    let mut events = vec![welcome_event(&welcome)];
    for (plaintext, from) in messages {
        events.push(message_event(
            &b64(peer.application_message(plaintext)),
            *from,
        ));
    }
    mount_notifications_page(server, &events);
}

// ---- observers ----

#[derive(Default)]
struct MessageRecorder {
    messages: Mutex<Vec<(String, Option<String>, String)>>,
}

impl MessageRecorder {
    /// `(sender_user_id, sender_device_id, message_id)` per `on_message`.
    fn seen(&self) -> Vec<(String, Option<String>, String)> {
        self.messages.lock().expect("recorder lock").clone()
    }
}

impl EventHandler for MessageRecorder {
    fn on_message(&self, message: Message) {
        self.messages.lock().expect("recorder lock").push((
            message.sender_user_id,
            message.sender_device_id,
            message.message_id,
        ));
    }
    fn on_room_invited(&self, _room_id: String) {}
    fn on_sync_error(&self, _description: String) {}
}

/// Minimal capturing `tracing::Subscriber` (the `keystore/file_store.rs`
/// pattern) — records every event's level + rendered fields. Installed
/// ONCE as the global default for this test binary ([`captured_events`]):
/// a thread-local `set_default` was flaky on CI, where tests run in
/// parallel and `tracing`'s callsite interest cache is global. Tests
/// filter the shared log by their own message id.
#[derive(Default)]
struct CaptureSubscriber {
    events: Arc<Mutex<Vec<(tracing::Level, String)>>>,
}

impl tracing::Subscriber for CaptureSubscriber {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        let mut rendered = String::new();
        event.record(&mut FieldRenderer(&mut rendered));
        self.events
            .lock()
            .expect("capture lock")
            .push((*event.metadata().level(), rendered));
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

struct FieldRenderer<'a>(&'a mut String);

impl tracing::field::Visit for FieldRenderer<'_> {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        use std::fmt::Write as _;
        let _ = write!(self.0, "{}={value:?} ", field.name());
    }
}

/// Every captured event: level + rendered fields.
type CapturedEvents = Arc<Mutex<Vec<(tracing::Level, String)>>>;

/// The binary-wide capture, installed as the global default on first use.
fn captured_events() -> CapturedEvents {
    static EVENTS: std::sync::OnceLock<CapturedEvents> = std::sync::OnceLock::new();
    EVENTS
        .get_or_init(|| {
            let subscriber = CaptureSubscriber::default();
            let events = Arc::clone(&subscriber.events);
            tracing::subscriber::set_global_default(subscriber)
                .expect("the only global subscriber of this test binary");
            events
        })
        .clone()
}

fn warns(events: &CapturedEvents) -> Vec<String> {
    events
        .lock()
        .expect("capture lock")
        .iter()
        .filter(|(level, _)| *level == tracing::Level::WARN)
        .map(|(_, rendered)| rendered.clone())
        .collect()
}

async fn sync(client: &kupferdraht::Client) {
    client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync_once");
}

async fn row_sender(store: &Arc<dyn Store>, message_id: &str) -> Option<(String, String)> {
    store
        .load_timeline_events(&room_id())
        .await
        .expect("load rows")
        .into_iter()
        .find(|r| r.message_id == message_id)
        .map(|r| (r.sender_user_id, r.sender_device_id))
}

// ---- tests ----

/// The core fix: the envelope claims carol, the MLS leaf is bob —
/// every surface (row, `EventHandler`, timeline item) carries bob.
#[tokio::test]
async fn envelope_sender_is_replaced_by_the_mls_leaf_identity() {
    let server = MockServer::start();
    let (client, store) = logged_in_client(&server).await;
    let recorder = Arc::new(MessageRecorder::default());
    client.add_event_handler(recorder.clone());
    mount_room(&server, "wire_member", "wire_member");
    let mut peer = common::mls::PeerGroup::new(GROUP_ID, BOB_IDENTITY);
    mount_welcome_and_messages(
        &server,
        &mut peer,
        &[(
            text_generic_message("m-forged", "hello"),
            Some((CAROL, "carolphone")),
        )],
    );

    sync(&client).await;

    assert_eq!(
        row_sender(&store, "m-forged").await,
        Some(("bob-uuid@example.test".into(), "bobdev01".into())),
        "the persisted row carries the leaf's user + device, not the envelope's"
    );
    assert_eq!(
        recorder.seen(),
        vec![(
            "bob-uuid@example.test".to_string(),
            Some("bobdev01".to_string()),
            "m-forged".to_string()
        )],
        "the EventHandler sees the MLS sender"
    );
    let room = client.room(&room_id()).await.expect("room cached");
    let items = room
        .messages(&kupferdraht::timeline::MessagesOptions {
            limit: None,
            before: None,
        })
        .await
        .expect("messages");
    let item = items
        .iter()
        .find(|i| i.item_id == "m-forged")
        .expect("timeline item");
    assert_eq!(item.sender_user_id, "bob-uuid@example.test");
    assert_eq!(item.sender_device_id, "bobdev01");
}

/// An envelope without `qualified_from` / `from_client` no longer
/// degrades to `unknown@unknown` — the leaf names the sender.
#[tokio::test]
async fn missing_envelope_sender_still_attributes_the_mls_sender() {
    let server = MockServer::start();
    let (client, store) = logged_in_client(&server).await;
    mount_room(&server, "wire_member", "wire_member");
    let mut peer = common::mls::PeerGroup::new(GROUP_ID, BOB_IDENTITY);
    mount_welcome_and_messages(
        &server,
        &mut peer,
        &[(text_generic_message("m-anon", "hello"), None)],
    );

    sync(&client).await;

    assert_eq!(
        row_sender(&store, "m-anon").await,
        Some(("bob-uuid@example.test".into(), "bobdev01".into()))
    );
}

/// ADR-0027 d.1: a mismatch is dispatched under the MLS sender AND
/// recorded as a `warn!` naming both values.
#[tokio::test]
async fn envelope_mismatch_is_logged_as_a_warning() {
    let events = captured_events();
    let server = MockServer::start();
    let (client, _store) = logged_in_client(&server).await;
    mount_room(&server, "wire_member", "wire_member");
    let mut peer = common::mls::PeerGroup::new(GROUP_ID, BOB_IDENTITY);
    mount_welcome_and_messages(
        &server,
        &mut peer,
        &[(
            text_generic_message("m-mismatch", "hello"),
            Some((CAROL, "carolphone")),
        )],
    );

    sync(&client).await;

    let mismatch: Vec<String> = warns(&events)
        .into_iter()
        .filter(|w| w.contains("m-mismatch") && w.contains("carol-uuid") && w.contains("bob-uuid"))
        .collect();
    assert_eq!(
        mismatch.len(),
        1,
        "one warn naming the envelope and the MLS sender: {:?}",
        warns(&events)
    );
}

/// The matching case stays quiet — the mismatch warn is a signal, not
/// noise on every message.
#[tokio::test]
async fn matching_envelope_logs_no_mismatch_warning() {
    let events = captured_events();
    let server = MockServer::start();
    let (client, store) = logged_in_client(&server).await;
    mount_room(&server, "wire_member", "wire_member");
    let mut peer = common::mls::PeerGroup::new(GROUP_ID, BOB_IDENTITY);
    mount_welcome_and_messages(
        &server,
        &mut peer,
        &[(
            text_generic_message("m-honest", "hello"),
            Some((BOB, "bobdev01")),
        )],
    );

    sync(&client).await;

    assert!(
        row_sender(&store, "m-honest").await.is_some(),
        "the honest message is dispatched"
    );
    assert!(
        !warns(&events)
            .iter()
            .any(|w| w.contains("m-honest") && w.contains("mismatch")),
        "no mismatch warn for an honest envelope: {:?}",
        warns(&events)
    );
}

/// ADR-0018's case gate is specified on the MLS-authenticated sender:
/// bob (member) signs, the envelope claims carol (admin) — no fold.
#[tokio::test]
async fn case_gate_uses_the_mls_sender_not_the_envelope() {
    let server = MockServer::start();
    let (client, store) = logged_in_client(&server).await;
    mount_room(&server, "wire_member", "wire_admin");
    let mut peer = common::mls::PeerGroup::new(GROUP_ID, BOB_IDENTITY);
    mount_welcome_and_messages(
        &server,
        &mut peer,
        &[(
            case_generic_message("case-forged", "hijack"),
            Some((CAROL, "carolphone")),
        )],
    );

    sync(&client).await;

    assert_eq!(
        row_sender(&store, "case-forged")
            .await
            .map(|(user, _)| user),
        Some("bob-uuid@example.test".into()),
        "the case row is stored under the MLS sender"
    );
    let room = client.room(&room_id()).await.expect("room cached");
    assert!(
        room.case().await.expect("case read").is_none(),
        "a member's snapshot must not fold behind an admin's envelope"
    );
}

/// A task's creator (the `delegated_tasks` key) is the MLS sender.
#[tokio::test]
async fn task_creator_is_the_mls_sender() {
    let server = MockServer::start();
    let (client, _store) = logged_in_client(&server).await;
    mount_room(&server, "wire_member", "wire_member");
    let mut peer = common::mls::PeerGroup::new(GROUP_ID, BOB_IDENTITY);
    mount_welcome_and_messages(
        &server,
        &mut peer,
        &[(
            task_generic_message("task-m1", "95a00000-0000-4000-8000-000000000001", "Pay"),
            Some((CAROL, "carolphone")),
        )],
    );

    sync(&client).await;

    let room = client.room(&room_id()).await.expect("room cached");
    let tasks = room.tasks().await.expect("tasks");
    assert_eq!(tasks.len(), 1, "the task folded");
    assert_eq!(tasks[0].creator_user_id, "bob-uuid@example.test");
}

/// ADR-0027 d.2 — preservation pin: a message signed by an X.509 leaf
/// keeps today's envelope attribution (no regression for mixed e2ei
/// teams) until the X.509 follow-up step.
#[tokio::test]
async fn x509_sender_keeps_the_envelope_attribution() {
    let server = MockServer::start();
    let (client, store) = logged_in_client(&server).await;
    mount_room(&server, "wire_member", "wire_member");
    let x509 = openmls::prelude::Credential::new(
        openmls::prelude::CredentialType::X509,
        b"fixture certificate chain".to_vec(),
    );
    let mut peer = common::mls::PeerGroup::new_with_credential(GROUP_ID, x509);
    mount_welcome_and_messages(
        &server,
        &mut peer,
        &[(
            text_generic_message("m-x509", "hello"),
            Some((BOB, "bobx509dev")),
        )],
    );

    sync(&client).await;

    assert_eq!(
        row_sender(&store, "m-x509").await,
        Some(("bob-uuid@example.test".into(), "bobx509dev".into())),
        "an X.509 sender is attributed by the envelope, as before"
    );
}

/// ADR-0027 d.1: a Basic credential that is not W-4 is malformed — the
/// message is dropped with a `warn!`, the sync itself succeeds.
#[tokio::test]
async fn malformed_basic_credential_drops_the_message() {
    let events = captured_events();
    let server = MockServer::start();
    let (client, store) = logged_in_client(&server).await;
    mount_room(&server, "wire_member", "wire_member");
    let mut peer = common::mls::PeerGroup::new(GROUP_ID, b"not-a-wire-identity");
    mount_welcome_and_messages(
        &server,
        &mut peer,
        &[(
            text_generic_message("m-malformed", "hello"),
            Some((BOB, "bobdev01")),
        )],
    );

    sync(&client).await;

    assert_eq!(
        row_sender(&store, "m-malformed").await,
        None,
        "a malformed leaf identity is never attributed"
    );
    assert!(
        warns(&events)
            .iter()
            .any(|w| w.contains("m-malformed") && w.contains("credential")),
        "the drop is logged naming the message and the credential: {:?}",
        warns(&events)
    );
}
