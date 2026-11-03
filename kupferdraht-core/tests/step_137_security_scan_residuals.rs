// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 137 — security-scan residuals (TDD red).
//!
//! The pins with a consumer-visible surface live here; the ones on
//! crate-private functions sit beside them (`wire/http/mod.rs` for the
//! path segments and the error-body cap, `wire/http/auth.rs` for the
//! rebind's body, `storage/remote/token_exchange.rs` for the redirect
//! policy and its body) and the CLI's in `kupferdraht-cli` (`main.rs`,
//! `cli/mod.rs`).
//!
//! * A case snapshot in the same drain as the Welcome that opened its
//!   room reaches the store and is authorized per the refreshed room
//!   list (M3). Reproduced red: the message dies before the ADR-0018
//!   admin check — `decrypt_to_application` maps the room to its group
//!   through the cache the Welcome only marked stale, the drain reports
//!   `no local group_id` and queues a heal of the room it just joined.
//! * `docs/cli.md` documents `--cookie-stdin` and `--key-stdin` and no
//!   longer shows either secret as an argument (M1).
//!
//! The harness is the Step-95a shape: a fixture peer (`PeerGroup`)
//! invites the client via Welcome and sends through the full receive
//! pipe; `wire::proto` is crate-private, so the protobuf bytes are
//! hand-emitted (the Step-72b precedent).

use std::path::Path;
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use httpmock::Method::{GET, POST};
use httpmock::{Mock, MockServer, prelude::HttpMockRequest};
use kupferdraht::storage::Store;
use kupferdraht::{ClientBuilder, EventHandler, Message, SyncSettings};

mod common;

const ROOM_UUID: &str = "13700aa1-77aa-4bde-9d10-223344556677";
const DOMAIN: &str = "example.test";
/// b64("grp-137") — the room's MLS group id.
const GROUP_ID_B64: &str = "Z3JwLTEzNw==";
const GROUP_ID: &[u8] = b"grp-137";
const BOB: &str = "bob-uuid";
const BOB_IDENTITY: &[u8] = b"bob-uuid:bobdev01@example.test";

fn room_id() -> String {
    format!("{ROOM_UUID}@{DOMAIN}")
}

/// Read a file at the workspace root (one level above the
/// `kupferdraht-core` manifest).
fn workspace_file(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// The `### <heading>` section of a Markdown document up to the next
/// `### ` heading.
fn section<'a>(doc: &'a str, heading: &str) -> &'a str {
    let start = doc
        .find(heading)
        .unwrap_or_else(|| panic!("section {heading:?} missing"));
    let rest = &doc[start + heading.len()..];
    let end = rest.find("\n### ").unwrap_or(rest.len());
    &rest[..end]
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

// ---- mocks (Step-69 / 72b / 95a shapes) ----

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

/// An account with no conversations and no users to resolve. The mocks
/// are returned so a test can take them down again.
fn mount_empty_room_list(server: &MockServer) -> Vec<Mock<'_>> {
    vec![
        server.mock(|when, then| {
            when.method(POST).path("/v14/conversations/list-ids");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"qualified_conversations":[],"has_more":false,"paging_state":"END"}"#);
        }),
        server.mock(|when, then| {
            when.method(POST).path("/v14/conversations/list");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"found":[],"not_found":[],"failed":[]}"#);
        }),
        server.mock(|when, then| {
            when.method(POST).path("/v14/list-users");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"found":[]}"#);
        }),
    ]
}

/// Room metadata listing bob with the given role.
fn mount_room(server: &MockServer, bob_role: &str) {
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
             "name":"Cold cache",
             "members":{{
               "self":{{"qualified_id":{{"id":"user-uuid","domain":"{DOMAIN}"}},
                       "conversation_role":"wire_member"}},
               "others":[
                 {{"qualified_id":{{"id":"{BOB}","domain":"{DOMAIN}"}},
                   "conversation_role":"{bob_role}"}}
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

fn mount_empty_notifications(server: &MockServer) -> Mock<'_> {
    server.mock(|when, then| {
        when.method(GET).path("/v14/notifications");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"notifications":[],"has_more":false}"#);
    })
}

fn mount_notifications_page<'a>(server: &'a MockServer, events: &[String]) -> Mock<'a> {
    let payload = events.join(",");
    let body = format!(
        r#"{{"notifications":[{{"id":"13700e58-8eae-11ef-0000-00000000cafe","payload":[{payload}]}}],"has_more":false}}"#
    );
    server.mock(move |when, then| {
        when.method(GET).path("/v14/notifications");
        then.status(200)
            .header("content-type", "application/json")
            .body(body.clone());
    })
}

fn welcome_event(data_b64: &str) -> String {
    format!(
        r#"{{"type":"conversation.mls-welcome",
             "qualified_conversation":{{"id":"{ROOM_UUID}","domain":"{DOMAIN}"}},
             "qualified_from":{{"id":"{BOB}","domain":"{DOMAIN}"}},
             "data":"{data_b64}",
             "time":"2026-11-02T10:00:00.000Z"}}"#
    )
}

/// The envelope the backend writes around bob's ciphertext.
fn message_event(data_b64: &str) -> String {
    format!(
        r#"{{"type":"conversation.mls-message-add",
             "qualified_conversation":{{"id":"{ROOM_UUID}","domain":"{DOMAIN}"}},
             "qualified_from":{{"id":"{BOB}","domain":"{DOMAIN}"}},
             "from_client":"bobdev01",
             "data":"{data_b64}",
             "time":"2026-11-02T10:00:01.000Z"}}"#
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

/// Bob's Welcome for this client and his case snapshot, in one page.
fn mount_welcome_and_case<'a>(server: &'a MockServer, message_id: &str, title: &str) -> Mock<'a> {
    let b64 = |bytes: Vec<u8>| base64::engine::general_purpose::STANDARD.encode(bytes);
    let mut peer = common::mls::PeerGroup::new(GROUP_ID, BOB_IDENTITY);
    let welcome = b64(peer.welcome_over_key_package(&captured_key_package_for(server)));
    let case = b64(peer.application_message(&case_generic_message(message_id, title)));
    mount_notifications_page(server, &[welcome_event(&welcome), message_event(&case)])
}

/// What the drain reports through `on_sync_error`, and the rooms it
/// announced through `on_room_invited`.
#[derive(Default)]
struct SyncErrors {
    reported: Mutex<Vec<String>>,
    invited: Mutex<Vec<String>>,
}

impl SyncErrors {
    fn reported(&self) -> Vec<String> {
        self.reported.lock().expect("errors lock").clone()
    }

    fn invited(&self) -> Vec<String> {
        self.invited.lock().expect("invited lock").clone()
    }
}

impl EventHandler for SyncErrors {
    fn on_message(&self, _message: Message) {}

    fn on_room_invited(&self, room_id: String) {
        self.invited.lock().expect("invited lock").push(room_id);
    }

    fn on_sync_error(&self, description: String) {
        self.reported.lock().expect("errors lock").push(description);
    }
}

async fn sync(client: &kupferdraht::Client) {
    client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync_once");
}

// ---- tests ----

/// Pass one syncs an account with no rooms (the list is fresh afterwards,
/// so pass two does not refresh it before the drain); the mocks of pass
/// one are gone when it returns, so pass two is the caller's to mount.
async fn cold_client(server: &MockServer) -> (Arc<kupferdraht::Client>, Arc<dyn Store>) {
    let mut empty_list = mount_empty_room_list(server);
    let (client, store) = logged_in_client(server).await;
    let mut no_events = mount_empty_notifications(server);
    sync(&client).await;
    assert!(client.rooms().await.is_empty(), "pass one: no rooms yet");
    for mock in empty_list.iter_mut().chain(std::iter::once(&mut no_events)) {
        mock.delete();
    }
    (client, store)
}

/// Pass two: bob's Welcome and his case snapshot in one page, bob with
/// `bob_role` in the room the backend lists by then. Returns what the
/// drain reported and whether the snapshot row is stored.
async fn drain_welcome_and_case_cold(
    server: &MockServer,
    client: &kupferdraht::Client,
    store: &Arc<dyn Store>,
    bob_role: &str,
) -> (Arc<SyncErrors>, bool) {
    mount_room(server, bob_role);
    mount_welcome_and_case(server, "case-cold", "Cold room");
    let events = Arc::new(SyncErrors::default());
    client.add_event_handler(events.clone());
    sync(client).await;
    let stored = store
        .load_timeline_events(&room_id())
        .await
        .expect("load rows")
        .into_iter()
        .any(|r| r.message_id == "case-cold");
    (events, stored)
}

/// M3: a room the drain's own Welcome has just opened is known to the
/// rest of the drain — the message after the Welcome decrypts, reaches
/// the store, and the ADR-0018 admin check reads the refreshed list:
/// with bob a `wire_admin`, nothing is reported, the row is stored, the
/// snapshot folds.
#[tokio::test]
async fn a_case_snapshot_in_the_drain_that_welcomed_its_room_is_authorized_per_the_refreshed_list()
{
    let server = MockServer::start();
    let (client, store) = cold_client(&server).await;

    let (events, stored) =
        drain_welcome_and_case_cold(&server, &client, &store, "wire_admin").await;

    assert!(
        events.reported().is_empty(),
        "the drain reported: {:?}",
        events.reported()
    );
    assert!(stored, "the snapshot row is stored under the welcomed room");
    let room = client
        .room(&room_id())
        .await
        .expect("the end-of-cycle refresh cached the room");
    assert!(
        room.case().await.expect("case read").is_some(),
        "an admin's snapshot must fold although the cache was cold when it arrived"
    );
}

/// The negative of the same drain: with bob a `wire_member`, the refreshed
/// list is what the ADR-0018 check reads — the row is stored, the snapshot
/// does not fold. A refresh that failed open (an unknown room treated as
/// authorized) would fold it.
#[tokio::test]
async fn a_members_case_snapshot_in_the_drain_that_welcomed_its_room_is_stored_but_not_folded() {
    let server = MockServer::start();
    let (client, store) = cold_client(&server).await;

    let (_events, stored) =
        drain_welcome_and_case_cold(&server, &client, &store, "wire_member").await;

    assert!(
        stored,
        "a member's snapshot row is stored (ADR-0018: stored, never folded)"
    );
    let room = client
        .room(&room_id())
        .await
        .expect("the end-of-cycle refresh cached the room");
    assert!(
        room.case().await.expect("case read").is_none(),
        "a member's snapshot must not fold, cold cache or not"
    );
}

/// The refresh after the Welcome fails (the list endpoint is down): the
/// Welcome stays absorbed and announced, the drain goes on, the message
/// after it is reported as before Step 137 (not lost silently), the
/// refresh is retried at the end of the cycle, and the next pass with the
/// list back caches the room.
#[tokio::test]
async fn a_failed_refresh_after_the_welcome_keeps_the_welcome_and_retries_the_list() {
    let server = MockServer::start();
    let (client, _store) = cold_client(&server).await;
    let mut list_down = server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list-ids");
        then.status(500).body("{}");
    });
    let mut page = mount_welcome_and_case(&server, "case-down", "List down");
    let events = Arc::new(SyncErrors::default());
    client.add_event_handler(events.clone());

    let outcome = client.sync_once(SyncSettings::default()).await;

    assert_eq!(
        events.invited(),
        vec![room_id()],
        "the Welcome was absorbed and announced although the refresh failed"
    );
    assert!(
        events.reported().iter().any(|e| e.contains(&room_id())),
        "the message after it is reported, as before Step 137 (outcome {outcome:?}): {:?}",
        events.reported()
    );
    assert_eq!(
        list_down.calls(),
        2,
        "refreshed right after the Welcome, retried at the end of the cycle"
    );

    list_down.delete();
    page.delete();
    mount_room(&server, "wire_admin");
    let _quiet = mount_empty_notifications(&server);
    sync(&client).await;
    assert!(
        client.room(&room_id()).await.is_some(),
        "the next pass refreshed the list the failed cycle left stale"
    );
}

/// M1: `docs/cli.md` shows the cookie and the link's key read from stdin
/// or the prompt, never as an argument.
#[test]
fn cli_doc_reads_the_cookie_and_the_link_key_from_stdin_or_the_prompt() {
    let doc = workspace_file("docs/cli.md");
    let finalize = section(&doc, "### `sso finalize`");
    assert!(
        finalize.contains("--cookie-stdin"),
        "sso finalize documents --cookie-stdin"
    );
    assert!(
        !finalize.contains("--cookie <"),
        "the cookie is no longer an argument"
    );
    let join = section(&doc, "### join-via-link");
    assert!(
        join.contains("--key-stdin"),
        "join-via-link documents --key-stdin"
    );
    assert!(
        !join.contains("--key <"),
        "the key is no longer an argument"
    );
}
