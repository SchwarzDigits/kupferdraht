// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 95c — no panics on foreign data. TDD red.
//!
//! Data a room member, the backend or a corrupted row can shape must
//! fail as a recoverable, typed error — never as an `assert!` /
//! `debug_assert!` panic that kills the sync task (CLAUDE.md
//! "Assertions"). Each test feeds one hostile input through the real
//! receive pipe (or `resume`) and pins that the client keeps working.
//! `wire::proto` is crate-private (ADR-0009): protobuf bytes are
//! hand-emitted (the Step-72b precedent).

use std::sync::{Arc, Mutex};

use base64::Engine as _;
use httpmock::Method::{GET, POST};
use httpmock::{MockServer, prelude::HttpMockRequest};
use kupferdraht::keystore::{InMemoryKeyStore, SecureKeyStore};
use kupferdraht::storage::Store;
use kupferdraht::{ClientBuilder, Kupferdraht, KupferdrahtError, SyncSettings};

mod common;

use common::mls::PeerGroup;

const DOMAIN: &str = "example.test";
const ROOM_UUID: &str = "95c0aa31-77aa-4bde-9d10-223344556677";
/// b64("grp-95c") — the room's MLS group id.
const GROUP_ID_B64: &str = "Z3JwLTk1Yw==";
const GROUP_ID: &[u8] = b"grp-95c";
const BOB: &str = "bob-uuid";
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

/// `GenericMessage { message_id = 1, task = 262000 { task_id = 1, title = 2, status = 3 } }`.
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

/// `GenericMessage { message_id = 1, ephemeral = 18 { expire_after_millis = 1, text = 2 } }`
/// with a raw varint TTL (an `int64` field: `i64::MAX` fits a varint).
fn ephemeral_text_message(message_id: &str, content: &str, ttl_raw: u64) -> Vec<u8> {
    let mut ephemeral = Vec::new();
    put_uint(1, ttl_raw, &mut ephemeral);
    put_len_delimited(2, &text_payload(content), &mut ephemeral);
    let mut m = Vec::new();
    put_len_delimited(1, message_id.as_bytes(), &mut m);
    put_len_delimited(18, &ephemeral, &mut m);
    m
}

/// `GenericMessage { message_id = 1, text = 2 { content = 1, mentions = 4 {
/// start = 1, length = 2, qualified_user_id = 4 { id = 1, domain = 2 } } } }`.
fn text_with_mention(message_id: &str, content: &str, id: &str, domain: &str) -> Vec<u8> {
    let mut qualified = Vec::new();
    put_len_delimited(1, id.as_bytes(), &mut qualified);
    put_len_delimited(2, domain.as_bytes(), &mut qualified);
    let mut mention = Vec::new();
    put_uint(1, 0, &mut mention);
    put_uint(2, 3, &mut mention);
    put_len_delimited(4, &qualified, &mut mention);
    let mut text = text_payload(content);
    put_len_delimited(4, &mention, &mut text);
    let mut m = Vec::new();
    put_len_delimited(1, message_id.as_bytes(), &mut m);
    put_len_delimited(2, &text, &mut m);
    m
}

/// `GenericMessage { message_id = 1, case = 262001 { revision = 1,
/// case_ref = 2, state = 6 } }`.
fn case_message(message_id: &str, case_ref: &str, revision: u64) -> Vec<u8> {
    let mut payload = Vec::new();
    put_uint(1, revision, &mut payload);
    put_len_delimited(2, case_ref.as_bytes(), &mut payload);
    put_uint(6, 0 /* OPEN */, &mut payload);
    let mut m = Vec::new();
    put_len_delimited(1, message_id.as_bytes(), &mut m);
    put_len_delimited(262_001, &payload, &mut m);
    m
}

// ---- mocks (Step-69 / 95a shapes) ----

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

/// Every key package `register_device` uploaded to `server`, decoded.
fn captured_key_packages_for(server: &MockServer) -> Vec<Vec<u8>> {
    let host = server.address().to_string();
    let guard = UPLOADED_KPS.lock().expect("capture lock");
    let (_, kps) = guard
        .iter()
        .rev()
        .find(|(h, _)| *h == host)
        .expect("register_device uploaded key packages");
    kps.iter()
        .map(|kp| {
            base64::engine::general_purpose::STANDARD
                .decode(kp)
                .expect("uploaded key package is base64")
        })
        .collect()
}

const MEMBER: &str = "wire_member";
const ADMIN: &str = "wire_admin";

fn mount_room(server: &MockServer) {
    mount_room_with_roles(server, MEMBER, MEMBER);
}

/// The one-room metadata, Alice as `self_role`, Bob as `bob_role`.
fn mount_room_with_roles(server: &MockServer, self_role: &str, bob_role: &str) {
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
                     "name":"Hostile",
                     "members":{{
                       "self":{{"qualified_id":{{"id":"user-uuid","domain":"{DOMAIN}"}},
                               "conversation_role":"{self_role}"}},
                       "others":[{{"qualified_id":{{"id":"{BOB}","domain":"{DOMAIN}"}},
                                   "conversation_role":"{bob_role}"}}]
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
    let body = format!(
        r#"{{"notifications":[{{"id":"95c0e858-8eae-11ef-0000-000000000001","payload":[{}]}}],"has_more":false}}"#,
        events.join(",")
    );
    server.mock(move |when, then| {
        when.method(GET).path("/v14/notifications");
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

/// `from_id` lands in `qualified_from.id` verbatim — a backend can put
/// anything there.
fn message_event(from_id: &str, data: &[u8]) -> String {
    format!(
        r#"{{"type":"conversation.mls-message-add",
             "qualified_conversation":{{"id":"{ROOM_UUID}","domain":"{DOMAIN}"}},
             "qualified_from":{{"id":"{from_id}","domain":"{DOMAIN}"}},
             "data":"{}",
             "time":"2026-09-26T10:00:01.000Z"}}"#,
        base64::engine::general_purpose::STANDARD.encode(data)
    )
}

fn property_set_event(key: &str) -> String {
    format!(r#"{{"type":"user.properties-set","key":"{key}","value":true}}"#)
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
    let keystore: Arc<dyn SecureKeyStore> = Arc::new(InMemoryKeyStore::new());
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

/// Login, the Welcome, then `events(bob)` in one page; one `sync_once`
/// that must return — not panic. The `MockServer` is returned so the
/// mocks outlive the client (the 95b pattern — no leaked pooled server).
async fn sync_hostile_page(
    events: impl FnOnce(&mut PeerGroup) -> Vec<String>,
) -> (MockServer, Arc<kupferdraht::Client>, Arc<dyn Store>) {
    sync_hostile_page_with_roles((MEMBER, MEMBER), events).await
}

async fn sync_hostile_page_with_roles(
    (self_role, bob_role): (&str, &str),
    events: impl FnOnce(&mut PeerGroup) -> Vec<String>,
) -> (MockServer, Arc<kupferdraht::Client>, Arc<dyn Store>) {
    let server = MockServer::start();
    let (client, store) = logged_in_client(&server).await;
    mount_room_with_roles(&server, self_role, bob_role);
    let mut bob = PeerGroup::new(GROUP_ID, BOB_IDENTITY);
    let (_commit, welcome) = bob.add_member(&captured_key_package_for(&server));
    let mut page = vec![welcome_event(&welcome)];
    page.extend(events(&mut bob));
    mount_notifications_page(&server, &page);
    client
        .sync_once(SyncSettings::default())
        .await
        .expect("a hostile page is not a sync failure");
    (server, client, store)
}

async fn row_exists(store: &Arc<dyn Store>, message_id: &str) -> bool {
    store
        .load_timeline_events(&room_id())
        .await
        .expect("load rows")
        .iter()
        .any(|r| r.message_id == message_id)
}

// ---- tests ----

/// Any room member: a `TaskPayload` with an empty `task_id` (proto2
/// `required` is not enforced by prost) used to hit `assert!` in the
/// task-cache upsert — the sync task died. Now the payload is invalid at
/// the decode boundary: dropped, and the drain goes on.
#[tokio::test]
async fn an_empty_task_id_is_dropped_and_the_drain_continues() {
    let (_server, client, store) = sync_hostile_page(|bob| {
        vec![
            message_event(
                BOB,
                &bob.application_message(&task_message("task-bad", "", "no id")),
            ),
            message_event(
                BOB,
                &bob.application_message(&text_message("m-after", "still here")),
            ),
        ]
    })
    .await;

    let room = client.room(&room_id()).await.expect("room cached");
    assert!(room.tasks().await.expect("tasks").is_empty());
    assert!(
        row_exists(&store, "m-after").await,
        "the message after the hostile one is dispatched"
    );
}

/// Any room member: `expire_after_millis = i64::MAX` used to overflow
/// `received_at_ms + ttl` (a panic in debug builds, a wrapped past
/// stamp in release). The timer is clamped to Wire's range (≤ 1 year).
#[tokio::test]
async fn an_ephemeral_ttl_of_i64_max_is_clamped_not_overflowed() {
    let (_server, _client, store) = sync_hostile_page(|bob| {
        vec![message_event(
            BOB,
            &bob.application_message(&ephemeral_text_message(
                "eph-max",
                "forever?",
                u64::try_from(i64::MAX).expect("fits"),
            )),
        )]
    })
    .await;

    let row = store
        .load_timeline_events(&room_id())
        .await
        .expect("load rows")
        .into_iter()
        .find(|r| r.message_id == "eph-max")
        .expect("the ephemeral message is stored");
    let expires = row.expires_at_ms.expect("an expiry stamp");
    let year_ms = 365 * 24 * 60 * 60 * 1000_i64;
    assert!(
        expires > row.sent_at_ms && expires <= row.sent_at_ms + year_ms + 60_000,
        "clamped into (received, received + 1 year]: {expires} vs {}",
        row.sent_at_ms
    );
}

/// The backend: an empty `qualified_from.id` used to reach
/// `UserId::new`'s `debug_assert!` while parsing the envelope. It is a
/// malformed envelope field now — treated as absent.
#[tokio::test]
async fn an_empty_envelope_user_id_does_not_panic() {
    let (_server, _client, store) = sync_hostile_page(|bob| {
        vec![message_event(
            "",
            &bob.application_message(&text_message("m-anon", "who?")),
        )]
    })
    .await;

    assert!(
        row_exists(&store, "m-anon").await,
        "attributed by the MLS leaf (ADR-0027)"
    );
}

/// A session whose tokens were blanked after `SessionExpired` (Step 39)
/// used to hit `resume`'s `debug_assert!` on the empty refresh token
/// (the 2026-08-13 incident class). It resumes to a typed
/// `SessionExpired` — the re-login gate — not a panic.
#[tokio::test]
async fn resume_after_the_session_expired_is_a_typed_error() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST)
            .path_includes("/v14/mls/key-packages/self/");
        then.status(201);
    });
    common::mount_full_login_flow(&server);
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
    let account = client.user_id().expect("user id").as_str().to_string();
    let store = client.store().expect("store");
    let mut session = store
        .load_session(&account)
        .await
        .expect("load session")
        .expect("a persisted session");
    session.access_token = String::new();
    session.refresh_token = String::new();
    store
        .save_session(&session)
        .await
        .expect("blank the tokens");
    drop(client);

    let resumed = ClientBuilder::new()
        .permit_http()
        .base_dir(home.path().to_path_buf())
        .keystore(keystore)
        .account(account.as_str())
        .resume()
        .await;

    assert!(
        matches!(resumed, Err(KupferdrahtError::SessionExpired)),
        "a dead session resumes to the re-login gate: {:?}",
        resumed.map(|_| "a client")
    );
}

/// A Welcome can come from anyone holding one of our key packages (or a
/// hostile backend): one whose group id is empty used to hit `assert!`
/// in `absorb_welcome` — while holding the MLS mutex, poisoning it for
/// every later MLS operation of the session. It is a rejected Welcome
/// now: a valid Welcome and a message later in the same page still land.
#[tokio::test]
async fn an_empty_group_id_welcome_is_rejected_without_poisoning_mls() {
    let server = MockServer::start();
    let (_client_keepalive, store) = {
        let (client, store) = logged_in_client(&server).await;
        mount_room(&server);
        let kps = captured_key_packages_for(&server);
        assert!(kps.len() >= 2, "two distinct key packages to spend");
        let hostile =
            common::mls::welcome_over_key_package(b"", b"mallory:m@example.test", &kps[0]);
        let mut bob = PeerGroup::new(GROUP_ID, BOB_IDENTITY);
        let (_commit, welcome) = bob.add_member(&kps[1]);
        mount_notifications_page(
            &server,
            &[
                welcome_event(&hostile),
                welcome_event(&welcome),
                message_event(
                    BOB,
                    &bob.application_message(&text_message("m-post", "after the hostile welcome")),
                ),
            ],
        );
        client
            .sync_once(SyncSettings::default())
            .await
            .expect("a hostile Welcome is not a sync failure");
        (client, store)
    };

    assert!(
        row_exists(&store, "m-post").await,
        "MLS still works after the rejected Welcome"
    );
}

/// Any room member: a mention whose `qualified_user_id` is empty used to
/// reach `UserId::new`'s `debug_assert!`s. The mention is dropped; the
/// message itself lands.
#[tokio::test]
async fn a_mention_with_an_empty_user_id_is_dropped_not_asserted() {
    let (_server, _client, store) = sync_hostile_page(|bob| {
        vec![message_event(
            BOB,
            &bob.application_message(&text_with_mention("m-mention", "hi @x", "", "")),
        )]
    })
    .await;

    assert!(row_exists(&store, "m-mention").await);
    assert!(
        !store
            .load_mentions_for_room(&room_id())
            .await
            .expect("load mentions")
            .iter()
            .any(|m| m.message_id == "m-mention"),
        "an unaddressable mention is not stored"
    );
}

/// A room admin: a case snapshot with an empty `case_ref` (proto2
/// `required`, not enforced by prost) used to be folded as the room's
/// case — and the next `set_case` / invite re-broadcast hit `assert!` in
/// the case builder. It is invalid at the decode boundary now.
#[tokio::test]
async fn a_case_snapshot_without_a_case_ref_is_dropped() {
    let (_server, client, _store) = sync_hostile_page_with_roles((MEMBER, ADMIN), |bob| {
        vec![message_event(
            BOB,
            &bob.application_message(&case_message("case-bad", "", 1)),
        )]
    })
    .await;

    let room = client.room(&room_id()).await.expect("room cached");
    assert!(room.case().await.expect("case").is_none());
}

/// A room admin: revision `u64::MAX` used to overflow the next local
/// `set_case` (`revision + 1` — a panic in debug builds, a wrap to 0 in
/// release, which every receiver's fold then ignores). The update is
/// refused with a typed error instead.
#[tokio::test]
async fn a_case_at_the_maximum_revision_refuses_the_next_update() {
    let (_server, client, _store) = sync_hostile_page_with_roles((ADMIN, ADMIN), |bob| {
        vec![message_event(
            BOB,
            &bob.application_message(&case_message("case-max", "case-1", u64::MAX)),
        )]
    })
    .await;

    let room = client.room(&room_id()).await.expect("room cached");
    let current = room
        .case()
        .await
        .expect("case")
        .expect("the admin case folds");
    assert_eq!(current.revision, u64::MAX);
    let err = room
        .set_case(kupferdraht::timeline::CasePatch {
            title: Some("next".into()),
            ..kupferdraht::timeline::CasePatch::default()
        })
        .await
        .expect_err("no revision after u64::MAX");
    assert!(
        matches!(err, KupferdrahtError::InvalidInput(_)),
        "got {err:?}"
    );
}

/// The backend: an `mls-message-add` with empty `data` used to reach the
/// decryptor's `debug_assert!`. It is a malformed event — skipped — and
/// the next message in the page lands.
#[tokio::test]
async fn an_empty_mls_message_is_skipped() {
    let (_server, _client, store) = sync_hostile_page(|bob| {
        vec![
            message_event(BOB, b""),
            message_event(
                BOB,
                &bob.application_message(&text_message("m-next", "still here")),
            ),
        ]
    })
    .await;

    assert!(row_exists(&store, "m-next").await);
}

/// The backend: a `user.properties-set` key goes into the
/// `GET /properties/{key}` path. An empty key used to hit a
/// `debug_assert!`; `../self` steered the authenticated GET elsewhere.
/// Both are skipped before any request.
#[tokio::test]
async fn hostile_property_keys_are_skipped_before_any_request() {
    let server = MockServer::start();
    let fetch = server.mock(|when, then| {
        when.method(GET).path_includes("/properties");
        then.status(200)
            .header("content-type", "application/json")
            .body("true");
    });
    let (client, store) = logged_in_client(&server).await;
    mount_room(&server);
    let mut bob = PeerGroup::new(GROUP_ID, BOB_IDENTITY);
    let (_commit, welcome) = bob.add_member(&captured_key_package_for(&server));
    let baseline = fetch.calls();
    mount_notifications_page(
        &server,
        &[
            welcome_event(&welcome),
            property_set_event(""),
            property_set_event("../self"),
            message_event(
                BOB,
                &bob.application_message(&text_message("m-props", "after the keys")),
            ),
        ],
    );
    client
        .sync_once(SyncSettings::default())
        .await
        .expect("hostile keys are not a sync failure");

    assert_eq!(
        fetch.calls(),
        baseline,
        "no property fetch for a hostile key"
    );
    assert!(row_exists(&store, "m-props").await);
}
