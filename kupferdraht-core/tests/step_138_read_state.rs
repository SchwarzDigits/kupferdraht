// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 138 — read state on the browser surface (TDD red): a read without
//! receipt, a hand-set unread flag, read state in `rooms()`.
//!
//! The core behaviour is driven natively here: `Timeline::mark_as_read`
//! with a [`ReceiptType`], `Room::set_unread_flag` / `is_marked_unread` /
//! `read_marker`, and the store's one-query read state for room lists. The
//! browser surface is hand-written, so its pins are source-level (the
//! Step 98 pattern): `rooms()` carries the three fields, `markAsRead`
//! takes its options, `markAsUnread` and `startSync`'s `onRoomsChanged`
//! exist, and the tab module mirrors them. The signal's fan-out across
//! tabs runs in the browser
//! (`step_94_leader_tab::a_read_state_change_reaches_every_tab`).

use std::path::Path;
use std::sync::Arc;

use httpmock::Method::{GET, POST};
use httpmock::{Mock, MockServer};
use kupferdraht::storage::{ReadStateRecord, Store, TimelineEventRecord};
use kupferdraht::{Client, ReceiptType, Room};
use openmls::group::{MlsGroup, MlsGroupCreateConfig};
use openmls::prelude::{
    BasicCredential, Capabilities, Ciphersuite, CredentialType, CredentialWithKey,
};
use openmls_basic_credential::SignatureKeyPair;
use openmls_rust_crypto::RustCrypto;
use openmls_sqlite_storage::{Codec, Connection, SqliteStorageProvider};
use openmls_traits::OpenMlsProvider;
use openmls_traits::types::SignatureScheme;
use serde::Serialize;
use serde::de::DeserializeOwned;
use tls_codec::Serialize as _;

mod common;
mod wasm_surface;

const SELF_USER: &str = "user-uuid@example.test";
const BOB: &str = "bob-uuid@example.test";
const ROOM_A: &str = "room-a@example.test";
const ROOM_B: &str = "room-b@example.test";
const CIPHERSUITE: Ciphersuite = Ciphersuite::MLS_128_DHKEMP256_AES128GCM_SHA256_P256;
const SIGNATURE_SCHEME: SignatureScheme = SignatureScheme::ECDSA_SECP256R1_SHA256;

fn workspace_file(rel: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// Login with the account's `WIRE_RECEIPT_MODE` on or off (the Step 10f
/// gate): off lets a `Read` short-circuit before any MLS work, on is the
/// setting under which a private read must still send nothing.
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

/// Two MLS rooms in the metadata mocks so `Room` handles exist.
fn mount_two_rooms(server: &MockServer) {
    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list-ids");
        then.status(200)
            .header("content-type", "application/json")
            .body(
                r#"{"qualified_conversations":[
                     {"id":"room-a","domain":"example.test"},
                     {"id":"room-b","domain":"example.test"}
                   ],"has_more":false,"paging_state":"END"}"#,
            );
    });
    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list");
        then.status(200)
            .header("content-type", "application/json")
            .body(
                r#"{"found":[
                     {"qualified_id":{"id":"room-a","domain":"example.test"},
                      "type":0,"protocol":"mls","epoch":1,"group_id":"Z3JwLWFh",
                      "name":"A","members":{"self":{"qualified_id":{"id":"user-uuid","domain":"example.test"}},"others":[]}},
                     {"qualified_id":{"id":"room-b","domain":"example.test"},
                      "type":0,"protocol":"mls","epoch":1,"group_id":"Z3JwLWJi",
                      "name":"B","members":{"self":{"qualified_id":{"id":"user-uuid","domain":"example.test"}},"others":[]}}
                   ], "not_found":[], "failed":[]}"#,
            );
    });
    server.mock(|when, then| {
        when.method(POST).path("/v14/list-users");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"found":[]}"#);
    });
}

/// The upload a `Confirmation` would be — it must stay at zero calls for
/// a private read.
fn mount_message_upload(server: &MockServer) -> Mock<'_> {
    server.mock(|when, then| {
        when.method(POST).path("/v14/mls/messages");
        then.status(201)
            .header("content-type", "application/json")
            .body(r#"{"events":[],"time":"2026-11-03T10:00:00.000Z"}"#);
    })
}

// ── the Step 4 MLS fixture: a group Alice can join externally ─────────

#[derive(Default)]
struct TestJsonCodec;
impl Codec for TestJsonCodec {
    type Error = serde_json::Error;
    fn to_vec<T: Serialize>(value: &T) -> Result<Vec<u8>, Self::Error> {
        serde_json::to_vec(value)
    }
    fn from_slice<T: DeserializeOwned>(slice: &[u8]) -> Result<T, Self::Error> {
        serde_json::from_slice(slice)
    }
}

struct AliceProvider {
    crypto: RustCrypto,
    storage: SqliteStorageProvider<TestJsonCodec, Connection>,
}

impl AliceProvider {
    fn new_in_memory() -> Self {
        let conn = Connection::open_in_memory().expect("sqlite in-memory");
        let mut storage: SqliteStorageProvider<TestJsonCodec, Connection> =
            SqliteStorageProvider::new(conn);
        storage.run_migrations().expect("openmls migrations");
        Self {
            crypto: RustCrypto::default(),
            storage,
        }
    }
}

impl OpenMlsProvider for AliceProvider {
    type CryptoProvider = RustCrypto;
    type RandProvider = RustCrypto;
    type StorageProvider = SqliteStorageProvider<TestJsonCodec, Connection>;

    fn storage(&self) -> &Self::StorageProvider {
        &self.storage
    }
    fn crypto(&self) -> &Self::CryptoProvider {
        &self.crypto
    }
    fn rand(&self) -> &Self::RandProvider {
        &self.crypto
    }
}

/// A group as the backend would serve it for an external join: its
/// `GroupInfo` bytes and its id as Wire delivers it (base64).
fn alice_group_setup() -> (Vec<u8>, String) {
    let provider = AliceProvider::new_in_memory();
    let signer = SignatureKeyPair::new(SIGNATURE_SCHEME).expect("signer");
    signer.store(provider.storage()).expect("persist signer");
    let caps = Capabilities::new(
        None,
        None,
        None,
        None,
        Some(&[CredentialType::Basic, CredentialType::X509]),
    );
    let credential = CredentialWithKey {
        credential: BasicCredential::new(b"alice-uuid:alice-client@example.test".to_vec()).into(),
        signature_key: signer.public().to_vec().into(),
    };
    let config = MlsGroupCreateConfig::builder()
        .ciphersuite(CIPHERSUITE)
        .use_ratchet_tree_extension(true)
        .capabilities(caps)
        .build();
    let group = MlsGroup::new(&provider, &signer, &config, credential).expect("group");
    let gi_bytes = group
        .export_group_info(provider.crypto(), &signer, true)
        .expect("export_group_info")
        .tls_serialize_detached()
        .expect("group_info serialize");
    use base64::Engine as _;
    let group_id_b64 =
        base64::engine::general_purpose::STANDARD.encode(group.group_id().as_slice());
    (gi_bytes, group_id_b64)
}

/// Room A alone in the metadata mocks, carrying the fixture group's id.
fn mount_room_a_with_group(server: &MockServer, group_id_b64: &str) {
    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list-ids");
        then.status(200)
            .header("content-type", "application/json")
            .body(
                r#"{"qualified_conversations":[{"id":"room-a","domain":"example.test"}],
                    "has_more":false,"paging_state":"END"}"#,
            );
    });
    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list");
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(
                r#"{{"found":[{{
                     "qualified_id":{{"id":"room-a","domain":"example.test"}},
                     "type":0,"protocol":"mls","epoch":0,
                     "group_id":"{group_id_b64}","name":"A",
                     "members":{{
                       "self":{{"qualified_id":{{"id":"user-uuid","domain":"example.test"}}}},
                       "others":[]
                     }}}}],"not_found":[],"failed":[]}}"#
            ));
    });
    server.mock(|when, then| {
        when.method(POST).path("/v14/list-users");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"found":[]}"#);
    });
}

/// The external join: the group's `GroupInfo` and the commit bundle the
/// backend accepts.
fn mount_external_join(server: &MockServer, gi_bytes: Vec<u8>) {
    server.mock(|when, then| {
        when.method(GET)
            .path("/v14/conversations/example.test/room-a/groupinfo");
        then.status(200)
            .header("content-type", "message/mls")
            .body(gi_bytes);
    });
    server.mock(|when, then| {
        when.method(POST)
            .path("/v14/mls/commit-bundles")
            .header("content-type", "message/mls");
        then.status(201)
            .header("content-type", "application/json")
            .body(r#"{"events":[],"time":"2026-11-03T10:00:00.000Z"}"#);
    });
}

fn inbound(room_id: &str, message_id: &str, sent_at_ms: i64) -> TimelineEventRecord {
    TimelineEventRecord {
        message_id: message_id.into(),
        room_id: room_id.into(),
        sender_user_id: BOB.into(),
        kind: "text".into(),
        body: format!("msg {message_id}"),
        sent_at_ms,
        delivery_status: "received".into(),
        ..TimelineEventRecord::default()
    }
}

/// A logged-in client with room A holding three inbound messages
/// (`m1` at 1000, `m2` at 2000, `m3` at 3000) and no marker.
async fn room_a_with_three_inbound(
    server: &MockServer,
    receipts_on: bool,
) -> (Arc<Client>, Arc<dyn Store>, Arc<Room>) {
    mount_login(server, receipts_on);
    mount_two_rooms(server);
    let (client, store) = common::run_login(server).await;
    client.sync_metadata().await.expect("rooms cached");
    for (id, at) in [("m1", 1_000), ("m2", 2_000), ("m3", 3_000)] {
        store
            .insert_timeline_event(&inbound(ROOM_A, id, at))
            .await
            .expect("seed inbound");
    }
    let room = client.room(ROOM_A).await.expect("room cached");
    (client, store, room)
}

// ── the flag ──────────────────────────────────────────────────────────

/// A room flagged by hand reads as flagged; the flag is beside the marker,
/// not a marker move — a room never read has none afterwards either.
#[tokio::test]
async fn a_room_flagged_by_hand_is_marked_unread_without_a_marker() {
    let server = MockServer::start();
    let (_client, _store, room) = room_a_with_three_inbound(&server, false).await;
    assert!(!room.is_marked_unread().await.expect("flag"), "fresh room");

    room.set_unread_flag(true).await.expect("flag the room");

    assert!(room.is_marked_unread().await.expect("flag"));
    assert_eq!(room.read_marker().await.expect("marker"), None);
    assert_eq!(room.unread_count().await.expect("count"), 3);
}

/// Clearing the flag by hand leaves the marker where it is.
#[tokio::test]
async fn clearing_the_flag_by_hand_leaves_the_marker() {
    let server = MockServer::start();
    let (_client, _store, room) = room_a_with_three_inbound(&server, true).await;
    let timeline = room.timeline().expect("timeline");
    timeline
        .mark_as_read("m2".into(), ReceiptType::ReadPrivate)
        .await
        .expect("private read");
    room.set_unread_flag(true).await.expect("flag");

    room.set_unread_flag(false).await.expect("clear by hand");

    assert!(!room.is_marked_unread().await.expect("flag"));
    assert_eq!(room.read_marker().await.expect("marker"), Some(2_000));
}

/// The motivating case: a room read to the end (count 0) is put back in
/// the work queue by hand — the flag carries the attention the count
/// cannot; flagging twice is idempotent.
#[tokio::test]
async fn a_room_read_to_the_end_still_needs_attention_when_flagged() {
    let server = MockServer::start();
    let (_client, _store, room) = room_a_with_three_inbound(&server, true).await;
    let timeline = room.timeline().expect("timeline");
    timeline
        .mark_as_read("m3".into(), ReceiptType::ReadPrivate)
        .await
        .expect("read to the end");
    assert_eq!(room.unread_count().await.expect("count"), 0);

    room.set_unread_flag(true).await.expect("flag");
    room.set_unread_flag(true)
        .await
        .expect("flagging twice is idempotent");

    assert_eq!(
        room.unread_count().await.expect("count"),
        0,
        "the count stays honest"
    );
    assert!(
        room.is_marked_unread().await.expect("flag"),
        "the flag carries the attention"
    );
    assert_eq!(room.read_marker().await.expect("marker"), Some(3_000));
}

// ── the receipt type ──────────────────────────────────────────────────

/// `Read` with receipts on — the first native pin of the READ upload since
/// 6-II-f: one `Confirmation` leaves, the marker moves, the flag clears,
/// and the own receipt is recorded so `read_by` shows the reader at once.
#[tokio::test]
async fn a_read_with_receipt_uploads_one_confirmation_moves_the_marker_and_clears_the_flag() {
    let server = MockServer::start();
    mount_login(&server, true);
    let (gi_bytes, group_id_b64) = alice_group_setup();
    mount_room_a_with_group(&server, &group_id_b64);
    mount_external_join(&server, gi_bytes);
    let upload = mount_message_upload(&server);
    let (client, store) = common::run_login(&server).await;
    client.sync_metadata().await.expect("rooms cached");
    client.join_room(ROOM_A).await.expect("external join");
    for (id, at) in [("m1", 1_000), ("m2", 2_000), ("m3", 3_000)] {
        store
            .insert_timeline_event(&inbound(ROOM_A, id, at))
            .await
            .expect("seed inbound");
    }
    let room = client.room(ROOM_A).await.expect("room cached");
    room.set_unread_flag(true).await.expect("flag");
    let timeline = room.timeline().expect("timeline");

    let receipt = timeline
        .mark_as_read("m2".into(), ReceiptType::Read)
        .await
        .expect("read with receipts on");

    let handle = receipt.expect("receipts on → a confirmation was sent");
    assert_eq!(handle.message_id.len(), 36, "UUID v4 hyphenated");
    upload.assert_calls(1);
    assert_eq!(room.read_marker().await.expect("marker"), Some(2_000));
    assert!(!room.is_marked_unread().await.expect("flag"));
    assert_eq!(room.unread_count().await.expect("count"), 1);
    let receipts = store
        .load_read_receipts_for_room(ROOM_A)
        .await
        .expect("receipts");
    assert!(
        receipts
            .iter()
            .any(|r| r.message_id == "m2" && r.reader_user_id == SELF_USER),
        "the own receipt is recorded: {receipts:?}"
    );
}

/// `ReadPrivate` with receipts on: the marker moves, the flag clears, and
/// nothing is uploaded — the room learns nothing.
#[tokio::test]
async fn a_private_read_moves_the_marker_clears_the_flag_and_sends_nothing() {
    let server = MockServer::start();
    let (_client, _store, room) = room_a_with_three_inbound(&server, true).await;
    let upload = mount_message_upload(&server);
    room.set_unread_flag(true).await.expect("flag");
    let timeline = room.timeline().expect("timeline");

    let receipt = timeline
        .mark_as_read("m2".into(), ReceiptType::ReadPrivate)
        .await
        .expect("a private read");

    assert!(receipt.is_none(), "a private read confirms nothing");
    assert_eq!(room.read_marker().await.expect("marker"), Some(2_000));
    assert!(!room.is_marked_unread().await.expect("flag"));
    assert_eq!(room.unread_count().await.expect("count"), 1);
    upload.assert_calls(0);
}

/// `Read` keeps the Step 10f gate (receipts off → `None`, marker moved)
/// and clears the flag too.
#[tokio::test]
async fn a_read_with_receipt_keeps_the_gate_and_clears_the_flag() {
    let server = MockServer::start();
    let (_client, _store, room) = room_a_with_three_inbound(&server, false).await;
    room.set_unread_flag(true).await.expect("flag");
    let timeline = room.timeline().expect("timeline");

    let receipt = timeline
        .mark_as_read("m2".into(), ReceiptType::Read)
        .await
        .expect("read with receipts off");

    assert!(
        receipt.is_none(),
        "receipts off → no confirmation (unchanged)"
    );
    assert_eq!(room.read_marker().await.expect("marker"), Some(2_000));
    assert!(!room.is_marked_unread().await.expect("flag"));
}

/// An item unknown locally anchors no position: the marker stays, the flag
/// still clears — the caller's "this room is read" holds.
#[tokio::test]
async fn a_read_of_an_unknown_item_moves_no_marker_and_still_clears_the_flag() {
    let server = MockServer::start();
    let (_client, _store, room) = room_a_with_three_inbound(&server, true).await;
    let upload = mount_message_upload(&server);
    room.set_unread_flag(true).await.expect("flag");
    let timeline = room.timeline().expect("timeline");

    let receipt = timeline
        .mark_as_read("not-synced".into(), ReceiptType::ReadPrivate)
        .await
        .expect("a private read of an unknown item");

    assert!(receipt.is_none());
    assert_eq!(room.read_marker().await.expect("marker"), None);
    assert!(!room.is_marked_unread().await.expect("flag"));
    upload.assert_calls(0);
}

/// The marker only moves forward, through a private read as well.
#[tokio::test]
async fn the_marker_stays_forward_only_under_a_private_read() {
    let server = MockServer::start();
    let (_client, _store, room) = room_a_with_three_inbound(&server, true).await;
    let timeline = room.timeline().expect("timeline");
    for item in ["m3", "m1"] {
        timeline
            .mark_as_read(item.into(), ReceiptType::ReadPrivate)
            .await
            .expect("private read");
    }

    assert_eq!(room.read_marker().await.expect("marker"), Some(3_000));
    assert_eq!(room.unread_count().await.expect("count"), 0);
}

// ── the store's room-list query ───────────────────────────────────────

/// One query answers the read state of every room: marker and flag per
/// room, a flagged room without a position included, an unmarked room
/// absent; the marker upsert clears the flag.
#[tokio::test]
async fn the_store_reports_every_room_read_state_in_one_query() {
    let server = MockServer::start();
    mount_login(&server, false);
    mount_two_rooms(&server);
    let (_client, store) = common::run_login(&server).await;
    store
        .set_read_marker(ROOM_A, 1_500)
        .await
        .expect("marker a");
    store.set_unread_flag(ROOM_A, true).await.expect("flag a");
    store.set_unread_flag(ROOM_B, true).await.expect("flag b");

    let states = store.read_states().await.expect("read states");

    assert_eq!(
        states.get(ROOM_A),
        Some(&ReadStateRecord {
            last_read_at_ms: Some(1_500),
            marked_unread: true,
        })
    );
    assert_eq!(
        states.get(ROOM_B),
        Some(&ReadStateRecord {
            last_read_at_ms: None,
            marked_unread: true,
        })
    );
    assert_eq!(states.len(), 2, "an unmarked room is absent");

    store.set_read_marker(ROOM_B, 2_500).await.expect("read b");
    let states = store.read_states().await.expect("read states");
    assert_eq!(
        states.get(ROOM_B),
        Some(&ReadStateRecord {
            last_read_at_ms: Some(2_500),
            marked_unread: false,
        }),
        "the marker upsert clears the flag"
    );
}

// ── the browser surface (source-level pins) ───────────────────────────

fn view_struct(src: &str, name: &str) -> String {
    let start = src
        .find(&format!("struct {name} {{"))
        .unwrap_or_else(|| panic!("the wasm source defines {name}"));
    let body = &src[start..];
    let end = body.find("\n}").expect("struct closes");
    body[..end].to_string()
}

/// The whole signature of `pub async fn {name}(` / `pub fn {name}(` —
/// rustfmt wraps a long parameter list.
fn signature(src: &str, name: &str) -> String {
    let start = src
        .find(&format!("pub async fn {name}("))
        .or_else(|| src.find(&format!("pub fn {name}(")))
        .unwrap_or_else(|| panic!("manager.rs binds {name}()"));
    let sig = &src[start..];
    sig[..sig.find(" {").expect("signature ends")].to_string()
}

/// `rooms()` carries the read state, `markAsRead` takes its options,
/// `markAsUnread` exists, `startSync` takes `onRoomsChanged`.
#[test]
fn the_wasm_surface_carries_the_read_state() {
    let s = wasm_surface::surface();
    wasm_surface::assert_bound(&s, "Client", "markAsUnread");
    let src = wasm_surface::manager_source();
    let projection = workspace_file("kupferdraht-core/src/wasm/read_state.rs");
    let view = view_struct(&projection, "RoomView");
    for field in ["unread_count", "last_read_at_ms", "marked_unread"] {
        assert!(view.contains(field), "RoomView carries {field}: {view}");
    }
    let change = view_struct(&projection, "ReadStateChange");
    for field in [
        "room_id",
        "unread_count",
        "last_read_at_ms",
        "marked_unread",
    ] {
        assert!(
            change.contains(field),
            "ReadStateChange carries {field}: {change}"
        );
    }
    assert!(
        !change.contains("name"),
        "the change payload is the room's row without its name"
    );
    let mark = signature(&src, "mark_as_read");
    assert!(mark.contains("options"), "markAsRead takes options: {mark}");
    let sync = signature(&src, "start_sync");
    assert!(
        sync.contains("on_rooms_changed"),
        "startSync takes onRoomsChanged: {sync}"
    );
}

/// The tab module mirrors the surface: the method table, the proxy, the
/// leader's broadcast and the hand-written types.
#[test]
fn the_tabs_module_mirrors_the_read_state() {
    let module = |name: &str| workspace_file(&format!("bindings/web/kupferdraht-tabs/{name}"));
    let protocol = module("protocol.js");
    assert!(
        protocol.contains(r#"["Client", "markAsUnread", "mutation", null]"#),
        "protocol.js METHODS lists markAsUnread as a mutation"
    );
    let events = protocol
        .lines()
        .find(|l| l.starts_with("export const LEADER_EVENTS"))
        .expect("protocol.js exports LEADER_EVENTS");
    assert!(
        events.contains("'rooms'"),
        "the leader broadcasts rooms: {events}"
    );
    let proxy = module("proxy.js");
    assert!(
        proxy.contains("\n  markAsUnread(roomId)"),
        "proxy.js defines markAsUnread"
    );
    assert!(
        proxy.contains("onRoomsChanged"),
        "proxy.js delivers onRoomsChanged"
    );
    assert!(
        module("leader.js").contains("emit('rooms')"),
        "leader.js broadcasts the seventh callback as rooms"
    );
    let dts = module("index.d.ts");
    for needle in [
        "markAsUnread(roomId: string): Promise<void>",
        "unreadCount: number",
        "lastReadAtMs: number | null",
        "markedUnread: boolean",
        "sendReceipt?: boolean",
        "onRoomsChanged",
        "interface ReadStateChange",
        "'rooms'",
    ] {
        assert!(dts.contains(needle), "index.d.ts lacks {needle:?}");
    }
}

// ── the doc homes ─────────────────────────────────────────────────────

/// The first line of `doc` containing `needle`.
fn line_with(doc: &str, needle: &str) -> String {
    doc.lines()
        .find(|l| l.contains(needle))
        .map(str::to_owned)
        .unwrap_or_else(|| panic!("no line contains {needle:?}"))
}

/// The paragraph of `doc` (up to the next blank line) that starts at the
/// first line containing `needle`.
fn paragraph_with(doc: &str, needle: &str) -> String {
    let lines: Vec<&str> = doc.lines().collect();
    let start = lines
        .iter()
        .position(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("no line contains {needle:?}"));
    lines[start..]
        .iter()
        .take_while(|l| !l.trim().is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every doc home the step touches names the change; the changelog calls
/// the signature change what it is.
#[test]
fn doc_homes_name_the_read_state() {
    let library = workspace_file("docs/library.md");
    for needle in [
        "ReadPrivate",
        "set_unread_flag",
        "is_marked_unread",
        "read_marker",
    ] {
        assert!(library.contains(needle), "docs/library.md lacks {needle:?}");
    }
    // The flag and the private read are device-local — said at the
    // symbols, not only in the plan (review 2026-11-03).
    assert!(
        line_with(&library, "| `Room::set_unread_flag(").contains("device-local"),
        "library.md: the set_unread_flag row says device-local"
    );
    let mark = line_with(&library, "| `Timeline::mark_as_read(");
    assert!(
        mark.contains("ReadPrivate") && mark.contains("device-local"),
        "library.md: the mark_as_read row names ReadPrivate as device-local: {mark}"
    );
    let bindings = workspace_file("docs/bindings.md");
    for needle in [
        "markAsUnread",
        "onRoomsChanged",
        "markedUnread",
        "sendReceipt",
    ] {
        assert!(
            bindings.contains(needle),
            "docs/bindings.md lacks {needle:?}"
        );
    }
    let signal = paragraph_with(&bindings, "`onRoomsChanged(");
    for needle in ["unreadCount", "onSync", "rooms"] {
        assert!(
            signal.contains(needle),
            "bindings.md: the onRoomsChanged paragraph names {needle:?} (payload, double signal, broadcast): {signal}"
        );
    }
    assert!(
        paragraph_with(&bindings, "`markAsUnread(").contains("device-local"),
        "bindings.md: markAsUnread is device-local"
    );
    let cli = workspace_file("docs/cli.md");
    assert!(
        cli.contains("### `mark-unread`"),
        "docs/cli.md lacks mark-unread"
    );
    assert!(
        cli.contains("--no-receipt"),
        "docs/cli.md lacks --no-receipt"
    );
    let mapping = workspace_file("docs/api-mapping-matrix-rust-sdk.md");
    for needle in ["set_unread_flag", "is_marked_unread"] {
        assert!(mapping.contains(needle), "api-mapping lacks {needle:?}");
    }
    let row = mapping
        .lines()
        .find(|l| l.starts_with("| `Timeline::mark_as_read("))
        .expect("api-mapping has the mark_as_read row");
    assert!(
        row.contains("ReceiptType") && row.contains("| Aligned"),
        "the mark_as_read row moved from Drift toward Aligned: {row}"
    );
    assert!(
        workspace_file("docs/ARCHITECTURE.md").contains("marked_unread"),
        "ARCHITECTURE §7 lacks the V31 column"
    );
    assert!(
        workspace_file("rules/0026-leader-tab-serves-thin-client-tabs.md").contains("`rooms`"),
        "rules/0026 lacks the rooms broadcast"
    );
    let changelog = workspace_file("CHANGELOG.md");
    let start = changelog
        .find("\n## [Unreleased]\n")
        .expect("an [Unreleased] section")
        + 1;
    let rest = &changelog[start..];
    let end = rest[3..].find("\n## [").map_or(rest.len(), |i| i + 3);
    let unreleased = &rest[..end];
    assert!(
        unreleased.contains("BREAKING") && unreleased.contains("ReceiptType"),
        "CHANGELOG [Unreleased] names the breaking mark_as_read change"
    );
}
