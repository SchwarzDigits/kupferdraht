// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 66 — join consistency (TDD red phase).
//!
//! Pins `docs/plan/66-join-consistency.md` (review findings S2, S16 —
//! `open.md` §66):
//!
//! * a failed commit-bundle upload discards the just-merged local
//!   group (openmls's external-join `finalize` persists atomically —
//!   gap-B.22 tracks the upstream staged-join wish), so
//!   `join_all_rooms` retries instead of reporting `already_joined`
//!   while the client sits one epoch ahead of the server;
//! * the post-join epoch is read from the openmls group, not
//!   `room.epoch() + 1` from a possibly stale rooms cache.
//!
//! Fixture machinery mirrors `step_3_room_join.rs` (Alice-owned
//! in-memory MLS group as the `GroupInfo` source; the codec/provider
//! duplication exists because the real ones are crate-private).

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

const CIPHERSUITE: Ciphersuite = Ciphersuite::MLS_128_DHKEMP256_AES128GCM_SHA256_P256;
const SIGNATURE_SCHEME: SignatureScheme = SignatureScheme::ECDSA_SECP256R1_SHA256;
const ROOM_UUID: &str = "room-uuid";
const ROOM_DOMAIN: &str = "example.test";

fn any_message_mls_body(_req: &HttpMockRequest) -> bool {
    true
}

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

/// Fresh single-member Alice group at epoch 0; the external join built
/// from this `GroupInfo` lands the joiner at **epoch 1** — the
/// authoritative value S16 asserts against.
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
    // The group id MUST match the conversation's `group_id`
    // (`Z3JwLWFh` = b"grp-aa") — that is how production behaves, and
    // it is what makes the S2 red case real: the failed join leaves a
    // local group under exactly the id `group_exists` checks.
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

/// One MLS room whose **cached** Wire epoch is `cache_epoch` — the
/// S16 fixture deliberately diverges it from the real group state.
fn mount_single_room_sync(server: &MockServer, cache_epoch: u64) {
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
    server.mock(move |when, then| {
        when.method(POST).path("/v14/conversations/list");
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(
                r#"{{"found":[
                     {{
                       "qualified_id":{{"id":"{ROOM_UUID}","domain":"{ROOM_DOMAIN}"}},
                       "type":0,
                       "protocol":"mls",
                       "epoch":{cache_epoch},
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

fn mount_failing_commit_bundle(server: &MockServer) -> Mock<'_> {
    server.mock(|when, then| {
        when.method(POST)
            .path("/v14/mls/commit-bundles")
            .is_true(any_message_mls_body);
        then.status(500)
            .header("content-type", "application/json")
            .body(r#"{"code":500,"label":"server-error","message":"boom"}"#);
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
async fn failed_bundle_upload_discards_the_local_group_so_retry_works() {
    // Review S2: today the group survives the failed upload one epoch
    // ahead of the server, and join_all_rooms reports already_joined
    // — no retry path exists short of a manual resync.
    let dir = tempfile::tempdir().expect("tempdir");
    let server = MockServer::start();
    let (client, _store) = logged_in_client_with_storage(&server, dir.path()).await;
    mount_single_room_sync(&server, 0);
    let gi_mock = mount_groupinfo(&server, alice_group_info_bytes());
    let failing = mount_failing_commit_bundle(&server);

    client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync");
    let room_id = format!("{ROOM_UUID}@{ROOM_DOMAIN}");
    client
        .join_room(&room_id)
        .await
        .expect_err("a 500 on the bundle upload must fail the join");
    assert_eq!(failing.calls(), 1);

    // The retry surface: join_all_rooms must RE-ATTEMPT the join (a
    // second groupinfo fetch + upload attempt), not skip the room as
    // already_joined because the failed join left local state behind.
    let summary = client
        .join_all_rooms()
        .await
        .expect("join_all_rooms itself tolerates per-room failures");
    assert_eq!(
        summary.already_joined, 0,
        "a failed join must not leave a group that masquerades as joined"
    );
    assert_eq!(
        gi_mock.calls(),
        2,
        "the retry must fetch groupinfo again (attempt 1 + retry)"
    );
}

#[tokio::test]
async fn garbage_ciphertext_does_not_discard_a_healthy_group() {
    // Step-64 narrowing (review re-check): recovery fires only on a
    // positive broken-local-state signal. A garbage frame in a
    // healthy, joined group surfaces as Encoding/Generic (the group
    // loads before the ciphertext parses) and must be skipped — any
    // member can post garbage, and healing on every class would turn
    // that into discard+rejoin epoch churn once per drain. Lives in
    // this file because it needs the real join fixture.
    let dir = tempfile::tempdir().expect("tempdir");
    let server = MockServer::start();
    let (client, _store) = logged_in_client_with_storage(&server, dir.path()).await;
    // Rooms metadata WITHOUT the empty-notifications mock — this test
    // mounts its own notifications frame below and httpmock matches
    // the first registered mock.
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
    let gi_mock = mount_groupinfo(&server, alice_group_info_bytes());
    let _commit = mount_commit_bundle(&server);

    client.sync_metadata().await.expect("rooms cached");
    let room_id = format!("{ROOM_UUID}@{ROOM_DOMAIN}");
    client.join_room(&room_id).await.expect("join succeeds");
    assert_eq!(gi_mock.calls(), 1, "the join fetched groupinfo once");

    // Garbage mls-message-add for the now-healthy group.
    server.mock(|when, then| {
        when.method(GET).path("/v14/notifications");
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(
                r#"{{"notifications":[{{"id":"notif-garbage","payload":[{{
                    "type":"conversation.mls-message-add",
                    "qualified_conversation":{{"id":"{ROOM_UUID}","domain":"{ROOM_DOMAIN}"}},
                    "qualified_from":{{"id":"bob-uuid","domain":"{ROOM_DOMAIN}"}},
                    "data":"AAECAwQFBgc="
                }}]}}],"has_more":false}}"#
            ));
    });
    client
        .sync_once(SyncSettings::default())
        .await
        .expect("garbage never aborts the drain");
    assert_eq!(
        gi_mock.calls(),
        1,
        "a garbage frame in a healthy group must be skipped, not answered \
         with a discard+rejoin (no second groupinfo fetch)"
    );
    // (No epoch assertion: sync_once's metadata refresh re-reads the
    // mock, which statically reports the pre-join epoch — a real
    // server would report the post-commit epoch. The no-rejoin proof
    // above is the meaningful signal.)
}

#[tokio::test]
async fn join_persists_the_epoch_reported_by_openmls_not_cache_plus_one() {
    // Review S16: the cached Wire epoch is deliberately wrong (7);
    // the real group is Alice's fresh epoch-0 group, so the external
    // join lands at epoch 1. Today the code persists cache+1 = 8.
    let dir = tempfile::tempdir().expect("tempdir");
    let server = MockServer::start();
    let (client, store) = logged_in_client_with_storage(&server, dir.path()).await;
    mount_single_room_sync(&server, 7);
    let _gi = mount_groupinfo(&server, alice_group_info_bytes());
    let _commit = mount_commit_bundle(&server);

    client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync");
    let room_id = format!("{ROOM_UUID}@{ROOM_DOMAIN}");
    client
        .join_room(&room_id)
        .await
        .expect("join_room must succeed");

    let after = client.room(&room_id).await.expect("still cached");
    assert_eq!(
        after.epoch(),
        1,
        "the post-join epoch comes from the openmls group (0 → join → 1), \
         not from the stale cache (7 + 1 = 8)"
    );
    let rows = store.load_rooms().await.expect("load_rooms");
    let row = rows
        .iter()
        .find(|(r, _)| r.room_id == room_id)
        .expect("row");
    assert_eq!(row.0.epoch, 1, "persisted epoch matches the group state");
}
