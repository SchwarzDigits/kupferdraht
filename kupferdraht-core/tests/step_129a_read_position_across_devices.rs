// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 129a — the read position across devices (TDD red): a read sends
//! Wire's `LastRead` into the personal room, a `LastRead` from the account's
//! other device moves this device's marker.
//!
//! The second device of the account is real (`common::mls::PeerDevice`, the
//! Step 86 fixture): the client's establishing commit Adds it, it decrypts
//! what the client sends into the personal room and encrypts what it sends
//! there itself. Room A is a group the client external-joins, with Bob's
//! messages seeded as rows; its read marker is the subject.

use std::sync::{Arc, Mutex};

use httpmock::Method::{GET, POST};
use httpmock::{Mock, MockServer, prelude::HttpMockRequest};
use kupferdraht::storage::{Store, TimelineEventRecord};
use kupferdraht::{Client, ReceiptType, Room, SyncSettings};
use openmls::prelude::WireFormat;

mod common;

use common::drain_fixture::{Notif, b64, mount_page, put_len_delimited, put_varint};
use common::mls::PeerDevice;

const DOMAIN: &str = "example.test";
const OWN_USER_UUID: &str = "user-uuid";
/// The (server-derived) id of the account's personal room.
const SELF_UUID: &str = "5f98a472-6364-50da-bf25-b77b0b2638e3";
/// b64("grp-self") — the personal room's MLS group id.
const SELF_GROUP_ID_B64: &str = "Z3JwLXNlbGY=";
const SELF_GROUP_ID: &[u8] = b"grp-self";
/// Room A: a group with Bob.
const ROOM_A_UUID: &str = "4b1ff12e-52a2-4b58-9bf6-1b40e64ae7a2";
/// b64("grp-grp").
const ROOM_A_GROUP_ID_B64: &str = "Z3JwLWdycA==";
const ROOM_A_GROUP_ID: &[u8] = b"grp-grp";
/// Leaf identity of the account's *other* device.
const OWN_OTHER_DEVICE: &[u8] = b"user-uuid:dev2@example.test";
const BOB_IDENTITY: &[u8] = b"bob-uuid:bobdev01@example.test";
const BOB: &str = "bob-uuid@example.test";

static COMMIT_BUNDLES: Mutex<Vec<(String, Vec<u8>)>> = Mutex::new(Vec::new());
static MLS_MESSAGES: Mutex<Vec<(String, Vec<u8>)>> = Mutex::new(Vec::new());

fn personal_room_id() -> String {
    format!("{SELF_UUID}@{DOMAIN}")
}

fn room_a_id() -> String {
    format!("{ROOM_A_UUID}@{DOMAIN}")
}

// ── captures ──────────────────────────────────────────────────────────

fn host_of(req: &HttpMockRequest) -> String {
    req.headers()
        .get("host")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
        .unwrap_or_default()
}

fn capture_commit_bundle(req: &HttpMockRequest) -> bool {
    COMMIT_BUNDLES
        .lock()
        .expect("capture lock")
        .push((host_of(req), req.body_ref().to_vec()));
    true
}

fn capture_mls_message(req: &HttpMockRequest) -> bool {
    MLS_MESSAGES
        .lock()
        .expect("capture lock")
        .push((host_of(req), req.body_ref().to_vec()));
    true
}

/// Every body captured for this server (httpmock pools servers — filter by
/// the Host header, LEARNINGS §5.13).
fn captures_for(store: &Mutex<Vec<(String, Vec<u8>)>>, server: &MockServer) -> Vec<Vec<u8>> {
    let host = server.address().to_string();
    store
        .lock()
        .expect("capture lock")
        .iter()
        .filter(|(h, _)| *h == host)
        .map(|(_, body)| body.clone())
        .collect()
}

/// The uploads addressed to a group since `start` (the captures a test on a
/// reused address left behind stay in the static): an MLS `PrivateMessage`
/// carries its `group_id` in the clear.
fn uploads_for_group(server: &MockServer, start: usize, group_id: &[u8]) -> Vec<Vec<u8>> {
    captures_for(&MLS_MESSAGES, server)
        .into_iter()
        .skip(start)
        .filter(|body| body.windows(group_id.len()).any(|w| w == group_id))
        .collect()
}

/// How many uploads this server has seen so far — the start of a test's own.
fn uploads_so_far(server: &MockServer) -> usize {
    captures_for(&MLS_MESSAGES, server).len()
}

// ── a minimal protobuf reader for the decrypted `GenericMessage` ─────────

fn read_varint(bytes: &[u8], pos: &mut usize) -> u64 {
    let mut value = 0u64;
    let mut shift = 0;
    loop {
        let byte = bytes[*pos];
        *pos += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return value;
        }
        shift += 7;
    }
}

/// `(field, varint)` or `(field, bytes)` pairs of one message.
fn fields(bytes: &[u8]) -> Vec<(u32, Option<u64>, Vec<u8>)> {
    let mut pos = 0;
    let mut out = Vec::new();
    while pos < bytes.len() {
        let tag = read_varint(bytes, &mut pos);
        let field = u32::try_from(tag >> 3).expect("field number");
        match tag & 7 {
            0 => {
                let v = read_varint(bytes, &mut pos);
                out.push((field, Some(v), Vec::new()));
            }
            2 => {
                let len = usize::try_from(read_varint(bytes, &mut pos)).expect("length");
                out.push((field, None, bytes[pos..pos + len].to_vec()));
                pos += len;
            }
            other => panic!("unexpected wire type {other} in the fixture's protobuf"),
        }
    }
    out
}

#[derive(Debug, PartialEq, Eq)]
struct DecodedLastRead {
    conversation_id: String,
    qualified: Option<(String, String)>,
    last_read_timestamp: i64,
}

/// The `LastRead` (field 6) of a decrypted `GenericMessage`, `None` for any
/// other content.
fn decode_last_read(plaintext: &[u8]) -> Option<DecodedLastRead> {
    let (_, _, last_read) = fields(plaintext)
        .into_iter()
        .find(|(field, _, _)| *field == 6)?;
    let mut decoded = DecodedLastRead {
        conversation_id: String::new(),
        qualified: None,
        last_read_timestamp: 0,
    };
    for (field, varint, bytes) in fields(&last_read) {
        match field {
            1 => decoded.conversation_id = String::from_utf8(bytes).expect("utf-8"),
            2 => {
                decoded.last_read_timestamp = i64::try_from(varint.expect("varint")).expect("i64");
            }
            3 => {
                let mut id = String::new();
                let mut domain = String::new();
                for (f, _, b) in fields(&bytes) {
                    let text = String::from_utf8(b).expect("utf-8");
                    if f == 1 {
                        id = text;
                    } else if f == 2 {
                        domain = text;
                    }
                }
                decoded.qualified = Some((id, domain));
            }
            _ => {}
        }
    }
    Some(decoded)
}

/// `GenericMessage { message_id = 1, lastRead = 6 { conversation_id = 1,
/// last_read_timestamp = 2, qualified_conversation_id = 3 { id, domain } } }`
/// — what Wire's clients send into the self conversation.
fn last_read_message(message_id: &str, room_uuid: &str, domain: &str, ts: i64) -> Vec<u8> {
    let mut qualified = Vec::new();
    put_len_delimited(1, room_uuid.as_bytes(), &mut qualified);
    put_len_delimited(2, domain.as_bytes(), &mut qualified);
    let mut last_read = Vec::new();
    put_len_delimited(1, room_uuid.as_bytes(), &mut last_read);
    put_varint(2 << 3, &mut last_read);
    put_varint(u64::try_from(ts).expect("a positive time"), &mut last_read);
    put_len_delimited(3, &qualified, &mut last_read);
    let mut m = Vec::new();
    put_len_delimited(1, message_id.as_bytes(), &mut m);
    put_len_delimited(6, &last_read, &mut m);
    m
}

// ── mocks ─────────────────────────────────────────────────────────────

/// Login with the account's `WIRE_RECEIPT_MODE` on or off (the Step 10f
/// gate): off lets a `Read` send no `Confirmation`, so the personal room's
/// `LastRead` is the only upload.
fn mount_login(server: &MockServer, receipts_on: bool) {
    common::mount_full_login_flow(server);
    server.mock(|when, then| {
        when.method(GET).path("/v14/feature-configs");
        then.status(200).body("{}");
    });
    server.mock(|when, then| {
        when.method(GET).path("/v14/properties");
        then.status(200).body(r#"["WIRE_RECEIPT_MODE"]"#);
    });
    server.mock(|when, then| {
        when.method(GET).path("/v14/properties/WIRE_RECEIPT_MODE");
        then.status(200).body(if receipts_on { "1" } else { "0" });
    });
}

fn self_conv_json(epoch: u64) -> String {
    format!(
        r#"{{
            "qualified_id":{{"id":"{SELF_UUID}","domain":"{DOMAIN}"}},
            "type":1,"protocol":"mls","epoch":{epoch},
            "group_id":"{SELF_GROUP_ID_B64}","name":null,
            "members":{{
              "self":{{"qualified_id":{{"id":"{OWN_USER_UUID}","domain":"{DOMAIN}"}}}},
              "others":[]
            }}
        }}"#
    )
}

fn room_a_json(epoch: u64) -> String {
    format!(
        r#"{{
            "qualified_id":{{"id":"{ROOM_A_UUID}","domain":"{DOMAIN}"}},
            "type":0,"protocol":"mls","epoch":{epoch},
            "group_id":"{ROOM_A_GROUP_ID_B64}","name":"A",
            "members":{{
              "self":{{"qualified_id":{{"id":"{OWN_USER_UUID}","domain":"{DOMAIN}"}}}},
              "others":[{{"qualified_id":{{"id":"bob-uuid","domain":"{DOMAIN}"}}}}]
            }}
        }}"#
    )
}

/// The rooms metadata: the personal room at `self_epoch`, room A at epoch 1.
fn mount_rooms(server: &MockServer, self_epoch: u64) {
    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list-ids");
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(
                r#"{{"qualified_conversations":[
                     {{"id":"{SELF_UUID}","domain":"{DOMAIN}"}},
                     {{"id":"{ROOM_A_UUID}","domain":"{DOMAIN}"}}],
                    "has_more":false,"paging_state":"END"}}"#
            ));
    });
    let body = format!(
        r#"{{"found":[{},{}],"not_found":[],"failed":[]}}"#,
        self_conv_json(self_epoch),
        room_a_json(1)
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

/// A harmless first page so the cursor stands at `n1`: Bob types in room A.
fn mount_first_page(server: &MockServer) {
    let typing = format!(
        r#"{{"type":"conversation.typing",
             "qualified_conversation":{{"id":"{ROOM_A_UUID}","domain":"{DOMAIN}"}},
             "qualified_from":{{"id":"bob-uuid","domain":"{DOMAIN}"}},
             "data":{{"status":"started"}},"time":"2026-11-25T10:00:00.000Z"}}"#
    );
    mount_page(
        server,
        None,
        &[Notif {
            id: "n1",
            events: vec![typing],
        }],
    );
}

/// The next page after `n1`: `events` as notification `n2`, then nothing.
fn mount_second_page(server: &MockServer, events: Vec<String>) {
    mount_page(server, Some("n1"), &[Notif { id: "n2", events }]);
    mount_page(server, Some("n2"), &[]);
}

/// `conversation.mls-message-add` in the personal room from the account's
/// other device.
fn personal_room_event(tls: &[u8]) -> String {
    format!(
        r#"{{"type":"conversation.mls-message-add",
             "qualified_conversation":{{"id":"{SELF_UUID}","domain":"{DOMAIN}"}},
             "qualified_from":{{"id":"{OWN_USER_UUID}","domain":"{DOMAIN}"}},
             "from_client":"dev2",
             "data":"{}","time":"2026-11-25T10:00:05.000Z"}}"#,
        b64(tls)
    )
}

fn mount_self_claim<'a>(server: &'a MockServer, key_package_b64: &str) -> Mock<'a> {
    let entry = format!(
        r#"{{"user":"{OWN_USER_UUID}","domain":"{DOMAIN}","client":"dev2",
            "key_package":"{key_package_b64}","key_package_ref":"cmVm"}}"#
    );
    server.mock(move |when, then| {
        when.method(POST).path(format!(
            "/v14/mls/key-packages/claim/{DOMAIN}/{OWN_USER_UUID}"
        ));
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(r#"{{"key_packages":[{entry}]}}"#));
    })
}

fn mount_groupinfo_missing<'a>(server: &'a MockServer, uuid: &str) -> Mock<'a> {
    server.mock(|when, then| {
        when.method(GET)
            .path(format!("/v14/conversations/{DOMAIN}/{uuid}/groupinfo"));
        then.status(404)
            .header("content-type", "application/json")
            .body(
                r#"{"code":404,"label":"mls-missing-group-info","message":"The conversation has no group information"}"#,
            );
    })
}

fn mount_groupinfo<'a>(server: &'a MockServer, uuid: &str, bytes: Vec<u8>) -> Mock<'a> {
    server.mock(move |when, then| {
        when.method(GET)
            .path(format!("/v14/conversations/{DOMAIN}/{uuid}/groupinfo"));
        then.status(200)
            .header("content-type", "message/mls")
            .body(bytes.clone());
    })
}

fn mount_commit_bundle(server: &MockServer) -> Mock<'_> {
    server.mock(|when, then| {
        when.method(POST)
            .path("/v14/mls/commit-bundles")
            .is_true(capture_commit_bundle);
        then.status(201)
            .header("content-type", "application/json")
            .body(r#"{"events":[],"time":"2026-11-25T10:00:01.000Z"}"#);
    })
}

fn mount_message_send(server: &MockServer) -> Mock<'_> {
    server.mock(|when, then| {
        when.method(POST)
            .path("/v14/mls/messages")
            .is_true(capture_mls_message);
        then.status(201)
            .header("content-type", "application/json")
            .body(r#"{"events":[],"time":"2026-11-25T10:00:02.000Z"}"#);
    })
}

/// Every upload is refused with an error the pipeline does not heal.
fn mount_failing_message_send(server: &MockServer) -> Mock<'_> {
    server.mock(|when, then| {
        when.method(POST)
            .path("/v14/mls/messages")
            .is_true(capture_mls_message);
        then.status(400)
            .header("content-type", "application/json")
            .body(r#"{"code":400,"label":"mls-protocol-error","message":"refused"}"#);
    })
}

// ── the fixture ───────────────────────────────────────────────────────

fn inbound(
    room_id: &str,
    message_id: &str,
    sent_at_ms: i64,
    arrived_at_ms: i64,
) -> TimelineEventRecord {
    TimelineEventRecord {
        message_id: message_id.into(),
        room_id: room_id.into(),
        sender_user_id: BOB.into(),
        kind: "text".into(),
        body: format!("msg {message_id}"),
        sent_at_ms,
        arrived_at_ms,
        delivery_status: "received".into(),
        ..TimelineEventRecord::default()
    }
}

/// Room A's rows: `m1` (1000), `m2` (2000), `late` sent at 1500 but arrived
/// at 2500 (Step 151), `m3` (3000); no marker.
async fn seed_room_a(store: &Arc<dyn Store>) {
    for (id, sent, arrived) in [
        ("m1", 1_000, 1_000),
        ("m2", 2_000, 2_000),
        ("late", 1_500, 2_500),
        ("m3", 3_000, 3_000),
    ] {
        store
            .insert_timeline_event(&inbound(&room_a_id(), id, sent, arrived))
            .await
            .expect("seed");
    }
}

struct Fixture {
    client: Arc<Client>,
    store: Arc<dyn Store>,
    room_a: Arc<Room>,
    other_device: PeerDevice,
    /// The uploads captured before this test acted.
    uploads_start: usize,
}

/// A logged-in client that established the personal room (its other device
/// Added, joined through the Welcome) and external-joined room A; room A's
/// rows seeded; the notification cursor at `n1`.
async fn established(server: &MockServer, receipts_on: bool) -> Fixture {
    mount_login(server, receipts_on);
    mount_rooms(server, 0);
    mount_first_page(server);
    let mut other_device = PeerDevice::new(OWN_OTHER_DEVICE);
    let _claim = mount_self_claim(server, &other_device.key_package_b64());
    let _missing = mount_groupinfo_missing(server, SELF_UUID);
    let _room_a_gi = mount_groupinfo(
        server,
        ROOM_A_UUID,
        common::mls::group_info_tls(ROOM_A_GROUP_ID, BOB_IDENTITY),
    );
    let _bundles = mount_commit_bundle(server);
    let (client, store) = common::run_login(server).await;
    client
        .sync_once(SyncSettings::default())
        .await
        .expect("first sync");
    let summary = client.join_all_rooms().await.expect("join_all_rooms");
    assert_eq!(summary.joined, 2, "{:?}", summary.failed);
    let welcome = captures_for(&COMMIT_BUNDLES, server)
        .iter()
        .rev()
        .map(|bundle| common::mls::bundle_segments(bundle))
        .find(|segments| segments.len() == 3 && segments[2].0 == WireFormat::Welcome)
        .map(|segments| segments[2].1.clone())
        .expect("the establishing bundle carries the other device's Welcome");
    other_device.join_via_welcome(&welcome);
    assert_eq!(other_device.epoch(), 1);
    seed_room_a(&store).await;
    let room_a = client.room(&room_a_id()).await.expect("room A cached");
    Fixture {
        client,
        store,
        room_a,
        other_device,
        uploads_start: uploads_so_far(server),
    }
}

async fn mark_read(room: &Room, item: &str, receipt: ReceiptType) {
    room.timeline()
        .expect("timeline")
        .mark_as_read(item.into(), receipt)
        .await
        .expect("mark_as_read");
}

/// The `LastRead`s the other device can read among the personal room's
/// uploads.
fn last_reads_received_by(fx: &mut Fixture, server: &MockServer) -> Vec<DecodedLastRead> {
    let start = fx.uploads_start;
    uploads_for_group(server, start, SELF_GROUP_ID)
        .iter()
        .filter_map(|tls| decode_last_read(&fx.other_device.decrypt_application_message(tls)))
        .collect()
}

fn last_read_for_room_a(ts: i64) -> DecodedLastRead {
    DecodedLastRead {
        conversation_id: ROOM_A_UUID.into(),
        qualified: Some((ROOM_A_UUID.into(), DOMAIN.into())),
        last_read_timestamp: ts,
    }
}

// ── sending ───────────────────────────────────────────────────────────

/// A private read sends one `LastRead` into the personal room: the room, in
/// both id forms, and the item's backend time; the local marker moves too.
#[tokio::test]
async fn a_private_read_sends_one_last_read_with_the_items_time() {
    let server = MockServer::start();
    let mut fx = established(&server, true).await;
    let _send = mount_message_send(&server);

    mark_read(&fx.room_a, "m2", ReceiptType::ReadPrivate).await;

    assert_eq!(
        last_reads_received_by(&mut fx, &server),
        vec![last_read_for_room_a(2_000)]
    );
    assert_eq!(
        uploads_for_group(&server, fx.uploads_start, ROOM_A_GROUP_ID).len(),
        0,
        "a private read tells the room nothing"
    );
    assert_eq!(fx.room_a.read_marker().await.expect("marker"), Some(2_000));
}

/// A read with receipt sends the `Confirmation` into the room and the
/// `LastRead` into the personal room — one each.
#[tokio::test]
async fn a_read_with_receipt_sends_the_confirmation_to_the_room_and_the_last_read_to_the_personal_room()
 {
    let server = MockServer::start();
    let mut fx = established(&server, true).await;
    let _send = mount_message_send(&server);

    mark_read(&fx.room_a, "m3", ReceiptType::Read).await;

    assert_eq!(
        uploads_for_group(&server, fx.uploads_start, ROOM_A_GROUP_ID).len(),
        1
    );
    assert_eq!(
        last_reads_received_by(&mut fx, &server),
        vec![last_read_for_room_a(3_000)]
    );
}

/// The item's backend time rides along, not its arrival: the late message
/// was sent at 1500.
#[tokio::test]
async fn the_last_read_carries_the_backend_time_of_the_item() {
    let server = MockServer::start();
    let mut fx = established(&server, false).await;
    let _send = mount_message_send(&server);

    mark_read(&fx.room_a, "late", ReceiptType::Read).await;

    assert_eq!(
        last_reads_received_by(&mut fx, &server),
        vec![last_read_for_room_a(1_500)]
    );
    assert_eq!(
        fx.room_a.read_marker().await.expect("marker"),
        Some(2_500),
        "the local marker is the arrival (Step 151)"
    );
}

/// A read inside the personal room reports nothing: the other devices read
/// the same room.
#[tokio::test]
async fn a_read_in_the_personal_room_sends_no_last_read() {
    let server = MockServer::start();
    let fx = established(&server, true).await;
    let _send = mount_message_send(&server);
    let mut note = inbound(&personal_room_id(), "note-1", 4_000, 4_000);
    note.sender_user_id = format!("{OWN_USER_UUID}@{DOMAIN}");
    fx.store.insert_timeline_event(&note).await.expect("seed");
    let personal = fx
        .client
        .room(&personal_room_id())
        .await
        .expect("personal room");

    mark_read(&personal, "note-1", ReceiptType::ReadPrivate).await;

    assert_eq!(
        uploads_for_group(&server, fx.uploads_start, SELF_GROUP_ID).len(),
        0
    );
    assert_eq!(personal.read_marker().await.expect("marker"), Some(4_000));
}

/// A read of an item this device does not know anchors no time: no
/// `LastRead` leaves, the flag still clears (Step 138).
#[tokio::test]
async fn a_read_of_an_unknown_item_sends_no_last_read() {
    let server = MockServer::start();
    let fx = established(&server, true).await;
    let _send = mount_message_send(&server);
    fx.room_a.set_unread_flag(true).await.expect("flag");

    mark_read(&fx.room_a, "never-seen", ReceiptType::ReadPrivate).await;

    assert_eq!(
        uploads_for_group(&server, fx.uploads_start, SELF_GROUP_ID).len(),
        0
    );
    assert!(!fx.room_a.is_marked_unread().await.expect("flag"));
}

/// The upload is best effort: a refused `LastRead` leaves the local read
/// standing and the call succeeds (the Step-25b receipt rule).
#[tokio::test]
async fn the_marker_moves_even_when_the_personal_room_upload_is_refused() {
    let server = MockServer::start();
    let fx = established(&server, true).await;
    let _send = mount_failing_message_send(&server);

    mark_read(&fx.room_a, "m2", ReceiptType::ReadPrivate).await;

    assert_eq!(
        uploads_for_group(&server, fx.uploads_start, SELF_GROUP_ID).len(),
        1,
        "the upload was attempted"
    );
    assert_eq!(fx.room_a.read_marker().await.expect("marker"), Some(2_000));
}

/// A device outside the personal room joins it first (the Step 86
/// inheritance), then sends: one external commit, then one upload.
#[tokio::test]
async fn a_device_outside_the_personal_room_joins_it_before_it_sends() {
    let server = MockServer::start();
    mount_login(&server, true);
    mount_rooms(&server, 1);
    mount_first_page(&server);
    let _gi = mount_groupinfo(
        &server,
        SELF_UUID,
        common::mls::group_info_tls(SELF_GROUP_ID, OWN_OTHER_DEVICE),
    );
    let _bundles = mount_commit_bundle(&server);
    let _send = mount_message_send(&server);
    let (client, store) = common::run_login(&server).await;
    client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync");
    seed_room_a(&store).await;
    let room_a = client.room(&room_a_id()).await.expect("room A");
    let bundles_before = captures_for(&COMMIT_BUNDLES, &server).len();
    let uploads_before = uploads_so_far(&server);

    mark_read(&room_a, "m2", ReceiptType::ReadPrivate).await;

    let personal_bundles = captures_for(&COMMIT_BUNDLES, &server)
        .iter()
        .skip(bundles_before)
        .filter(|b| b.windows(SELF_GROUP_ID.len()).any(|w| w == SELF_GROUP_ID))
        .count();
    assert_eq!(
        personal_bundles, 1,
        "one external commit into the personal room"
    );
    assert_eq!(
        uploads_for_group(&server, uploads_before, SELF_GROUP_ID).len(),
        1
    );
    assert_eq!(room_a.read_marker().await.expect("marker"), Some(2_000));
}

// ── receiving ─────────────────────────────────────────────────────────

/// A `LastRead` from the other device lands on the latest arrival among the
/// rows sent by its time (the late message included), and the sync pass
/// names the room.
#[tokio::test]
async fn a_last_read_from_the_other_device_moves_the_marker_and_names_the_room() {
    let server = MockServer::start();
    let mut fx = established(&server, true).await;
    let tls =
        fx.other_device
            .application_message(&last_read_message("lr-1", ROOM_A_UUID, DOMAIN, 2_000));
    mount_second_page(&server, vec![personal_room_event(&tls)]);

    let summary = fx
        .client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync");

    assert_eq!(fx.room_a.read_marker().await.expect("marker"), Some(2_500));
    assert_eq!(summary.changed_rooms, vec![room_a_id()]);
    assert_eq!(
        fx.store
            .unread_count_for_room(&room_a_id(), &format!("{OWN_USER_UUID}@{DOMAIN}"))
            .await
            .expect("unread"),
        1,
        "only m3 is left unread"
    );
}

/// An older position moves nothing, names nothing and leaves a flag set by
/// hand in place.
#[tokio::test]
async fn an_older_last_read_moves_nothing_and_leaves_the_flag() {
    let server = MockServer::start();
    let mut fx = established(&server, true).await;
    let _send = mount_message_send(&server);
    mark_read(&fx.room_a, "m3", ReceiptType::ReadPrivate).await;
    fx.room_a.set_unread_flag(true).await.expect("flag");
    let tls =
        fx.other_device
            .application_message(&last_read_message("lr-2", ROOM_A_UUID, DOMAIN, 1_000));
    mount_second_page(&server, vec![personal_room_event(&tls)]);

    let summary = fx
        .client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync");

    assert_eq!(fx.room_a.read_marker().await.expect("marker"), Some(3_000));
    assert!(fx.room_a.is_marked_unread().await.expect("flag"));
    assert_eq!(summary.changed_rooms, Vec::<String>::new());
}

/// A `LastRead` that moves the marker clears the flag, as a local read does.
#[tokio::test]
async fn a_last_read_that_moves_the_marker_clears_the_flag() {
    let server = MockServer::start();
    let mut fx = established(&server, true).await;
    fx.room_a.set_unread_flag(true).await.expect("flag");
    let tls =
        fx.other_device
            .application_message(&last_read_message("lr-3", ROOM_A_UUID, DOMAIN, 3_000));
    mount_second_page(&server, vec![personal_room_event(&tls)]);

    fx.client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync");

    assert_eq!(fx.room_a.read_marker().await.expect("marker"), Some(3_000));
    assert!(!fx.room_a.is_marked_unread().await.expect("flag"));
}

/// A `LastRead` for a room this device does not know is dropped: no marker
/// row, no changed room, no error.
#[tokio::test]
async fn a_last_read_for_an_unknown_room_is_dropped() {
    let server = MockServer::start();
    let mut fx = established(&server, true).await;
    let ghost = "0d0d0d0d-0000-4000-8000-000000000000";
    let tls = fx
        .other_device
        .application_message(&last_read_message("lr-4", ghost, DOMAIN, 9_000));
    mount_second_page(&server, vec![personal_room_event(&tls)]);

    let summary = fx
        .client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync");

    assert_eq!(summary.changed_rooms, Vec::<String>::new());
    assert!(
        fx.store
            .read_states()
            .await
            .expect("read states")
            .is_empty(),
        "no marker row for a room this device does not know"
    );
}

/// The legacy form — `conversation_id` without the qualified id — names a
/// room of the own backend.
#[tokio::test]
async fn a_legacy_last_read_without_the_qualified_id_names_the_own_backends_room() {
    let server = MockServer::start();
    let mut fx = established(&server, true).await;
    let mut last_read = Vec::new();
    put_len_delimited(1, ROOM_A_UUID.as_bytes(), &mut last_read);
    put_varint(2 << 3, &mut last_read);
    put_varint(2_000, &mut last_read);
    let mut m = Vec::new();
    put_len_delimited(1, b"lr-5", &mut m);
    put_len_delimited(6, &last_read, &mut m);
    let tls = fx.other_device.application_message(&m);
    mount_second_page(&server, vec![personal_room_event(&tls)]);

    fx.client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync");

    assert_eq!(fx.room_a.read_marker().await.expect("marker"), Some(2_500));
}

/// A `LastRead` outside the personal room is a stranger's claim: Bob's, in
/// his group, moves nothing.
#[tokio::test]
async fn a_last_read_outside_the_personal_room_is_ignored() {
    use common::drain_fixture::{ROOM_UUID, joined_client, mls_event, room_id};
    let server = MockServer::start();
    let mut fx = joined_client(&server).await;
    let tls = fx
        .bob
        .application_message(&last_read_message("lr-6", ROOM_UUID, DOMAIN, 5_000));
    mount_page(
        &server,
        Some("n1"),
        &[Notif {
            id: "n2",
            events: vec![mls_event(&tls)],
        }],
    );
    mount_page(&server, Some("n2"), &[]);

    fx.client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync");

    let room = fx.client.room(&room_id()).await.expect("the room");
    assert_eq!(room.read_marker().await.expect("marker"), None);
    assert!(
        fx.recorder.errors().is_empty(),
        "{:?}",
        fx.recorder.errors()
    );
}
