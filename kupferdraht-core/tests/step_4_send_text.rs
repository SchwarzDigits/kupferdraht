// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 4 cycle 4d — `Timeline::send_text` end-to-end.
//!
//! Reuses the Step 3 mock pattern: spin up an `httpmock` stand-in,
//! pre-compute an Alice-owned MLS group, drive
//! login → sync_metadata → join_room → `Room::timeline().send_text`.
//! Asserts that the `/v14/mls/messages` endpoint receives a
//! `PrivateMessage`-framed body and that a `timeline_events` row
//! lands with `delivery_status='submitted'`.

use std::sync::Arc;

use httpmock::Method::{GET, POST};
use httpmock::{Mock, MockServer, prelude::HttpMockRequest};
use kupferdraht::storage::Store;
use kupferdraht::{ClientBuilder, SyncSettings};
use openmls::group::{MlsGroup, MlsGroupCreateConfig};
use openmls::prelude::{
    BasicCredential, Capabilities, Ciphersuite, CredentialType, CredentialWithKey,
    MlsMessageBodyIn, MlsMessageIn, WireFormat,
};
use openmls_basic_credential::SignatureKeyPair;
use openmls_rust_crypto::RustCrypto;
use openmls_sqlite_storage::{Codec, Connection, SqliteStorageProvider};
use openmls_traits::OpenMlsProvider;
use openmls_traits::types::SignatureScheme;
use serde::Serialize;
use serde::de::DeserializeOwned;
use tls_codec::{DeserializeBytes as _, Serialize as _};

mod common;

const CIPHERSUITE: Ciphersuite = Ciphersuite::MLS_128_DHKEMP256_AES128GCM_SHA256_P256;
const SIGNATURE_SCHEME: SignatureScheme = SignatureScheme::ECDSA_SECP256R1_SHA256;
const ROOM_UUID: &str = "room-uuid";
const ROOM_DOMAIN: &str = "example.test";

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

fn mount_groupinfo<'a>(server: &'a MockServer, bytes: Vec<u8>) -> Mock<'a> {
    server.mock(|when, then| {
        when.method(GET).path(format!(
            "/v14/conversations/{ROOM_DOMAIN}/{ROOM_UUID}/groupinfo"
        ));
        then.status(200)
            .header("content-type", "message/mls")
            .body(bytes);
    })
}

fn mount_commit_bundle(server: &MockServer) -> Mock<'_> {
    server.mock(|when, then| {
        when.method(POST)
            .path("/v14/mls/commit-bundles")
            .header("content-type", "message/mls");
        then.status(201)
            .header("content-type", "application/json")
            .body(r#"{"events":[],"time":"2026-04-25T10:00:00.000Z"}"#);
    })
}

/// Global capture slot for the `/v14/mls/messages` body. httpmock's
/// `matches()` only accepts non-capturing `fn` pointers, so the
/// matcher writes through this static mutex instead of borrowing the
/// captor by reference.
static CAPTURED_SEND_BODY: std::sync::OnceLock<std::sync::Mutex<Option<Vec<u8>>>> =
    std::sync::OnceLock::new();

fn capture_slot() -> &'static std::sync::Mutex<Option<Vec<u8>>> {
    CAPTURED_SEND_BODY.get_or_init(|| std::sync::Mutex::new(None))
}

fn capture_send_body(req: &HttpMockRequest) -> bool {
    {
        let bytes = req.body_ref();
        *capture_slot().lock().expect("captured mutex") = Some(bytes.to_vec());
    }
    true
}

fn mount_send_text(server: &MockServer) -> Mock<'_> {
    server.mock(|when, then| {
        when.method(POST)
            .path("/v14/mls/messages")
            .header("content-type", "message/mls")
            .is_true(capture_send_body);
        then.status(201)
            .header("content-type", "application/json")
            .body(r#"{"events":[],"time":"2026-04-25T10:01:00.000Z"}"#);
    })
}

/// `/v14/mls/messages` rejects the send — used by the Step 25a Failed test.
fn mount_send_text_failure(server: &MockServer) -> Mock<'_> {
    server.mock(|when, then| {
        when.method(POST)
            .path("/v14/mls/messages")
            .header("content-type", "message/mls");
        then.status(500)
            .header("content-type", "application/json")
            .body(r#"{"code":500,"label":"server-error","message":"boom"}"#);
    })
}

async fn logged_in_client(
    server: &MockServer,
    base_dir: &std::path::Path,
) -> (Arc<kupferdraht::Client>, Arc<dyn Store>) {
    common::mount_full_login_flow(server);
    common::mount_empty_notifications(server);
    let client = ClientBuilder::new()
        .server(common::server_config(&server.base_url()))
        .permit_http()
        .base_dir(base_dir.to_path_buf())
        .login("alice@example.test".into(), "pw".into(), None)
        .await
        .expect("login")
        .register_device()
        .await
        .expect("register device");
    let store = client.store().expect("client store");
    (client, store)
}

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
    let alice_group = MlsGroup::new(&provider, &signer, &config, credential).expect("alice group");
    let gi_bytes = alice_group
        .export_group_info(provider.crypto(), &signer, true)
        .expect("export_group_info")
        .tls_serialize_detached()
        .expect("group_info serialize");
    use base64::Engine as _;
    let group_id_b64 =
        base64::engine::general_purpose::STANDARD.encode(alice_group.group_id().as_slice());
    (gi_bytes, group_id_b64)
}

#[tokio::test]
async fn timeline_send_text_uploads_private_message_and_persists_row() {
    let dir = tempfile::tempdir().expect("tempdir");
    let server = MockServer::start();
    let (client, store) = logged_in_client(&server, dir.path()).await;

    let (gi_bytes, group_id_b64) = alice_group_setup();

    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list-ids");
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(
                r#"{{"qualified_conversations":[{{"id":"{ROOM_UUID}","domain":"{ROOM_DOMAIN}"}}],
                    "has_more":false,"paging_state":"END"}}"#
            ));
    });
    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list");
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(
                r#"{{"found":[{{
                     "qualified_id":{{"id":"{ROOM_UUID}","domain":"{ROOM_DOMAIN}"}},
                     "type":0,"protocol":"mls","epoch":0,
                     "group_id":"{group_id_b64}","name":"Engineering",
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
    mount_groupinfo(&server, gi_bytes);
    mount_commit_bundle(&server);
    let send_mock = mount_send_text(&server);

    client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync");
    let room_id = format!("{ROOM_UUID}@{ROOM_DOMAIN}");
    client.join_room(&room_id).await.expect("join_room");

    let room = client.room(&room_id).await.expect("cached room");
    let timeline = room.timeline().expect("timeline handle");
    let handle = timeline
        .send_text("Hello from Kupferdraht".into())
        .await
        .expect("send_text must succeed");

    // Hits + body shape.
    send_mock.assert_calls(1);
    let body = capture_slot()
        .lock()
        .expect("captured")
        .clone()
        .expect("body captured");
    let (msg, rest) = MlsMessageIn::tls_deserialize_bytes(&body).expect("parse MlsMessage");
    assert!(rest.is_empty(), "single MlsMessage envelope");
    assert_eq!(msg.wire_format(), WireFormat::PrivateMessage);
    assert!(matches!(msg.extract(), MlsMessageBodyIn::PrivateMessage(_)));

    // MessageHandle invariants.
    assert_eq!(handle.message_id.len(), 36, "UUID v4 hyphenated");
    assert!(handle.sent_at_ms > 0);

    // Persistence: a row landed with the same message_id and
    // delivery_status='submitted'.
    let events = store
        .load_timeline_events(&room_id)
        .await
        .expect("load events");
    assert_eq!(events.len(), 1);
    let row = &events[0];
    assert_eq!(row.message_id, handle.message_id);
    assert_eq!(row.room_id, room_id);
    assert_eq!(row.kind, "text");
    assert_eq!(row.body, "Hello from Kupferdraht");
    assert_eq!(row.delivery_status, "submitted");
    assert_eq!(row.sender_user_id, "user-uuid@example.test");
    assert_eq!(row.sender_device_id, "new-client-id");
}

#[tokio::test]
async fn timeline_send_text_persists_failed_row_when_upload_fails() {
    let dir = tempfile::tempdir().expect("tempdir");
    let server = MockServer::start();
    let (client, store) = logged_in_client(&server, dir.path()).await;

    let (gi_bytes, group_id_b64) = alice_group_setup();

    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list-ids");
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(
                r#"{{"qualified_conversations":[{{"id":"{ROOM_UUID}","domain":"{ROOM_DOMAIN}"}}],
                    "has_more":false,"paging_state":"END"}}"#
            ));
    });
    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list");
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(
                r#"{{"found":[{{
                     "qualified_id":{{"id":"{ROOM_UUID}","domain":"{ROOM_DOMAIN}"}},
                     "type":0,"protocol":"mls","epoch":0,
                     "group_id":"{group_id_b64}","name":"Engineering",
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
    mount_groupinfo(&server, gi_bytes);
    mount_commit_bundle(&server);
    // The send itself is rejected by the backend.
    let _send_fail = mount_send_text_failure(&server);

    client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync");
    let room_id = format!("{ROOM_UUID}@{ROOM_DOMAIN}");
    client.join_room(&room_id).await.expect("join_room");

    let room = client.room(&room_id).await.expect("cached room");
    let timeline = room.timeline().expect("timeline handle");
    let result = timeline.send_text("doomed message".into()).await;
    assert!(
        result.is_err(),
        "a rejected send must surface the upload error to the caller"
    );

    // The message must NOT vanish — it persists as `failed` so the UI can
    // show it (Step 25a).
    let events = store
        .load_timeline_events(&room_id)
        .await
        .expect("load events");
    assert_eq!(
        events.len(),
        1,
        "the failed message must persist, not vanish"
    );
    let row = &events[0];
    assert_eq!(row.delivery_status, "failed");
    assert_eq!(row.body, "doomed message");
    assert_eq!(row.kind, "text");
}

#[tokio::test]
async fn timeline_send_text_errors_when_room_has_no_local_mls_state() {
    // No `join_room` called → group not in openmls storage. The
    // encrypt step must surface InvalidInput with a pointer to
    // `Client::join_room`.
    let dir = tempfile::tempdir().expect("tempdir");
    let server = MockServer::start();
    let (client, _store) = logged_in_client(&server, dir.path()).await;
    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list-ids");
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(
                r#"{{"qualified_conversations":[{{"id":"{ROOM_UUID}","domain":"{ROOM_DOMAIN}"}}],
                    "has_more":false,"paging_state":"END"}}"#
            ));
    });
    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list");
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(
                r#"{{"found":[{{
                     "qualified_id":{{"id":"{ROOM_UUID}","domain":"{ROOM_DOMAIN}"}},
                     "type":0,"protocol":"mls","epoch":0,
                     "group_id":"Z3JwLWFh","name":"Engineering",
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

    client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync");
    let room = client
        .room(&format!("{ROOM_UUID}@{ROOM_DOMAIN}"))
        .await
        .expect("cached room");
    let timeline = room.timeline().expect("timeline handle");
    let err = timeline
        .send_text("hi".into())
        .await
        .expect_err("send must fail without local MLS state");
    // Step 14g: typed `MlsGroupMissing` variant replaces the prior
    // `InvalidInput("...group not in storage...")` for the
    // "room cached but openmls store has no group" case, so
    // callers (including `Timeline::send_text_auto_join`) can
    // typed-match the recovery path.
    assert!(
        matches!(err, kupferdraht::KupferdrahtError::MlsGroupMissing),
        "expected MlsGroupMissing, got {err:?}"
    );
}
