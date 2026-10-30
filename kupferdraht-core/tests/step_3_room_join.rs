// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 3 cycle 3c — `Client::join_room` + `Room::join` end-to-end.
//!
//! Spins up an `httpmock`-backed Wire stand-in, pre-computes an
//! Alice-owned MLS `GroupInfo` (same format Wire delivers at
//! `GET /v14/conversations/{d}/{id}/groupinfo`), drives Kupferdraht's
//! public API through login → `sync_metadata` → `join_room`, and
//! verifies both the commit-bundle upload hit and the post-join
//! epoch bump in memory + persistent store.

use std::sync::Arc;

use httpmock::Method::{GET, POST};
use httpmock::{Mock, MockServer, prelude::HttpMockRequest};
use kupferdraht::storage::Store;
use kupferdraht::{ClientBuilder, SyncSettings};
use openmls::group::{GroupId, MlsGroup, MlsGroupCreateConfig};
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

// Wire production ciphersuite (P-256) — matches Kupferdraht's
// `MLS_CIPHERSUITE` and therefore the `SignatureKeyPair` scheme.
const CIPHERSUITE: Ciphersuite = Ciphersuite::MLS_128_DHKEMP256_AES128GCM_SHA256_P256;
const SIGNATURE_SCHEME: SignatureScheme = SignatureScheme::ECDSA_SECP256R1_SHA256;

// Fixed Wire conversation id used throughout the mock. The path
// segment order Wire uses is `{cnv_domain}/{cnv_uuid}`.
const ROOM_UUID: &str = "room-uuid";
const ROOM_DOMAIN: &str = "example.test";

/// Matches any commit-bundle byte payload — we can't pin the exact
/// bytes because openmls randomizes the external init proposal.
fn any_message_mls_body(_req: &HttpMockRequest) -> bool {
    true
}

/// Bare-bones `Codec` mirroring Kupferdraht's `JsonCodec` — integration
/// tests can't reach the crate-private one, so we duplicate it to
/// build a matching `SqliteStorageProvider` for Alice's side.
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

/// `OpenMlsProvider` used for constructing Alice's side of the test
/// group. Same composition as `kupferdraht::encryption::KupferdrahtProvider`
/// (RustCrypto + `SqliteStorageProvider<TestJsonCodec, Connection>`),
/// independently reproduced here because the real provider type is
/// crate-private.
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

/// Build a fresh single-member MLS group as "Alice" and TLS-serialize
/// its `MlsMessage(GroupInfo)` — the same bytes Wire returns on
/// `GET /v14/conversations/{d}/{id}/groupinfo`.
fn alice_group_info_bytes() -> Vec<u8> {
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
        credential: BasicCredential::new(b"alice@example.test".to_vec()).into(),
        signature_key: signer.public().to_vec().into(),
    };
    let config = MlsGroupCreateConfig::builder()
        .ciphersuite(CIPHERSUITE)
        .use_ratchet_tree_extension(true)
        .capabilities(caps)
        .build();
    // Step 66 fixture fix: the group id must match the conversation's
    // `group_id` (`Z3JwLWFh` = b"grp-aa") — production behavior, and
    // required since join_room reads the post-join epoch from the
    // openmls group under exactly that id.
    let group = MlsGroup::new_with_group_id(
        &provider,
        &signer,
        &config,
        GroupId::from_slice(b"grp-aa"),
        credential,
    )
    .expect("alice group");
    group
        .export_group_info(provider.crypto(), &signer, true)
        .expect("export group_info")
        .tls_serialize_detached()
        .expect("TLS serialize group_info")
}

/// Mounts `conversations/list-ids` + `conversations/list` so
/// `sync_metadata` returns exactly one MLS group keyed by
/// `ROOM_UUID@ROOM_DOMAIN` at epoch 0.
fn mount_single_room_sync(server: &MockServer) {
    common::mount_empty_notifications(server);
    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list-ids");
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(
                r#"{{"qualified_conversations":[
                     {{"id":"{ROOM_UUID}","domain":"{ROOM_DOMAIN}"}}
                   ],"has_more":false,"paging_state":"END"}}"#
            ));
    });
    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list");
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(
                r#"{{"found":[
                     {{
                       "qualified_id":{{"id":"{ROOM_UUID}","domain":"{ROOM_DOMAIN}"}},
                       "type":0,
                       "protocol":"mls",
                       "epoch":0,
                       "group_id":"Z3JwLWFh",
                       "name":"Engineering",
                       "members":{{
                         "self":{{"qualified_id":{{"id":"user-uuid","domain":"example.test"}}}},
                         "others":[]
                       }}
                     }}
                   ], "not_found":[], "failed":[]}}"#
            ));
    });
    server.mock(|when, then| {
        when.method(POST).path("/v14/list-users");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"found":[]}"#);
    });
}

fn mount_groupinfo(server: &MockServer, bytes: Vec<u8>) -> Mock<'_> {
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
            .header("content-type", "message/mls")
            .is_true(any_message_mls_body);
        then.status(201)
            .header("content-type", "application/json")
            .body(r#"{"events":[],"time":"2026-04-24T10:00:00.000Z"}"#);
    })
}

async fn logged_in_client_with_storage(
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

#[tokio::test]
async fn join_room_posts_commit_bundle_and_bumps_room_epoch() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mls_dir = dir.path().to_path_buf();
    let server = MockServer::start();
    let (client, store) = logged_in_client_with_storage(&server, &mls_dir).await;
    mount_single_room_sync(&server);
    let gi = alice_group_info_bytes();
    let gi_mock = mount_groupinfo(&server, gi);
    let commit_mock = mount_commit_bundle(&server);

    client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync");
    let room_id = format!("{ROOM_UUID}@{ROOM_DOMAIN}");
    let before = client.room(&room_id).await.expect("cached room");
    assert_eq!(before.epoch(), 0, "pre-join room epoch is Wire's 0");

    client
        .join_room(&room_id)
        .await
        .expect("join_room must succeed");

    gi_mock.assert_calls(1);
    commit_mock.assert_calls(1);
    let after = client.room(&room_id).await.expect("still cached");
    assert_eq!(after.epoch(), 1, "post-join epoch is server_epoch + 1");

    // Persistence mirrors the bump — `update_room_epoch` writes through.
    let rows = store.load_rooms().await.expect("load_rooms");
    let row = rows
        .iter()
        .find(|(r, _)| r.room_id == room_id)
        .expect("row");
    assert_eq!(row.0.epoch, 1, "persisted epoch must match in-memory");
}

#[tokio::test]
async fn room_join_wrapper_delegates_to_client_join_room() {
    let dir = tempfile::tempdir().expect("tempdir");
    let server = MockServer::start();
    let (client, _store) = logged_in_client_with_storage(&server, dir.path()).await;
    mount_single_room_sync(&server);
    let gi_mock = mount_groupinfo(&server, alice_group_info_bytes());
    let commit_mock = mount_commit_bundle(&server);

    client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync");
    let room = client
        .room(&format!("{ROOM_UUID}@{ROOM_DOMAIN}"))
        .await
        .expect("cached room");
    // Drive the Room::join wrapper — it must upgrade the Weak<Client>
    // and reach the same /groupinfo + /commit-bundles endpoints.
    room.join().await.expect("Room::join must succeed");

    gi_mock.assert_calls(1);
    commit_mock.assert_calls(1);
}

#[tokio::test]
async fn sync_members_enumerates_leaves_and_backfills_profiles() {
    // Drive Bob through login → sync → join → sync_members against
    // an Alice-owned group whose `group_id` bytes match the `group_id`
    // that Wire delivers in the conversations/list response. After
    // `join_room` persists the MLS group under the same `group_id`,
    // `sync_members` must enumerate both leaves (Alice + Bob) and
    // resolve Alice's display name via `POST /v14/list-users`.
    let dir = tempfile::tempdir().expect("tempdir");
    let server = MockServer::start();
    let (client, store) = logged_in_client_with_storage(&server, dir.path()).await;

    // Build Alice's group with a known identity so sync_members can
    // resolve her via the list-users mock below.
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
        .expect("TLS serialize group_info");
    // Base64 the group_id bytes so Wire's conversations/list response
    // delivers exactly the same bytes our `room.group_id_bytes()`
    // later hands to `MlsClient::group_leaves`.
    use base64::Engine as _;
    let group_id_b64 =
        base64::engine::general_purpose::STANDARD.encode(alice_group.group_id().as_slice());

    // conversations/list must return this specific group_id so Bob's
    // post-join `group_leaves(group_id_bytes)` hits the persisted
    // group.
    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list-ids");
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(
                r#"{{"qualified_conversations":[
                     {{"id":"{ROOM_UUID}","domain":"{ROOM_DOMAIN}"}}
                   ],"has_more":false,"paging_state":"END"}}"#
            ));
    });
    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list");
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(
                r#"{{"found":[
                     {{
                       "qualified_id":{{"id":"{ROOM_UUID}","domain":"{ROOM_DOMAIN}"}},
                       "type":0,
                       "protocol":"mls",
                       "epoch":0,
                       "group_id":"{group_id_b64}",
                       "name":"Engineering",
                       "members":{{
                         "self":{{"qualified_id":{{"id":"user-uuid","domain":"example.test"}}}},
                         "others":[]
                       }}
                     }}
                   ], "not_found":[], "failed":[]}}"#
            ));
    });
    // sync_metadata already drains /list-users once (no other
    // members). sync_members will hit it a second time with Alice's
    // qualified id — return Alice's profile there.
    server.mock(|when, then| {
        when.method(POST).path("/v14/list-users").is_true(|req| {
            let bytes = req.body_ref();
            std::str::from_utf8(bytes)
                .map(|s| s.contains("alice-uuid"))
                .unwrap_or(false)
        });
        then.status(200)
            .header("content-type", "application/json")
            .body(
                r#"{"found":[
                     {"qualified_id":{"id":"alice-uuid","domain":"example.test"},
                      "name":"Alice","handle":"alix"}
                   ]}"#,
            );
    });
    // sync_metadata's empty-others /list-users (pre-join) — matches
    // any body that does not mention alice-uuid.
    server.mock(|when, then| {
        when.method(POST).path("/v14/list-users");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"found":[]}"#);
    });
    // Serve the pre-computed group_info on the /groupinfo endpoint.
    mount_groupinfo(&server, gi_bytes);
    mount_commit_bundle(&server);

    client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync");
    let room_id = format!("{ROOM_UUID}@{ROOM_DOMAIN}");
    client.join_room(&room_id).await.expect("join");
    let members = client.sync_members(&room_id).await.expect("sync_members");

    assert_eq!(members.len(), 2, "leaves must enumerate Alice + Bob");
    let alice = members
        .iter()
        .find(|m| m.user_id == "alice-uuid@example.test")
        .expect("alice present");
    assert!(!alice.is_self);
    assert_eq!(alice.display_name.as_deref(), Some("Alice"));
    assert_eq!(alice.handle.as_deref(), Some("alix"));
    let self_member = members
        .iter()
        .find(|m| m.user_id == "user-uuid@example.test")
        .expect("self member present");
    assert!(self_member.is_self);
    assert_eq!(self_member.display_name.as_deref(), Some("Alice Example"));

    // Persistence round trip: the refresh overwrote the placeholder
    // `self-only` member list from sync_metadata.
    let rows = store.load_rooms().await.expect("load_rooms");
    let (_room, persisted) = rows
        .iter()
        .find(|(r, _)| r.room_id == room_id)
        .expect("row");
    assert_eq!(persisted.len(), 2, "persisted member count");
}

#[tokio::test]
async fn room_sync_members_wrapper_matches_client() {
    // `Room::sync_members` must produce the same list as
    // `Client::sync_members` via the Weak<Client> upgrade.
    let dir = tempfile::tempdir().expect("tempdir");
    let server = MockServer::start();
    let (client, _store) = logged_in_client_with_storage(&server, dir.path()).await;

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
        .expect("TLS serialize group_info");
    use base64::Engine as _;
    let group_id_b64 =
        base64::engine::general_purpose::STANDARD.encode(alice_group.group_id().as_slice());

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
            .body(
                r#"{"found":[
                     {"qualified_id":{"id":"alice-uuid","domain":"example.test"},
                      "name":"Alice","handle":"alix"}
                   ]}"#,
            );
    });
    mount_groupinfo(&server, gi_bytes);
    mount_commit_bundle(&server);

    client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync");
    let room_id = format!("{ROOM_UUID}@{ROOM_DOMAIN}");
    client.join_room(&room_id).await.expect("join");
    let room = client.room(&room_id).await.expect("cached");
    let members = room.sync_members().await.expect("Room::sync_members");
    assert_eq!(members.len(), 2);
}

#[tokio::test]
async fn join_all_rooms_skips_already_joined_and_counts_successes() {
    // Drives `join_all_rooms` twice in a row — first call joins the
    // room, second call finds the group already persisted locally
    // and bumps `already_joined` without issuing any commit or
    // /groupinfo fetch.
    let dir = tempfile::tempdir().expect("tempdir");
    let server = MockServer::start();
    let (client, _store) = logged_in_client_with_storage(&server, dir.path()).await;

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
        .expect("TLS serialize group_info");
    use base64::Engine as _;
    let group_id_b64 =
        base64::engine::general_purpose::STANDARD.encode(alice_group.group_id().as_slice());

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
    let gi_mock = mount_groupinfo(&server, gi_bytes);
    let commit_mock = mount_commit_bundle(&server);

    client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync");

    let first = client.join_all_rooms().await.expect("join_all_rooms #1");
    assert_eq!(first.attempted, 1);
    assert_eq!(first.joined, 1);
    assert_eq!(first.already_joined, 0);
    assert!(first.failed.is_empty(), "no per-room failures expected");
    assert_eq!(gi_mock.calls(), 1);
    assert_eq!(commit_mock.calls(), 1);

    // Second pass — the group is already persisted locally, so the
    // outer `join_all_rooms` must skip without a network round trip.
    let second = client.join_all_rooms().await.expect("join_all_rooms #2");
    assert_eq!(second.attempted, 1);
    assert_eq!(second.joined, 0, "no new join on the second pass");
    assert_eq!(second.already_joined, 1, "second pass sees local MLS state");
    assert_eq!(gi_mock.calls(), 1, "no extra /groupinfo fetch");
    assert_eq!(commit_mock.calls(), 1, "no extra commit bundle");
}

#[tokio::test]
async fn join_room_rejects_unknown_room_id() {
    let dir = tempfile::tempdir().expect("tempdir");
    let server = MockServer::start();
    let (client, _store) = logged_in_client_with_storage(&server, dir.path()).await;
    // No sync_metadata → no rooms in the cache.
    let err = client
        .join_room("nonexistent@example.test")
        .await
        .expect_err("unknown room must fail");
    assert!(
        matches!(err, kupferdraht::KupferdrahtError::InvalidInput(_)),
        "expected InvalidInput, got {err:?}"
    );
}
