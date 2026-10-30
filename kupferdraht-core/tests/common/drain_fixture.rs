// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! The client-side fixture of the drain tests (Steps 97e and 109): a
//! client whose account store is a `FaultyStore`, joined through a Welcome
//! to the MLS group of a fixture peer (bob), a mock server for the
//! notification queue and the room, and small protobuf builders.
#![allow(dead_code)]

use std::sync::{Arc, Mutex};

use base64::Engine as _;
use httpmock::Method::{GET, POST};
use httpmock::{MockServer, prelude::HttpMockRequest};
use kupferdraht::keystore::{InMemoryKeyStore, SecureKeyStore};
use kupferdraht::storage::Store;
use kupferdraht::{Client, EventHandler, Message, SyncSettings};

use super::faulty_store::{FaultyProvider, FaultyStore};

pub const ROOM_UUID: &str = "97e0aa31-77aa-4bde-9d10-223344556677";
pub const DOMAIN: &str = "example.test";
/// b64("grp-97e") — the room's MLS group id.
pub const GROUP_ID_B64: &str = "Z3JwLTk3ZQ==";
pub const GROUP_ID: &[u8] = b"grp-97e";
pub const BOB: &str = "bob-uuid";
pub const BOB_IDENTITY: &[u8] = b"bob-uuid:bobdev01@example.test";

pub fn room_id() -> String {
    format!("{ROOM_UUID}@{DOMAIN}")
}

pub fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

// ---- minimal protobuf writer (proto2 wire format, the Step-72b precedent) ----

pub fn put_varint(mut v: u64, out: &mut Vec<u8>) {
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

pub fn put_len_delimited(field: u32, bytes: &[u8], out: &mut Vec<u8>) {
    put_varint((u64::from(field) << 3) | 2, out);
    put_varint(bytes.len() as u64, out);
    out.extend_from_slice(bytes);
}

/// `GenericMessage { message_id = 1, text = 2 { content = 1 } }`.
pub fn text_message(message_id: &str, content: &str) -> Vec<u8> {
    let mut text = Vec::new();
    put_len_delimited(1, content.as_bytes(), &mut text);
    let mut m = Vec::new();
    put_len_delimited(1, message_id.as_bytes(), &mut m);
    put_len_delimited(2, &text, &mut m);
    m
}

/// `GenericMessage { message_id = 1, reaction = 17 { emoji = 1, message_id = 2 } }`.
pub fn reaction_message(message_id: &str, target: &str, emoji: &str) -> Vec<u8> {
    let mut reaction = Vec::new();
    put_len_delimited(1, emoji.as_bytes(), &mut reaction);
    put_len_delimited(2, target.as_bytes(), &mut reaction);
    let mut m = Vec::new();
    put_len_delimited(1, message_id.as_bytes(), &mut m);
    put_len_delimited(17, &reaction, &mut m);
    m
}

/// The text content of a decrypted `GenericMessage` (field 2 → 1).
pub fn text_content(generic: &[u8]) -> String {
    let needle_start = generic
        .windows(1)
        .position(|w| w[0] == 0x12)
        .expect("a text field");
    let text = &generic[needle_start + 2..];
    let content = &text[2..];
    String::from_utf8_lossy(content).to_string()
}

// ---- the queue ----

pub struct Notif {
    pub id: &'static str,
    pub events: Vec<String>,
}

pub fn welcome_event(welcome: &[u8]) -> String {
    format!(
        r#"{{"type":"conversation.mls-welcome",
             "qualified_conversation":{{"id":"{ROOM_UUID}","domain":"{DOMAIN}"}},
             "qualified_from":{{"id":"{BOB}","domain":"{DOMAIN}"}},
             "data":"{}","time":"2026-09-28T10:00:00.000Z"}}"#,
        b64(welcome)
    )
}

pub fn mls_event(tls: &[u8]) -> String {
    format!(
        r#"{{"type":"conversation.mls-message-add",
             "qualified_conversation":{{"id":"{ROOM_UUID}","domain":"{DOMAIN}"}},
             "qualified_from":{{"id":"{BOB}","domain":"{DOMAIN}"}},
             "from_client":"bobdev01",
             "data":"{}","time":"2026-09-28T10:00:01.000Z"}}"#,
        b64(tls)
    )
}

pub fn since(req: &HttpMockRequest) -> Option<String> {
    req.query_params_map().get("since").cloned()
}

/// Answer `GET /notifications` for one `since` cursor with `notifs`.
pub fn mount_page(server: &MockServer, cursor: Option<&'static str>, notifs: &[Notif]) {
    let items: Vec<String> = notifs
        .iter()
        .map(|n| format!(r#"{{"id":"{}","payload":[{}]}}"#, n.id, n.events.join(",")))
        .collect();
    let body = format!(
        r#"{{"notifications":[{}],"has_more":false}}"#,
        items.join(",")
    );
    server.mock(move |when, then| {
        when.method(GET)
            .path("/v14/notifications")
            .is_true(move |req| since(req).as_deref() == cursor);
        then.status(200)
            .header("content-type", "application/json")
            .body(body.clone());
    });
}

// ---- mocks ----

pub static UPLOADED_KPS: Mutex<Vec<(String, Vec<String>)>> = Mutex::new(Vec::new());

pub fn capture_key_package_upload(req: &HttpMockRequest) -> bool {
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
/// to this server (httpmock pools servers, LEARNINGS §5.13).
pub fn client_key_package(server: &MockServer) -> Vec<u8> {
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

pub fn mount_room(server: &MockServer) {
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
             "type":0,"protocol":"mls","epoch":1,"group_id":"{GROUP_ID_B64}",
             "name":"Persist first",
             "members":{{
               "self":{{"qualified_id":{{"id":"user-uuid","domain":"{DOMAIN}"}},
                       "conversation_role":"wire_admin"}},
               "others":[
                 {{"qualified_id":{{"id":"{BOB}","domain":"{DOMAIN}"}},"conversation_role":"wire_admin"}}
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

/// `POST /v14/mls/messages`: remember every uploaded ciphertext.
pub fn mount_message_upload(server: &MockServer) -> Arc<Mutex<Vec<Vec<u8>>>> {
    let uploads = Arc::new(Mutex::new(Vec::new()));
    let sink = uploads.clone();
    server.mock(move |when, then| {
        when.method(POST)
            .path("/v14/mls/messages")
            .is_true(move |req| {
                sink.lock()
                    .expect("uploads lock")
                    .push(req.body_ref().to_vec());
                true
            });
        then.status(201)
            .header("content-type", "application/json")
            .body(r#"{"events":[],"time":"2026-09-28T10:00:02.000Z"}"#);
    });
    uploads
}

// ---- fixture ----

#[derive(Default)]
pub struct Recorder {
    pub messages: Mutex<Vec<String>>,
    pub errors: Mutex<Vec<String>>,
}

impl EventHandler for Recorder {
    fn on_message(&self, message: Message) {
        self.messages
            .lock()
            .expect("messages")
            .push(message.message_id);
    }
    fn on_room_invited(&self, _room_id: String) {}
    fn on_sync_error(&self, description: String) {
        self.errors.lock().expect("errors").push(description);
    }
}

impl Recorder {
    pub fn count(&self, message_id: &str) -> usize {
        self.messages
            .lock()
            .expect("messages")
            .iter()
            .filter(|id| *id == message_id)
            .count()
    }
    pub fn errors(&self) -> Vec<String> {
        self.errors.lock().expect("errors").clone()
    }
}

pub struct Fixture {
    pub client: Arc<Client>,
    pub store: Arc<FaultyStore>,
    pub bob: super::mls::PeerGroup,
    pub recorder: Arc<Recorder>,
    pub _home: tempfile::TempDir,
}

/// A client whose account store is a `FaultyStore`, joined to bob's group
/// through the Welcome at `n1` (drained).
pub async fn joined_client(server: &MockServer) -> Fixture {
    server.mock(|when, then| {
        when.method(POST)
            .path_includes("/v14/mls/key-packages/self/")
            .is_true(capture_key_package_upload);
        then.status(201);
    });
    super::mount_full_login_flow(server);
    mount_room(server);
    let home = tempfile::tempdir().expect("tempdir");
    let keystore: Arc<dyn SecureKeyStore> = Arc::new(InMemoryKeyStore::new());
    let provider = Arc::new(FaultyProvider::new(
        home.path().to_path_buf(),
        keystore.clone(),
    ));
    let client = kupferdraht::Kupferdraht::builder()
        .base_dir(home.path().to_path_buf())
        .keystore(keystore)
        .permit_http()
        .storage_provider(provider.clone())
        .build()
        .expect("open the home")
        .login(
            super::server_config(&server.base_url()),
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
    let recorder = Arc::new(Recorder::default());
    client.add_event_handler(recorder.clone());
    let mut bob = super::mls::PeerGroup::new_mixed_plaintext(GROUP_ID, BOB_IDENTITY);
    let welcome = bob.welcome_over_key_package(&client_key_package(server));
    mount_page(
        server,
        None,
        &[Notif {
            id: "n1",
            events: vec![welcome_event(&welcome)],
        }],
    );
    drain(&client).await;
    Fixture {
        client,
        store: provider.faulty(),
        bob,
        recorder,
        _home: home,
    }
}

pub async fn drain(client: &Client) {
    client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync_once");
}

pub async fn cursor(store: &Arc<FaultyStore>) -> Option<String> {
    store
        .load_sync_state()
        .await
        .expect("sync state")
        .and_then(|s| s.last_event_id)
}
