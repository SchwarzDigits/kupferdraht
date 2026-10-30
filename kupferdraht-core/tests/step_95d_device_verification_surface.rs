// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 95d — the read-only device verification surface. TDD red.
//!
//! A room's devices are the leaves of its MLS group, each with the W-4
//! identity and the RFC 7638 MLS thumbprint of its signature key (what
//! official Wire apps show as "MLS Thumbprint"). A change of the device
//! set is reported as a diff against the last persisted set — the only
//! signal against an injected device on a backend without e2ei.
//!
//! The group: bob (creator, `PeerGroup`) and carol (`PeerDevice`),
//! then the client joins by Welcome. `wire::proto` is crate-private
//! (ADR-0009), so application payloads are hand-emitted protobuf.

use std::sync::{Arc, Mutex};

use base64::Engine as _;
use httpmock::Method::{GET, POST};
use httpmock::{MockServer, prelude::HttpMockRequest};
use kupferdraht::storage::Store;
use kupferdraht::{
    ClientBuilder, DeviceCredential, RoomDevice, RoomDevicesChange, RoomDevicesListener,
    SyncSettings,
};

mod common;

use common::mls::{PeerDevice, PeerGroup};

const DOMAIN: &str = "example.test";
const ROOM_UUID: &str = "95d0aa31-77aa-4bde-9d10-223344556677";
/// b64("grp-95d") — the room's MLS group id.
const GROUP_ID_B64: &str = "Z3JwLTk1ZA==";
const GROUP_ID: &[u8] = b"grp-95d";
const BOB: &str = "bob-uuid";
const BOB_IDENTITY: &[u8] = b"bob-uuid:bobdev01@example.test";
const CAROL_IDENTITY: &[u8] = b"carol-uuid:caroldev@example.test";
const DAVE_IDENTITY: &[u8] = b"dave-uuid:davedev@example.test";

fn room_id() -> String {
    format!("{ROOM_UUID}@{DOMAIN}")
}

// ---- mocks (Step-69 / 95b shapes) ----

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
    let (_, kps) = guard
        .iter()
        .rev()
        .find(|(h, _)| *h == host)
        .expect("register_device uploaded key packages");
    base64::engine::general_purpose::STANDARD
        .decode(kps.first().expect("a key package"))
        .expect("uploaded key package is base64")
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
    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list");
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(
                r#"{{"found":[{{
                     "qualified_id":{{"id":"{ROOM_UUID}","domain":"{DOMAIN}"}},
                     "type":0,"protocol":"mls","epoch":1,"group_id":"{GROUP_ID_B64}",
                     "name":"Devices",
                     "members":{{
                       "self":{{"qualified_id":{{"id":"user-uuid","domain":"{DOMAIN}"}},
                               "conversation_role":"wire_member"}},
                       "others":[
                         {{"qualified_id":{{"id":"{BOB}","domain":"{DOMAIN}"}},
                           "conversation_role":"wire_member"}}
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

/// One notifications page, answered only for the request whose
/// `since` cursor is `since` (`None`: the first, cursor-less drain) —
/// each sync sees exactly its own page.
fn mount_notifications_page(
    server: &MockServer,
    since: Option<&str>,
    notif_id: &str,
    events: &[String],
) {
    let body = format!(
        r#"{{"notifications":[{{"id":"{notif_id}","payload":[{}]}}],"has_more":false}}"#,
        events.join(",")
    );
    let since = since.map(str::to_string);
    server.mock(move |when, then| {
        let when = when.method(GET).path("/v14/notifications");
        match &since {
            Some(cursor) => when.query_param("since", cursor.as_str()),
            None => when.query_param_missing("since"),
        };
        then.status(200)
            .header("content-type", "application/json")
            .body(body.clone());
    });
}

fn welcome_event(data: &[u8]) -> String {
    format!(
        r#"{{"type":"conversation.mls-welcome",
             "qualified_conversation":{{"id":"{ROOM_UUID}","domain":"{DOMAIN}"}},
             "qualified_from":{{"id":"{BOB}","domain":"{DOMAIN}"}},
             "data":"{}",
             "time":"2026-09-26T10:00:00.000Z"}}"#,
        base64::engine::general_purpose::STANDARD.encode(data)
    )
}

fn message_event(data: &[u8]) -> String {
    format!(
        r#"{{"type":"conversation.mls-message-add",
             "qualified_conversation":{{"id":"{ROOM_UUID}","domain":"{DOMAIN}"}},
             "qualified_from":{{"id":"{BOB}","domain":"{DOMAIN}"}},
             "data":"{}",
             "time":"2026-09-26T10:00:01.000Z"}}"#,
        base64::engine::general_purpose::STANDARD.encode(data)
    )
}

/// `GenericMessage { message_id = 1, text = 2 { content = 1 } }`.
fn text_message(message_id: &str, content: &str) -> Vec<u8> {
    fn put(field: u8, bytes: &[u8], out: &mut Vec<u8>) {
        out.push(field << 3 | 2);
        out.push(u8::try_from(bytes.len()).expect("short field"));
        out.extend_from_slice(bytes);
    }
    let mut text = Vec::new();
    put(1, content.as_bytes(), &mut text);
    let mut m = Vec::new();
    put(1, message_id.as_bytes(), &mut m);
    put(2, &text, &mut m);
    m
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

#[derive(Default)]
struct ChangeRecorder {
    changes: Mutex<Vec<RoomDevicesChange>>,
}

impl ChangeRecorder {
    fn changes(&self) -> Vec<RoomDevicesChange> {
        self.changes.lock().expect("recorder lock").clone()
    }
}

impl RoomDevicesListener for ChangeRecorder {
    fn on_devices_changed(&self, change: RoomDevicesChange) {
        self.changes.lock().expect("recorder lock").push(change);
    }
}

/// bob + carol, then the client by Welcome (first page, first sync).
/// Returns the peers so a test can change the group afterwards.
struct Scenario {
    server: MockServer,
    client: Arc<kupferdraht::Client>,
    store: Arc<dyn Store>,
    bob: PeerGroup,
    /// The last page's notification id — the next drain's `since`.
    cursor: String,
}

async fn joined_scenario(
    recorder: &Arc<ChangeRecorder>,
) -> (Scenario, kupferdraht::timeline::Subscription) {
    let server = MockServer::start();
    let (client, store) = logged_in_client(&server).await;
    let subscription = client.subscribe_device_changes(recorder.clone());
    mount_room(&server);
    let mut bob = PeerGroup::new(GROUP_ID, BOB_IDENTITY);
    let mut carol = PeerDevice::new(CAROL_IDENTITY);
    let (_commit, welcome_carol) = bob.add_member(&carol.key_package_tls());
    carol.join_via_welcome(&welcome_carol);
    let (_commit, welcome_client) = bob.add_member(&captured_key_package_for(&server));
    let first = "95d0e858-8eae-11ef-0000-000000000001";
    mount_notifications_page(&server, None, first, &[welcome_event(&welcome_client)]);
    client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync_once (welcome)");
    (
        Scenario {
            server,
            client,
            store,
            bob,
            cursor: first.to_string(),
        },
        subscription,
    )
}

async fn sync_next_page(scenario: &mut Scenario, notif_id: &str, events: &[String]) {
    mount_notifications_page(&scenario.server, Some(&scenario.cursor), notif_id, events);
    scenario.cursor = notif_id.to_string();
    scenario
        .client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync_once");
}

async fn devices(scenario: &Scenario) -> Vec<RoomDevice> {
    scenario
        .client
        .room(&room_id())
        .await
        .expect("room cached")
        .devices()
        .await
        .expect("room devices")
}

fn basic(user: &str, device: &str, devices: &[RoomDevice]) -> RoomDevice {
    devices
        .iter()
        .find(|d| d.user_id.as_deref() == Some(user) && d.device_id.as_deref() == Some(device))
        .cloned()
        .unwrap_or_else(|| panic!("{user}/{device} in {devices:?}"))
}

// ---- tests ----

/// Every leaf is listed with its W-4 identity, credential type and MLS
/// thumbprint; the client's own entry carries `own_device_thumbprint`.
#[tokio::test]
async fn room_devices_list_every_leaf_with_identity_and_thumbprint() {
    let recorder = Arc::new(ChangeRecorder::default());
    let (scenario, _sub) = joined_scenario(&recorder).await;

    let list = devices(&scenario).await;
    assert_eq!(list.len(), 3, "bob, carol, the client: {list:?}");
    for (user, device) in [
        ("bob-uuid@example.test", "bobdev01"),
        ("carol-uuid@example.test", "caroldev"),
        ("user-uuid@example.test", "new-client-id"),
    ] {
        let entry = basic(user, device, &list);
        assert_eq!(entry.credential, DeviceCredential::Basic);
        let thumbprint = entry.thumbprint.expect("a P-256 leaf has a thumbprint");
        assert_eq!(thumbprint.len(), 43, "base64url SHA-256, no padding");
    }
    assert_eq!(
        basic("user-uuid@example.test", "new-client-id", &list).thumbprint,
        Some(
            scenario
                .client
                .own_device_thumbprint()
                .expect("own thumbprint")
        ),
        "the own entry is this device"
    );
}

/// The first observation of a room is a silent baseline, persisted.
#[tokio::test]
async fn joining_records_a_silent_persisted_baseline() {
    let recorder = Arc::new(ChangeRecorder::default());
    let (scenario, _sub) = joined_scenario(&recorder).await;

    assert!(
        recorder.changes().is_empty(),
        "no change on the first observation"
    );
    let mut persisted = scenario
        .store
        .load_room_devices(&room_id())
        .await
        .expect("load baseline")
        .expect("a baseline was persisted");
    let mut listed = devices(&scenario).await;
    persisted.sort();
    listed.sort();
    assert_eq!(persisted, listed);
}

/// A device bob adds is reported once, as `added`.
#[tokio::test]
async fn a_peer_adding_a_device_emits_a_change() {
    let recorder = Arc::new(ChangeRecorder::default());
    let (mut scenario, _sub) = joined_scenario(&recorder).await;
    let dave = PeerDevice::new(DAVE_IDENTITY);
    let (commit, _welcome) = scenario.bob.add_member(&dave.key_package_tls());

    sync_next_page(
        &mut scenario,
        "95d0e858-8eae-11ef-0000-000000000002",
        &[message_event(&commit)],
    )
    .await;

    let changes = recorder.changes();
    assert_eq!(changes.len(), 1, "one change: {changes:?}");
    assert_eq!(changes[0].room_id, room_id());
    assert!(changes[0].removed.is_empty());
    assert_eq!(changes[0].added.len(), 1);
    assert_eq!(
        changes[0].added[0].user_id.as_deref(),
        Some("dave-uuid@example.test")
    );
    assert_eq!(changes[0].added[0].device_id.as_deref(), Some("davedev"));
}

/// A device bob removes is reported once, as `removed`.
#[tokio::test]
async fn a_peer_removing_a_device_emits_a_change() {
    let recorder = Arc::new(ChangeRecorder::default());
    let (mut scenario, _sub) = joined_scenario(&recorder).await;
    let carol_leaf = scenario.bob.leaf_index_of(CAROL_IDENTITY);
    let commit = scenario.bob.remove_member(carol_leaf);

    sync_next_page(
        &mut scenario,
        "95d0e858-8eae-11ef-0000-000000000003",
        &[message_event(&commit)],
    )
    .await;

    let changes = recorder.changes();
    assert_eq!(changes.len(), 1, "one change: {changes:?}");
    assert!(changes[0].added.is_empty());
    assert_eq!(
        changes[0]
            .removed
            .iter()
            .map(|d| d.user_id.as_deref())
            .collect::<Vec<_>>(),
        vec![Some("carol-uuid@example.test")]
    );
}

/// Application traffic changes no device — no event.
#[tokio::test]
async fn an_application_message_emits_no_change() {
    let recorder = Arc::new(ChangeRecorder::default());
    let (mut scenario, _sub) = joined_scenario(&recorder).await;
    let text = scenario
        .bob
        .application_message(&text_message("m-bob", "hello"));

    sync_next_page(
        &mut scenario,
        "95d0e858-8eae-11ef-0000-000000000004",
        &[message_event(&text)],
    )
    .await;

    assert!(recorder.changes().is_empty());
}

/// An e2ei X.509 leaf is listed (kind X.509, identity unparsed,
/// thumbprint present) — never an error.
#[tokio::test]
async fn an_x509_leaf_is_listed_not_an_error() {
    let server = MockServer::start();
    let (client, _store) = logged_in_client(&server).await;
    mount_room(&server);
    let x509 = openmls::prelude::Credential::new(
        openmls::prelude::CredentialType::X509,
        b"fixture certificate chain".to_vec(),
    );
    let mut peer = PeerGroup::new_with_credential(GROUP_ID, x509);
    let (_commit, welcome) = peer.add_member(&captured_key_package_for(&server));
    mount_notifications_page(
        &server,
        None,
        "95d0e858-8eae-11ef-0000-000000000005",
        &[welcome_event(&welcome)],
    );
    client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync_once");

    let list = client
        .room(&room_id())
        .await
        .expect("room cached")
        .devices()
        .await
        .expect("an X.509 leaf is not an error");
    let x509_entry = list
        .iter()
        .find(|d| d.credential == DeviceCredential::X509)
        .expect("the X.509 leaf is listed");
    assert_eq!(x509_entry.user_id, None);
    assert_eq!(x509_entry.device_id, None);
    assert!(x509_entry.thumbprint.is_some());
}

/// Live-smoke finding: a room without a persisted baseline (joined by a
/// pre-95d build, or created mid-session) must get one BEFORE the next
/// drain — otherwise its first real change becomes the silent baseline
/// and is never reported.
#[tokio::test]
async fn a_room_without_a_baseline_gets_one_before_the_next_drain() {
    let recorder = Arc::new(ChangeRecorder::default());
    let (mut scenario, _sub) = joined_scenario(&recorder).await;
    // Simulate a room that predates Step 95d: no baseline on disk.
    scenario
        .store
        .replace_room_devices(&room_id(), &[])
        .await
        .expect("clear baseline");
    let dave = PeerDevice::new(DAVE_IDENTITY);
    let (commit, _welcome) = scenario.bob.add_member(&dave.key_package_tls());

    sync_next_page(
        &mut scenario,
        "95d0e858-8eae-11ef-0000-000000000006",
        &[message_event(&commit)],
    )
    .await;

    let changes = recorder.changes();
    assert_eq!(changes.len(), 1, "dave's arrival is reported: {changes:?}");
    assert_eq!(
        changes[0].added[0].user_id.as_deref(),
        Some("dave-uuid@example.test")
    );
}
