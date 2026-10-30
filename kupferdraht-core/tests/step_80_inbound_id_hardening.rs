// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 80 — inbound-id path hardening.
//!
//! The envelope `message_id` is PEER-controlled and reaches the
//! filesystem (asset cache `<id>.enc`, `asset_receive.rs`). The
//! dispatch boundary must discard a path-unsafe id like any other
//! malformed envelope (log-and-discard — never a hard error that
//! aborts the drain, never an assert: runtime data). Path-site
//! guards and the `UserId::parse` charset are pinned as unit tests
//! next to their code (`storage/home.rs`, `identity/mod.rs`,
//! `timeline/asset_receive.rs`).
//!
//! Fixtures mirror `step_72b_case_fold.rs`: the client is welcomed
//! into a fixture peer's group; inbound payloads are hand-emitted
//! protobuf (`wire::proto` is crate-private, ADR-0009).

use std::sync::{Arc, Mutex};

use base64::Engine as _;
use httpmock::Method::{GET, POST};
use httpmock::{MockServer, prelude::HttpMockRequest};
use kupferdraht::{ClientBuilder, SyncSettings};

mod common;

const ROOM_UUID: &str = "9a55e6c1-0b32-4d8f-8f1a-77aa00bb44cc";
const DOMAIN: &str = "example.test";
/// b64("grp-hardening")
const GROUP_ID_B64: &str = "Z3JwLWhhcmRlbmluZw==";
const GROUP_ID: &[u8] = b"grp-hardening";
const BOB: &str = "bob-uuid";
const BOB_IDENTITY: &[u8] = b"bob-uuid:bobdev01@example.test";

// ---- minimal protobuf writer (proto2 wire format, 72b precedent) ----

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

/// `GenericMessage { message_id = 1, content.text = 2 }` with
/// `Text { content = 1 }` — the smallest dispatchable message.
fn text_generic_message(message_id: &str, body: &str) -> Vec<u8> {
    let mut text = Vec::new();
    put_len_delimited(1, body.as_bytes(), &mut text);
    let mut m = Vec::new();
    put_len_delimited(1, message_id.as_bytes(), &mut m);
    put_len_delimited(2, &text, &mut m);
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

fn mount_room(server: &MockServer) {
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
                     "name":"Hardening",
                     "members":{{
                       "self":{{"qualified_id":{{"id":"user-uuid","domain":"{DOMAIN}"}},
                               "conversation_role":"wire_admin"}},
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

fn mount_notifications_page(server: &MockServer, events: &[String]) {
    let payload = events.join(",");
    let body = format!(
        r#"{{"notifications":[{{"id":"9100e858-8eae-11ef-0000-0000080cafe0","payload":[{payload}]}}],"has_more":false}}"#
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
             "time":"2026-09-10T10:00:00.000Z"}}"#
    )
}

fn message_event(data_b64: &str) -> String {
    format!(
        r#"{{"type":"conversation.mls-message-add",
             "qualified_conversation":{{"id":"{ROOM_UUID}","domain":"{DOMAIN}"}},
             "qualified_from":{{"id":"{BOB}","domain":"{DOMAIN}"}},
             "data":"{data_b64}",
             "time":"2026-09-10T10:00:01.000Z"}}"#
    )
}

/// Records every `on_sync_error` — the discard must be silent
/// toward consumers (log-and-discard, not an error storm).
#[derive(Default)]
struct ErrorRecorder(Mutex<Vec<String>>);

impl kupferdraht::EventHandler for ErrorRecorder {
    fn on_message(&self, _message: kupferdraht::Message) {}
    fn on_room_invited(&self, _room_id: String) {}
    fn on_sync_error(&self, error: String) {
        self.0.lock().expect("recorder lock").push(error);
    }
}

#[tokio::test]
async fn path_unsafe_message_ids_are_discarded_at_the_boundary() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST)
            .path_includes("/v14/mls/key-packages/self/")
            .is_true(capture_key_package_upload);
        then.status(201);
    });
    common::mount_full_login_flow(&server);
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
    let errors = Arc::new(ErrorRecorder::default());
    client.add_event_handler(errors.clone());
    let store = client.store().expect("store");

    mount_room(&server);
    let mut peer = common::mls::PeerGroup::new(GROUP_ID, BOB_IDENTITY);
    let b64 = |bytes: Vec<u8>| base64::engine::general_purpose::STANDARD.encode(bytes);
    let welcome = b64(peer.welcome_over_key_package(&captured_key_package_for(&server)));
    // One page: welcome, then three hostile ids around one good
    // message — the drain must discard the hostile ones and still
    // deliver the good one (a poisoned id must not abort the page).
    let events = vec![
        welcome_event(&welcome),
        message_event(&b64(peer.application_message(&text_generic_message(
            "../../../../tmp/evil",
            "traversal",
        )))),
        message_event(&b64(peer.application_message(&text_generic_message(
            "..\\..\\win-evil",
            "backslash",
        )))),
        message_event(&b64(peer.application_message(&text_generic_message(
            "5c22fa31-77aa-4bde-9d10-aabbccddeeff",
            "legitimate",
        )))),
        message_event(&b64(peer.application_message(&text_generic_message(
            "id\u{0}null",
            "control char",
        )))),
    ];
    mount_notifications_page(&server, &events);

    client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync survives hostile ids");

    let room_id = format!("{ROOM_UUID}@{DOMAIN}");
    let rows = store
        .load_timeline_events(&room_id)
        .await
        .expect("load rows");
    assert_eq!(
        rows.len(),
        1,
        "only the legitimate message persists — hostile ids are \
         discarded at the boundary, got {:?}",
        rows.iter()
            .map(|r| r.message_id.clone())
            .collect::<Vec<_>>()
    );
    assert_eq!(rows[0].message_id, "5c22fa31-77aa-4bde-9d10-aabbccddeeff");
    assert_eq!(
        errors.0.lock().expect("recorder lock").len(),
        0,
        "log-and-discard: hostile envelopes never reach on_sync_error"
    );
}

/// The materialization sites themselves refuse a hostile account id
/// (belt-and-braces below `UserId::parse`): the login identity is
/// server-supplied, `UserId::new` is infallible, and
/// `base/accounts/<id>` must never be derived from an unchecked
/// string — whichever path built it.
#[tokio::test]
async fn open_default_store_refuses_path_unsafe_account_ids() {
    let dir = tempfile::tempdir().expect("tempdir");
    let keystore: Arc<dyn kupferdraht::keystore::SecureKeyStore> =
        Arc::new(kupferdraht::keystore::InMemoryKeyStore::new());
    // Whole-component semantics: separators, exact dot-dirs, and
    // control characters are the escapes here; part-level strictness
    // ("uuid@.." — harmless as ONE component) is UserId::parse's
    // layer and pinned there.
    for bad in [
        "../../evil@example.test",
        "a/b@c",
        "..",
        "a\\b@c",
        "x\u{0}y@c",
    ] {
        let result = kupferdraht::storage::open_default_store(dir.path(), &keystore, bad);
        assert!(result.is_err(), "must refuse account id {bad:?}");
    }
    let leaked: Vec<_> = std::fs::read_dir(dir.path())
        .expect("read tempdir")
        .collect();
    assert!(
        leaked.is_empty(),
        "no directory may be created for a refused id: {leaked:?}"
    );
}

/// The second layer probed honestly (review 2026-09-11): even with a
/// hostile id ALREADY persisted (a row predating the boundary check,
/// or a future dispatch path that forgets it), the download path
/// refuses before any filesystem write — nothing appears outside the
/// account layout, no `assets/` directory is even created.
#[tokio::test]
async fn download_with_persisted_hostile_id_writes_nothing() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST)
            .path_includes("/v14/mls/key-packages/self/")
            .is_true(capture_key_package_upload);
        then.status(201);
    });
    common::mount_full_login_flow(&server);
    mount_room(&server);
    mount_notifications_page(&server, &[]);
    let dir = tempfile::tempdir().expect("tempdir");
    let keystore: Arc<dyn kupferdraht::keystore::SecureKeyStore> =
        Arc::new(kupferdraht::keystore::InMemoryKeyStore::new());
    let client = ClientBuilder::new()
        .server(common::server_config(&server.base_url()))
        .permit_http()
        .base_dir(dir.path().to_path_buf())
        .keystore(keystore)
        .login("alice@example.test".into(), "pw".into(), None)
        .await
        .expect("login")
        .register_device()
        .await
        .expect("register device");
    client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync");
    let room_id = format!("{ROOM_UUID}@{DOMAIN}");
    let store = client.store().expect("store");
    let hostile_id = "../../hostile-dl";
    store
        .insert_timeline_event(&kupferdraht::storage::TimelineEventRecord {
            message_id: hostile_id.to_string(),
            room_id: room_id.clone(),
            sender_user_id: format!("{BOB}@{DOMAIN}"),
            sender_device_id: "bobdev01".into(),
            kind: "asset".into(),
            body: "x.bin".into(),
            sent_at_ms: 1,
            delivery_status: "received".into(),
            reply_to_id: None,
            edited_at_ms: None,
            redacted_at_ms: None,
            expires_at_ms: None,
            ephemeral_ttl_ms: None,
        })
        .await
        .expect("seed hostile timeline row");
    store
        .insert_attachment(&kupferdraht::storage::AttachmentRecord {
            message_id: hostile_id.to_string(),
            room_id: room_id.clone(),
            mime_type: "application/octet-stream".into(),
            filename: Some("x.bin".into()),
            size_bytes: 4,
            sha256: vec![0u8; 32],
            otr_key_ct: vec![0u8; 32],
            asset_id: "asset-1".into(),
            asset_token: None,
            asset_domain: DOMAIN.into(),
            encryption_algo: "AES_CBC".into(),
            cache_path: None,
            created_at_ms: 1,
        })
        .await
        .expect("seed hostile attachment row");

    let room = client.room(&room_id).await.expect("room cached");
    let timeline = room.timeline().expect("timeline");
    let summary = timeline
        .asset(hostile_id)
        .await
        .expect("asset lookup")
        .expect("seeded row found");
    let err = summary
        .download()
        .await
        .expect_err("download of a hostile id must refuse");
    assert!(
        matches!(err, kupferdraht::KupferdrahtError::InvalidInput(_)),
        "expected InvalidInput, got {err:?}"
    );
    // Nothing was written: no assets/ dir, no artifact anywhere in
    // the home carrying the hostile name.
    let mut pending = vec![dir.path().to_path_buf()];
    while let Some(p) = pending.pop() {
        for entry in std::fs::read_dir(&p).expect("walk home") {
            let path = entry.expect("entry").path();
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            assert!(
                !name.contains("hostile") && name != "assets",
                "unexpected artifact {path:?}"
            );
            if path.is_dir() {
                pending.push(path);
            }
        }
    }
}
