// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 95b — inbound edits and deletes are bound to the original
//! sender, with Wire's delete exceptions (ADR-0028). TDD red.
//!
//! One MLS group with four fixture members besides the client: bob
//! (the creator, `PeerGroup`), a second device of bob, carol, and a
//! second device of the client's own user (`PeerDevice`s kept at the
//! group's epoch). The backend is honest — every envelope names the
//! real sender; the attack is a member editing or deleting someone
//! else's message, which today applies by message id alone.
//!
//! `wire::proto` is crate-private (ADR-0009), so the test hand-emits
//! the protobuf bytes (the Step-72b precedent).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use httpmock::Method::{GET, POST};
use httpmock::{Mock, MockServer, prelude::HttpMockRequest};
use kupferdraht::storage::Store;
use kupferdraht::timeline::{
    MessagesOptions, TimelineDiff, TimelineItem, TimelineItemKind, TimelineListener,
};
use kupferdraht::{ClientBuilder, SyncSettings};

mod common;

use common::mls::{PeerDevice, PeerGroup};

const DOMAIN: &str = "example.test";
const ROOM_UUID: &str = "95b0aa31-77aa-4bde-9d10-223344556677";
/// b64("grp-95b") — the room's MLS group id.
const GROUP_ID_B64: &str = "Z3JwLTk1Yg==";
const GROUP_ID: &[u8] = b"grp-95b";
const OTHER_ROOM_UUID: &str = "95b0bb31-77aa-4bde-9d10-223344556677";
/// b64("grp-95b-other").
const OTHER_GROUP_ID_B64: &str = "Z3JwLTk1Yi1vdGhlcg==";
const OTHER_GROUP_ID: &[u8] = b"grp-95b-other";
const SELF: &str = "user-uuid";
const BOB: &str = "bob-uuid";
const CAROL: &str = "carol-uuid";

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

fn text_payload(content: &str) -> Vec<u8> {
    let mut text = Vec::new();
    put_len_delimited(1, content.as_bytes(), &mut text);
    text
}

/// `GenericMessage { message_id = 1, text = 2 }`.
fn text_message(message_id: &str, content: &str) -> Vec<u8> {
    let mut m = Vec::new();
    put_len_delimited(1, message_id.as_bytes(), &mut m);
    put_len_delimited(2, &text_payload(content), &mut m);
    m
}

/// `GenericMessage { message_id = 1, edited = 15 { replacing = 1, text = 2 } }`.
fn edit_message(message_id: &str, replacing: &str, content: &str) -> Vec<u8> {
    let mut edit = Vec::new();
    put_len_delimited(1, replacing.as_bytes(), &mut edit);
    put_len_delimited(2, &text_payload(content), &mut edit);
    let mut m = Vec::new();
    put_len_delimited(1, message_id.as_bytes(), &mut m);
    put_len_delimited(15, &edit, &mut m);
    m
}

/// `GenericMessage { message_id = 1, deleted = 14 { message_id = 1 } }`.
fn delete_message(message_id: &str, target: &str) -> Vec<u8> {
    let mut delete = Vec::new();
    put_len_delimited(1, target.as_bytes(), &mut delete);
    let mut m = Vec::new();
    put_len_delimited(1, message_id.as_bytes(), &mut m);
    put_len_delimited(14, &delete, &mut m);
    m
}

/// `GenericMessage { message_id = 1, ephemeral = 18 { expire_after_millis = 1, text = 2 } }`.
fn ephemeral_text_message(message_id: &str, content: &str, ttl_ms: u64) -> Vec<u8> {
    let mut ephemeral = Vec::new();
    put_uint(1, ttl_ms, &mut ephemeral);
    put_len_delimited(2, &text_payload(content), &mut ephemeral);
    let mut m = Vec::new();
    put_len_delimited(1, message_id.as_bytes(), &mut m);
    put_len_delimited(18, &ephemeral, &mut m);
    m
}

/// `GenericMessage { message_id = 1, task = 262000 }` with the minimal
/// `TaskPayload { task_id = 1, title = 2, status = 3 }` (mode a).
fn task_message(message_id: &str, task_id: &str, title: &str) -> Vec<u8> {
    let mut payload = Vec::new();
    put_len_delimited(1, task_id.as_bytes(), &mut payload);
    put_len_delimited(2, title.as_bytes(), &mut payload);
    put_uint(3, 0 /* OPEN */, &mut payload);
    let mut m = Vec::new();
    put_len_delimited(1, message_id.as_bytes(), &mut m);
    put_len_delimited(262_000, &payload, &mut m);
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

/// The client's uploaded key packages, most recent upload per host
/// (httpmock pools servers — LEARNINGS §5.13); `n` picks one so two
/// groups never consume the same package.
fn captured_key_package_for(server: &MockServer, n: usize) -> Vec<u8> {
    let host = server.address().to_string();
    let guard = UPLOADED_KPS.lock().expect("capture lock");
    let (_, kps) = guard
        .iter()
        .rev()
        .find(|(h, _)| *h == host)
        .expect("register_device uploaded key packages");
    base64::engine::general_purpose::STANDARD
        .decode(kps.get(n).expect("enough key packages uploaded"))
        .expect("uploaded key package is base64")
}

fn conversation_json(room_uuid: &str, group_b64: &str) -> String {
    format!(
        r#"{{"qualified_id":{{"id":"{room_uuid}","domain":"{DOMAIN}"}},
             "type":0,"protocol":"mls","epoch":1,"group_id":"{group_b64}",
             "name":"Edit/delete auth",
             "members":{{
               "self":{{"qualified_id":{{"id":"{SELF}","domain":"{DOMAIN}"}},
                       "conversation_role":"wire_member"}},
               "others":[
                 {{"qualified_id":{{"id":"{BOB}","domain":"{DOMAIN}"}},
                   "conversation_role":"wire_member"}},
                 {{"qualified_id":{{"id":"{CAROL}","domain":"{DOMAIN}"}},
                   "conversation_role":"wire_member"}}
               ]
             }}}}"#
    )
}

/// Room metadata for the given `(room uuid, group id b64)` rooms.
fn mount_rooms(server: &MockServer, rooms: &[(&str, &str)]) {
    let ids: Vec<String> = rooms
        .iter()
        .map(|(uuid, _)| format!(r#"{{"id":"{uuid}","domain":"{DOMAIN}"}}"#))
        .collect();
    let ids_body = format!(
        r#"{{"qualified_conversations":[{}],"has_more":false,"paging_state":"END"}}"#,
        ids.join(",")
    );
    server.mock(move |when, then| {
        when.method(POST).path("/v14/conversations/list-ids");
        then.status(200)
            .header("content-type", "application/json")
            .body(ids_body.clone());
    });
    let found: Vec<String> = rooms
        .iter()
        .map(|(uuid, group)| conversation_json(uuid, group))
        .collect();
    let list_body = format!(
        r#"{{"found":[{}], "not_found":[], "failed":[]}}"#,
        found.join(",")
    );
    server.mock(move |when, then| {
        when.method(POST).path("/v14/conversations/list");
        then.status(200)
            .header("content-type", "application/json")
            .body(list_body.clone());
    });
    server.mock(|when, then| {
        when.method(POST).path("/v14/list-users");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"found":[]}"#);
    });
}

/// One notifications page (`notif_id` distinct per page so the
/// cursor-equality dedup never swallows a later page).
fn mount_notifications_page<'a>(
    server: &'a MockServer,
    notif_id: &str,
    events: &[String],
) -> Mock<'a> {
    let body = format!(
        r#"{{"notifications":[{{"id":"{notif_id}","payload":[{}]}}],"has_more":false}}"#,
        events.join(",")
    );
    server.mock(move |when, then| {
        when.method(GET).path("/v14/notifications");
        then.status(200)
            .header("content-type", "application/json")
            .body(body.clone());
    })
}

fn welcome_event(room_uuid: &str, data: &[u8]) -> String {
    format!(
        r#"{{"type":"conversation.mls-welcome",
             "qualified_conversation":{{"id":"{room_uuid}","domain":"{DOMAIN}"}},
             "qualified_from":{{"id":"{BOB}","domain":"{DOMAIN}"}},
             "data":"{}",
             "time":"2026-09-26T10:00:00.000Z"}}"#,
        base64::engine::general_purpose::STANDARD.encode(data)
    )
}

/// An honest envelope: `sender` is the user whose device signed `data`.
fn message_event(room_uuid: &str, sender: &str, data: &[u8]) -> String {
    format!(
        r#"{{"type":"conversation.mls-message-add",
             "qualified_conversation":{{"id":"{room_uuid}","domain":"{DOMAIN}"}},
             "qualified_from":{{"id":"{sender}","domain":"{DOMAIN}"}},
             "data":"{}",
             "time":"2026-09-26T10:00:01.000Z"}}"#,
        base64::engine::general_purpose::STANDARD.encode(data)
    )
}

fn mount_message_send(server: &MockServer) -> Mock<'_> {
    server.mock(|when, then| {
        when.method(POST).path("/v14/mls/messages");
        then.status(201)
            .header("content-type", "application/json")
            .body(r#"{"events":[],"time":"2026-09-26T10:00:02.000Z"}"#);
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

// ---- the group ----

/// bob (creator) + a second device of bob + carol + a second device of
/// the client's own user, all joined before the client.
struct Peers {
    bob: PeerGroup,
    bob2: PeerDevice,
    carol: PeerDevice,
    self2: PeerDevice,
}

impl Peers {
    fn new() -> Self {
        let mut bob = PeerGroup::new(GROUP_ID, b"bob-uuid:bobdev01@example.test");
        let mut joined: Vec<PeerDevice> = Vec::new();
        for identity in [
            &b"bob-uuid:bobdev02@example.test"[..],
            b"carol-uuid:caroldev@example.test",
            b"user-uuid:selfdev2@example.test",
        ] {
            let mut device = PeerDevice::new(identity);
            let (commit, welcome) = bob.add_member(&device.key_package_tls());
            for member in &mut joined {
                member.process_commit(&commit);
            }
            device.join_via_welcome(&welcome);
            joined.push(device);
        }
        let self2 = joined.pop().expect("self2");
        let carol = joined.pop().expect("carol");
        let bob2 = joined.pop().expect("bob2");
        Self {
            bob,
            bob2,
            carol,
            self2,
        }
    }

    /// Add the client (its uploaded key package) and return the
    /// Welcome event; every peer device merges the Add commit.
    fn welcome_client(&mut self, kp: &[u8]) -> String {
        let (commit, welcome) = self.bob.add_member(kp);
        for device in [&mut self.bob2, &mut self.carol, &mut self.self2] {
            device.process_commit(&commit);
        }
        welcome_event(ROOM_UUID, &welcome)
    }

    fn sent_by_bob(&mut self, plaintext: &[u8]) -> String {
        message_event(ROOM_UUID, BOB, &self.bob.application_message(plaintext))
    }

    fn sent_by_bob2(&mut self, plaintext: &[u8]) -> String {
        message_event(ROOM_UUID, BOB, &self.bob2.application_message(plaintext))
    }

    fn sent_by_carol(&mut self, plaintext: &[u8]) -> String {
        message_event(ROOM_UUID, CAROL, &self.carol.application_message(plaintext))
    }

    fn sent_by_self2(&mut self, plaintext: &[u8]) -> String {
        message_event(ROOM_UUID, SELF, &self.self2.application_message(plaintext))
    }
}

/// Records every `TimelineDiff` the client emits.
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
    /// The `UpdateAt` diffs that carried `message_id`.
    fn updates_for(&self, message_id: &str) -> usize {
        self.diffs
            .lock()
            .expect("recorder lock")
            .iter()
            .filter(
                |d| matches!(d, TimelineDiff::UpdateAt { item, .. } if item.item_id == message_id),
            )
            .count()
    }
}

/// Login + one room + the Welcome, followed by `events` built from the
/// peers, in one notifications page; one `sync_once`.
async fn sync_scenario(
    events: impl FnOnce(&mut Peers) -> Vec<String>,
) -> (MockServer, Arc<kupferdraht::Client>, Arc<dyn Store>, Peers) {
    let (server, client, store, peers, _recorder) = sync_scenario_recorded(events).await;
    (server, client, store, peers)
}

/// [`sync_scenario`] with a timeline listener subscribed before the sync.
async fn sync_scenario_recorded(
    events: impl FnOnce(&mut Peers) -> Vec<String>,
) -> (
    MockServer,
    Arc<kupferdraht::Client>,
    Arc<dyn Store>,
    Peers,
    Arc<DiffRecorder>,
) {
    let server = MockServer::start();
    let (client, store) = logged_in_client(&server).await;
    let recorder = Arc::new(DiffRecorder::default());
    let subscription = client.add_timeline_listener(recorder.clone());
    mount_rooms(&server, &[(ROOM_UUID, GROUP_ID_B64)]);
    let mut peers = Peers::new();
    let mut page = vec![peers.welcome_client(&captured_key_package_for(&server, 0))];
    page.extend(events(&mut peers));
    let _ = mount_notifications_page(&server, "95b0e858-8eae-11ef-0000-000000000001", &page);
    client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync_once");
    drop(subscription);
    (server, client, store, peers, recorder)
}

async fn items(client: &kupferdraht::Client) -> Vec<TimelineItem> {
    client
        .room(&room_id())
        .await
        .expect("room cached")
        .messages(&MessagesOptions {
            limit: None,
            before: None,
        })
        .await
        .expect("messages")
}

async fn item(client: &kupferdraht::Client, message_id: &str) -> TimelineItem {
    items(client)
        .await
        .into_iter()
        .find(|i| i.item_id == message_id)
        .unwrap_or_else(|| panic!("timeline item {message_id}"))
}

fn body(item: &TimelineItem) -> &str {
    match &item.kind {
        TimelineItemKind::Text { body, .. } => body,
        other => panic!("expected a text item, got {other:?}"),
    }
}

// ---- tests ----

/// ADR-0028 d.1: carol cannot rewrite bob's message.
#[tokio::test]
async fn a_members_edit_of_another_members_message_is_ignored() {
    let (_server, client, _store, _peers, recorder) = sync_scenario_recorded(|p| {
        vec![
            p.sent_by_bob(&text_message("m-bob", "original")),
            p.sent_by_carol(&edit_message("e-carol", "m-bob", "hijacked")),
        ]
    })
    .await;

    let original = item(&client, "m-bob").await;
    assert_eq!(body(&original), "original", "carol's edit must not apply");
    assert_eq!(original.edited_at_ms, None);
    assert_eq!(
        recorder.updates_for("m-bob"),
        0,
        "a rejected edit emits no UpdateAt diff"
    );
}

/// ADR-0028 d.2 (a): carol cannot delete bob's message.
#[tokio::test]
async fn a_members_delete_of_another_members_message_is_ignored() {
    let (_server, client, _store, _peers, recorder) = sync_scenario_recorded(|p| {
        vec![
            p.sent_by_bob(&text_message("m-bob", "keep me")),
            p.sent_by_carol(&delete_message("d-carol", "m-bob")),
        ]
    })
    .await;

    let original = item(&client, "m-bob").await;
    assert!(!original.redacted, "carol's delete must not apply");
    assert_eq!(body(&original), "keep me");
    assert_eq!(
        recorder.updates_for("m-bob"),
        0,
        "a rejected delete emits no UpdateAt diff"
    );
}

/// ADR-0028 d.1 + d.2 (a) — preservation pin: the original sender's
/// other device edits and deletes like the first one.
#[tokio::test]
async fn the_original_sender_may_edit_and_delete_from_another_device() {
    let (_server, client, _store, _peers, recorder) = sync_scenario_recorded(|p| {
        vec![
            p.sent_by_bob(&text_message("m-bob-1", "typo")),
            p.sent_by_bob(&text_message("m-bob-2", "oops")),
            p.sent_by_bob2(&edit_message("e-bob2", "m-bob-1", "fixed")),
            p.sent_by_bob2(&delete_message("d-bob2", "m-bob-2")),
        ]
    })
    .await;

    assert_eq!(body(&item(&client, "m-bob-1").await), "fixed");
    assert!(item(&client, "m-bob-2").await.redacted);
    // The counter-pin to the rejection tests: an applied edit / delete
    // does emit its UpdateAt, so "no UpdateAt" there is not vacuous.
    assert!(
        recorder.updates_for("m-bob-1") >= 1,
        "the applied edit is emitted"
    );
    assert!(
        recorder.updates_for("m-bob-2") >= 1,
        "the applied delete is emitted"
    );
}

/// ADR-0028 d.2 (b), as revised 2026-09-28: another own device may
/// delete a foreign **ephemeral** message — the own-device sync of a
/// receiver-side expiry (observations §74).
#[tokio::test]
async fn a_delete_from_another_own_device_applies_to_a_foreign_ephemeral() {
    let (_server, client, _store, _peers) = sync_scenario(|p| {
        vec![
            p.sent_by_bob(&ephemeral_text_message("m-bob-eph", "expires", 60_000)),
            p.sent_by_self2(&delete_message("d-self2", "m-bob-eph")),
        ]
    })
    .await;

    assert!(item(&client, "m-bob-eph").await.redacted);
}

/// ADR-0028 d.2 (b), as revised 2026-09-28: a delete from another own
/// device of a foreign **ordinary** message is ignored — the official
/// webapp does not apply it either (observations §91).
#[tokio::test]
async fn a_delete_from_another_own_device_of_a_foreign_message_is_ignored() {
    let (_server, client, _store, _peers) = sync_scenario(|p| {
        vec![
            p.sent_by_bob(&text_message("m-bob", "seen elsewhere")),
            p.sent_by_self2(&delete_message("d-self2", "m-bob")),
        ]
    })
    .await;

    let original = item(&client, "m-bob").await;
    assert!(
        !original.redacted,
        "a self-sent delete of bob's message must not apply"
    );
    assert_eq!(body(&original), "seen elsewhere");
}

/// ADR-0028 d.2 (c) — preservation pin: any member may delete the
/// client's own ephemeral message (the receiver enforces the sender's
/// timer, observations §74 / Step 89).
#[tokio::test]
async fn a_member_may_delete_our_own_ephemeral_message() {
    let server = MockServer::start();
    let (client, _store) = logged_in_client(&server).await;
    mount_rooms(&server, &[(ROOM_UUID, GROUP_ID_B64)]);
    let mut peers = Peers::new();
    let mut first = mount_notifications_page(
        &server,
        "95b0e858-8eae-11ef-0000-000000000002",
        &[peers.welcome_client(&captured_key_package_for(&server, 0))],
    );
    client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync_once (welcome)");
    let _send = mount_message_send(&server);
    let handle = client
        .room(&room_id())
        .await
        .expect("room cached")
        .timeline()
        .expect("timeline")
        .send_ephemeral_text("own secret".into(), Duration::from_secs(3_600))
        .await
        .expect("send ephemeral text");

    first.delete();
    let _second = mount_notifications_page(
        &server,
        "95b0e858-8eae-11ef-0000-000000000003",
        &[peers.sent_by_carol(&delete_message("d-carol", &handle.message_id))],
    );
    client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync_once (receiver delete)");

    assert!(
        item(&client, &handle.message_id).await.redacted,
        "a receiver's delete of our own ephemeral message applies"
    );
}

/// ADR-0028 d.2: a third party cannot hasten someone else's ephemeral
/// message — carol's delete of bob's ephemeral is rejected.
#[tokio::test]
async fn a_third_partys_delete_of_someone_elses_ephemeral_is_rejected() {
    let (_server, client, _store, _peers) = sync_scenario(|p| {
        vec![
            p.sent_by_bob(&ephemeral_text_message(
                "eph-bob",
                "self-destructs in an hour",
                3_600_000,
            )),
            p.sent_by_carol(&delete_message("d-carol", "eph-bob")),
        ]
    })
    .await;

    assert!(
        !item(&client, "eph-bob").await.redacted,
        "each receiver runs its own timer — carol cannot expire bob's message here"
    );
}

/// ADR-0028: the carrier-delete interaction of ADR-0014 is reachable
/// only by the carrier's sender — carol cannot remove bob's task.
#[tokio::test]
async fn a_members_delete_of_another_members_task_carrier_keeps_the_task() {
    let (_server, client, _store, _peers) = sync_scenario(|p| {
        vec![
            p.sent_by_bob(&task_message(
                "task-bob",
                "95b00000-0000-4000-8000-000000000001",
                "Pay invoice",
            )),
            p.sent_by_carol(&delete_message("d-carol", "task-bob")),
        ]
    })
    .await;

    let room = client.room(&room_id()).await.expect("room cached");
    assert_eq!(
        room.tasks().await.expect("tasks").len(),
        1,
        "bob's task survives carol's delete"
    );
}

/// ADR-0028 (same room) — preservation pin: bob's edit sent in another
/// room never reaches a message of this room.
#[tokio::test]
async fn an_edit_naming_a_message_of_another_room_is_ignored() {
    let server = MockServer::start();
    let (client, _store) = logged_in_client(&server).await;
    mount_rooms(
        &server,
        &[
            (ROOM_UUID, GROUP_ID_B64),
            (OTHER_ROOM_UUID, OTHER_GROUP_ID_B64),
        ],
    );
    let mut peers = Peers::new();
    let mut other = PeerGroup::new(OTHER_GROUP_ID, b"bob-uuid:bobdev01@example.test");
    let welcome_here = peers.welcome_client(&captured_key_package_for(&server, 0));
    let (_commit, welcome_other) = other.add_member(&captured_key_package_for(&server, 1));
    let events = vec![
        welcome_here,
        welcome_event(OTHER_ROOM_UUID, &welcome_other),
        peers.sent_by_bob(&text_message("m-here", "stays")),
        message_event(
            OTHER_ROOM_UUID,
            BOB,
            &other.application_message(&edit_message("e-other", "m-here", "cross-room")),
        ),
    ];
    let _ = mount_notifications_page(&server, "95b0e858-8eae-11ef-0000-000000000004", &events);
    client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync_once");

    assert_eq!(body(&item(&client, "m-here").await), "stays");
}

// ---- the sending side: the client's own API ----

/// ADR-0028 applied to the client's own API: `Timeline::edit` of a
/// message another user sent is refused before anything is uploaded —
/// every receiver would reject it, and the local row must not diverge
/// from theirs.
#[tokio::test]
async fn timeline_edit_of_another_users_message_is_refused() {
    let (server, client, _store, _peers) =
        sync_scenario(|p| vec![p.sent_by_bob(&text_message("m-bob", "not yours"))]).await;
    let send = mount_message_send(&server);

    let result = client
        .room(&room_id())
        .await
        .expect("room cached")
        .timeline()
        .expect("timeline")
        .edit("m-bob".into(), "overwritten".into())
        .await;

    assert!(
        matches!(result, Err(kupferdraht::KupferdrahtError::InvalidInput(_))),
        "editing someone else's message is refused: {result:?}"
    );
    assert_eq!(send.calls(), 0, "nothing is uploaded");
    assert_eq!(body(&item(&client, "m-bob").await), "not yours");
}

/// Same for `Timeline::redact`.
#[tokio::test]
async fn timeline_redact_of_another_users_message_is_refused() {
    let (server, client, _store, _peers) =
        sync_scenario(|p| vec![p.sent_by_bob(&text_message("m-bob", "not yours"))]).await;
    let send = mount_message_send(&server);

    let result = client
        .room(&room_id())
        .await
        .expect("room cached")
        .timeline()
        .expect("timeline")
        .redact("m-bob")
        .await;

    assert!(
        matches!(result, Err(kupferdraht::KupferdrahtError::InvalidInput(_))),
        "deleting someone else's message for everyone is refused: {result:?}"
    );
    assert_eq!(send.calls(), 0, "nothing is uploaded");
    assert!(!item(&client, "m-bob").await.redacted);
}

/// Preservation pin: the client's own messages — including one its
/// other device sent — stay editable and deletable.
#[tokio::test]
async fn timeline_edit_and_redact_of_own_messages_still_work() {
    let (server, client, _store, _peers) =
        sync_scenario(|p| vec![p.sent_by_self2(&text_message("m-self2", "from my other device"))])
            .await;
    let send = mount_message_send(&server);
    let timeline = client
        .room(&room_id())
        .await
        .expect("room cached")
        .timeline()
        .expect("timeline");

    let own = timeline
        .send_text("mine".into())
        .await
        .expect("send own text");
    timeline
        .edit(own.message_id.clone(), "mine, edited".into())
        .await
        .expect("edit own message");
    timeline
        .redact("m-self2")
        .await
        .expect("delete a message of my other device");

    assert_eq!(send.calls(), 3, "send + edit + delete went out");
    assert_eq!(body(&item(&client, &own.message_id).await), "mine, edited");
    assert!(item(&client, "m-self2").await.redacted);
}

/// Step 105 (D3): an edit of an own edit — `replacing` is the first
/// edit's id — updates the row like the first edit did. The row's stable
/// id (the key of its mentions and of the `UpdateAt`) is resolved before
/// the edit advances the chain pointer; resolved after, the old link no
/// longer matches and the second edit's mentions and update were lost.
#[tokio::test]
async fn an_edit_of_an_own_edit_updates_the_row() {
    let (server, client, _store, _peers) = sync_scenario(|_| Vec::new()).await;
    let _send = mount_message_send(&server);
    let recorder = Arc::new(DiffRecorder::default());
    let _subscription = client.add_timeline_listener(recorder.clone());
    let timeline = client
        .room(&room_id())
        .await
        .expect("room cached")
        .timeline()
        .expect("timeline");

    let original = timeline.send_text("v0".into()).await.expect("send");
    let first = timeline
        .edit(original.message_id.clone(), "v1".into())
        .await
        .expect("first edit");
    let before = recorder.updates_for(&original.message_id);
    timeline
        .edit(first.message_id, "v2".into())
        .await
        .expect("edit of the edit");

    assert_eq!(body(&item(&client, &original.message_id).await), "v2");
    assert_eq!(
        recorder.updates_for(&original.message_id),
        before + 1,
        "the edit of the edit surfaces as an update of the row"
    );
}
