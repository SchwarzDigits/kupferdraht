// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 72b — case fold through the real receive pipe (TDD red).
//!
//! A fixture peer (`common::mls::PeerGroup`) invites the client via
//! Welcome (the Step-69 machinery) and then sends `CasePayload`
//! application messages through the full dispatch: MLS decrypt →
//! `GenericMessage` field 262001 → persist (`kind='case'`) → fold.
//! The admin gate keys off the peer's `conversation_role` in the
//! room metadata (Step 72a); the fold itself is pinned per rule in
//! `timeline::case::fold` unit tests — here the pipe end-to-end:
//! LWW by revision, stored-but-not-folded for non-admins, and the
//! redaction re-fold.
//!
//! `wire::proto` is crate-private (ADR-0009), so the test hand-emits
//! the protobuf bytes (varint/len-delimited — the `prekeys.rs` CBOR
//! precedent): `GenericMessage { message_id = 1, case = 262001 }`,
//! `CasePayload { revision = 1, case_ref = 2, title = 4, state = 6 }`.

use std::sync::{Arc, Mutex};

use base64::Engine as _;
use httpmock::Method::{GET, POST};
use httpmock::{MockServer, prelude::HttpMockRequest};
use kupferdraht::storage::Store;
use kupferdraht::timeline::CaseState;
use kupferdraht::{ClientBuilder, SyncSettings};

mod common;

const ROOM_UUID: &str = "5c22fa31-77aa-4bde-9d10-223344556677";
const DOMAIN: &str = "example.test";
/// b64("grp-case") — the room's MLS group id.
const GROUP_ID_B64: &str = "Z3JwLWNhc2U=";
const BOB: &str = "bob-uuid";
const BOB_IDENTITY: &[u8] = b"bob-uuid:bobdev01@example.test";

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

/// `CasePayload { revision, case_ref, title, state }` — the minimal
/// core the fold keys off (state raw value per the proto enum:
/// 0 = OPEN … 5 = CANCELLED).
fn case_payload_bytes(revision: u64, case_ref: &str, title: &str, state_raw: u64) -> Vec<u8> {
    let mut p = Vec::new();
    put_uint(1, revision, &mut p);
    put_len_delimited(2, case_ref.as_bytes(), &mut p);
    put_len_delimited(4, title.as_bytes(), &mut p);
    put_uint(6, state_raw, &mut p);
    p
}

/// `GenericMessage { message_id = 1, case = 262001 }` — no content
/// oneof (cases carry no text fallback, ADR-0018).
fn case_generic_message(message_id: &str, payload: &[u8]) -> Vec<u8> {
    let mut m = Vec::new();
    put_len_delimited(1, message_id.as_bytes(), &mut m);
    put_len_delimited(262_001, payload, &mut m);
    m
}

// ---- mocks (Step-69 shapes) ----

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

/// Room metadata listing bob with the given `conversation_role`.
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
    let bob_role_json = format!(r#","conversation_role":"{bob_role}""#);
    server.mock(move |when, then| {
        when.method(POST).path("/v14/conversations/list");
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(
                r#"{{"found":[{{
                     "qualified_id":{{"id":"{ROOM_UUID}","domain":"{DOMAIN}"}},
                     "type":0,"protocol":"mls","epoch":1,"group_id":"{GROUP_ID_B64}",
                     "name":"Cases",
                     "members":{{
                       "self":{{"qualified_id":{{"id":"user-uuid","domain":"{DOMAIN}"}},
                               "conversation_role":"wire_admin"}},
                       "others":[
                         {{"qualified_id":{{"id":"{BOB}","domain":"{DOMAIN}"}}{bob_role_json}}}
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
        r#"{{"notifications":[{{"id":"9100e858-8eae-11ef-0000-00000000cafe","payload":[{payload}]}}],"has_more":false}}"#
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
             "time":"2026-08-21T10:00:00.000Z"}}"#
    )
}

fn message_event(data_b64: &str) -> String {
    format!(
        r#"{{"type":"conversation.mls-message-add",
             "qualified_conversation":{{"id":"{ROOM_UUID}","domain":"{DOMAIN}"}},
             "qualified_from":{{"id":"{BOB}","domain":"{DOMAIN}"}},
             "data":"{data_b64}",
             "time":"2026-08-21T10:00:01.000Z"}}"#
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

/// Welcome for this client + N case messages from bob, all in one
/// notifications page (arrival order = slice order).
fn mount_welcome_and_cases(
    server: &MockServer,
    peer: &mut common::mls::PeerGroup,
    cases: &[(&str, u64, &str)], // (message_id, revision, title)
) {
    let b64 = |bytes: Vec<u8>| base64::engine::general_purpose::STANDARD.encode(bytes);
    let welcome = b64(peer.welcome_over_key_package(&captured_key_package_for(server)));
    let mut events = vec![welcome_event(&welcome)];
    for (message_id, revision, title) in cases {
        let payload = case_payload_bytes(*revision, "case-1", title, 1 /* ACTIVE */);
        let generic = case_generic_message(message_id, &payload);
        events.push(message_event(&b64(peer.application_message(&generic))));
    }
    mount_notifications_page(server, &events);
}

#[tokio::test]
async fn admin_case_snapshots_fold_lww_by_revision() {
    let server = MockServer::start();
    let (client, _store) = logged_in_client(&server).await;
    mount_room(&server, "wire_admin");
    let mut peer = common::mls::PeerGroup::new(b"grp-case", BOB_IDENTITY);
    mount_welcome_and_cases(
        &server,
        &mut peer,
        &[
            ("case-m1", 1, "rev one"),
            ("case-m3", 3, "rev three"),
            ("case-m2", 2, "rev two"),
        ],
    );

    client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync");
    let room = client
        .room(&format!("{ROOM_UUID}@{DOMAIN}"))
        .await
        .expect("room cached");
    let case = room
        .case()
        .await
        .expect("case read")
        .expect("an admin snapshot folded");
    assert_eq!(case.revision, 3, "LWW by revision, not arrival");
    assert_eq!(case.title.as_deref(), Some("rev three"));
    assert_eq!(case.state, CaseState::Active);
    assert_eq!(case.case_ref, "case-1");

    // Review feedback 2026-08-21: the timeline must surface case rows
    // as Case items, not degrade them to Text (which would render the
    // packed JSON body as chat text).
    let items = room
        .messages(&kupferdraht::timeline::MessagesOptions {
            limit: None,
            before: None,
        })
        .await
        .expect("messages");
    let case_items = items
        .iter()
        .filter(|i| matches!(i.kind, kupferdraht::timeline::TimelineItemKind::Case(_)))
        .count();
    assert_eq!(
        case_items, 3,
        "every case row surfaces as TimelineItemKind::Case"
    );
    assert!(
        !items.iter().any(|i| {
            matches!(&i.kind, kupferdraht::timeline::TimelineItemKind::Text { body, .. }
                if body.contains("case-1"))
        }),
        "no case row degrades to a Text item leaking the packed body"
    );
}

#[tokio::test]
async fn non_admin_snapshot_is_stored_but_never_folds_core() {
    let server = MockServer::start();
    let (client, store) = logged_in_client(&server).await;
    mount_room(&server, "wire_member");
    let mut peer = common::mls::PeerGroup::new(b"grp-case", BOB_IDENTITY);
    mount_welcome_and_cases(&server, &mut peer, &[("case-m1", 1, "hijack")]);

    client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync");
    let room_id = format!("{ROOM_UUID}@{DOMAIN}");
    // Stored as history (the audit trail)…
    let rows = store
        .load_timeline_events(&room_id)
        .await
        .expect("load rows");
    assert_eq!(
        rows.iter().filter(|r| r.kind == "case").count(),
        1,
        "the non-admin case message is persisted as a timeline row"
    );
    // …but never folded into case state.
    let room = client.room(&room_id).await.expect("room cached");
    assert!(
        room.case().await.expect("case read").is_none(),
        "a non-admin snapshot must not create case state"
    );
}

#[tokio::test]
async fn redacting_a_case_message_refolds_from_survivors() {
    let server = MockServer::start();
    let (client, store) = logged_in_client(&server).await;
    mount_room(&server, "wire_admin");
    let mut peer = common::mls::PeerGroup::new(b"grp-case", BOB_IDENTITY);
    mount_welcome_and_cases(
        &server,
        &mut peer,
        &[("case-m1", 1, "rev one"), ("case-m2", 2, "rev two")],
    );

    client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync");
    let room_id = format!("{ROOM_UUID}@{DOMAIN}");
    let room = client.room(&room_id).await.expect("room cached");
    assert_eq!(
        room.case().await.expect("read").expect("folded").revision,
        2,
        "pre-redaction fold sits on rev 2"
    );

    // The rev-2 carrier is redacted (production: inbound
    // MessageDelete) — the fold must fall back to the survivor.
    assert!(
        store
            .apply_message_delete(&room_id, "case-m2", 4_500)
            .await
            .expect("redact"),
        "redaction fixture must hit its row"
    );
    let case = room
        .case()
        .await
        .expect("case read")
        .expect("survivor still folds");
    assert_eq!(case.revision, 1, "redacted rows drop out of the fold");
    assert_eq!(case.title.as_deref(), Some("rev one"));
}
