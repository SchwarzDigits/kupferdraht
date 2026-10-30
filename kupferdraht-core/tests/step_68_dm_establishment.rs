// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 68 — create_dm establishes the MLS 1:1 (TDD red phase).
//!
//! Pins `docs/plan/68-dm-establishment.md`: on an epoch-0 one2one the
//! initiator claims the peer's key packages, commits the peer in, and
//! uploads the bundle (only then does current wire-server list the
//! conversation — observations §66); on epoch > 0 it joins via
//! external commit instead; an empty peer pool discards the local
//! group so a retry starts clean. The peer key package and the
//! established-group `GroupInfo` are real openmls artifacts from
//! the shared `common::mls` fixtures.

use std::sync::Arc;

use httpmock::Method::{GET, POST};
use httpmock::{Mock, MockServer, prelude::HttpMockRequest};
use kupferdraht::ClientBuilder;
use kupferdraht::storage::Store;

mod common;

const DM_UUID: &str = "29d24d47-8e9e-5160-9b95-559113824997";
const DOMAIN: &str = "example.test";
const PEER_UUID: &str = "bob-uuid";
/// b64("grp-dm") — the conversation's MLS group id.
const DM_GROUP_ID_B64: &str = "Z3JwLWRt";

fn any_message_mls_body(_req: &HttpMockRequest) -> bool {
    true
}

/// A real TLS-serialized KeyPackage for Bob's device, base64 as the
/// claim endpoint delivers it. Credential identity uses Wire's
/// `{uuid}:{client}@{domain}` shape.
fn bob_key_package_b64() -> String {
    common::mls::key_package_b64(b"bob-uuid:bobdev01@example.test")
}

/// `GroupInfo` of an established DM group (epoch ≥ 1 side of the
/// flow) under the conversation's group id.
fn established_dm_group_info() -> Vec<u8> {
    common::mls::group_info_tls(b"grp-dm", b"bob-uuid:bobdev01@example.test")
}

fn dm_conv_json(epoch: u64) -> String {
    format!(
        r#"{{
            "qualified_id":{{"id":"{DM_UUID}","domain":"{DOMAIN}"}},
            "type":2,
            "protocol":"mls",
            "epoch":{epoch},
            "group_id":"{DM_GROUP_ID_B64}",
            "name":null,
            "members":{{
              "self":{{"qualified_id":{{"id":"user-uuid","domain":"{DOMAIN}"}}}},
              "others":[{{"qualified_id":{{"id":"{PEER_UUID}","domain":"{DOMAIN}"}}}}]
            }}
        }}"#
    )
}

fn mount_one2one(server: &MockServer, epoch: u64) -> Mock<'_> {
    // The envelope requires `public_keys` (MlsKeysByPurpose) — same
    // shape as the login fixture's /mls/public-keys body.
    let body = format!(
        r#"{{"conversation":{},"public_keys":{{"removal":{{"ed25519":"ZWQ=","ecdsa_secp256r1_sha256":"cDI1Ng==","ecdsa_secp384r1_sha384":"cDM4NA==","ecdsa_secp521r1_sha512":"cDUyMQ=="}}}}}}"#,
        dm_conv_json(epoch)
    );
    server.mock(move |when, then| {
        when.method(GET)
            .path(format!("/v14/one2one-conversations/{DOMAIN}/{PEER_UUID}"));
        then.status(200)
            .header("content-type", "application/json")
            .body(body.clone());
    })
}

/// Rooms metadata that already lists the DM (the post-establish
/// server view; the pre-establish hiding is instance behavior the
/// live smoke covers).
fn mount_rooms_with_dm(server: &MockServer, epoch: u64) {
    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list-ids");
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(
                r#"{{"qualified_conversations":[
                     {{"id":"{DM_UUID}","domain":"{DOMAIN}"}}
                   ],"has_more":false,"paging_state":"END"}}"#
            ));
    });
    let body = format!(
        r#"{{"found":[{}], "not_found":[], "failed":[]}}"#,
        dm_conv_json(epoch)
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
    common::mount_empty_notifications(server);
}

/// Self-claim (own other devices) — empty pool, no other devices.
fn mount_self_claim_empty(server: &MockServer) {
    server.mock(|when, then| {
        when.method(POST)
            .path(format!("/v14/mls/key-packages/claim/{DOMAIN}/user-uuid"));
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"key_packages":[]}"#);
    });
}

fn mount_peer_claim(server: &MockServer, key_packages_json: String) -> Mock<'_> {
    server.mock(move |when, then| {
        when.method(POST)
            .path(format!("/v14/mls/key-packages/claim/{DOMAIN}/{PEER_UUID}"));
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(r#"{{"key_packages":[{key_packages_json}]}}"#));
    })
}

fn mount_commit_bundle(server: &MockServer) -> Mock<'_> {
    server.mock(|when, then| {
        when.method(POST)
            .path("/v14/mls/commit-bundles")
            .is_true(any_message_mls_body);
        then.status(201)
            .header("content-type", "application/json")
            .body(r#"{"events":[],"time":"2026-08-14T10:00:00.000Z"}"#);
    })
}

fn mount_groupinfo(server: &MockServer, bytes: Vec<u8>) -> Mock<'_> {
    server.mock(move |when, then| {
        when.method(GET)
            .path(format!("/v14/conversations/{DOMAIN}/{DM_UUID}/groupinfo"));
        then.status(200)
            .header("content-type", "message/mls")
            .body(bytes.clone());
    })
}

async fn logged_in_client(server: &MockServer) -> (Arc<kupferdraht::Client>, Arc<dyn Store>) {
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

fn peer_kp_entry(b64: &str) -> String {
    format!(
        r#"{{"user":"{PEER_UUID}","domain":"{DOMAIN}","client":"bobdev01",
            "key_package":"{b64}","key_package_ref":"cmVm"}}"#
    )
}

#[tokio::test]
async fn create_dm_establishes_and_is_idempotent() {
    let server = MockServer::start();
    let (client, _store) = logged_in_client(&server).await;
    mount_rooms_with_dm(&server, 1);
    let one2one = mount_one2one(&server, 0);
    mount_self_claim_empty(&server);
    let claim = mount_peer_claim(&server, peer_kp_entry(&bob_key_package_b64()));
    let commit = mount_commit_bundle(&server);

    let peer = format!("{PEER_UUID}@{DOMAIN}");
    let room = client
        .create_dm(&peer)
        .await
        .expect("create_dm establishes the 1:1");
    assert_eq!(room.id(), format!("{DM_UUID}@{DOMAIN}"));
    assert_eq!(claim.calls(), 1, "the peer's key package was claimed");
    assert_eq!(commit.calls(), 1, "the establishing commit was uploaded");
    assert_eq!(one2one.calls(), 1);

    // Second call: the local group exists — resume, no re-establish.
    let again = client
        .create_dm(&peer)
        .await
        .expect("create_dm is idempotent");
    assert_eq!(again.id(), room.id());
    assert_eq!(claim.calls(), 1, "no second claim");
    assert_eq!(commit.calls(), 1, "no second upload");
}

#[tokio::test]
async fn create_dm_joins_when_the_peer_already_established() {
    let server = MockServer::start();
    let (client, _store) = logged_in_client(&server).await;
    mount_rooms_with_dm(&server, 2);
    let _one2one = mount_one2one(&server, 2);
    let claim = mount_peer_claim(&server, peer_kp_entry(&bob_key_package_b64()));
    let gi = mount_groupinfo(&server, established_dm_group_info());
    let commit = mount_commit_bundle(&server);

    let peer = format!("{PEER_UUID}@{DOMAIN}");
    let room = client
        .create_dm(&peer)
        .await
        .expect("create_dm joins the established 1:1");
    assert_eq!(room.id(), format!("{DM_UUID}@{DOMAIN}"));
    assert_eq!(gi.calls(), 1, "epoch > 0 → external-commit join");
    assert_eq!(commit.calls(), 1, "the join commit was uploaded");
    assert_eq!(claim.calls(), 0, "joining claims nothing");
}

#[tokio::test]
async fn create_dm_discards_on_empty_peer_pool_so_retry_reclaims() {
    let server = MockServer::start();
    let (client, _store) = logged_in_client(&server).await;
    mount_rooms_with_dm(&server, 1);
    let _one2one = mount_one2one(&server, 0);
    mount_self_claim_empty(&server);
    let claim = mount_peer_claim(&server, String::new());
    // Empty entry list: mount_peer_claim wraps `key_packages:[<json>]`;
    // an empty string yields `[]` — the exhausted-pool answer.

    let peer = format!("{PEER_UUID}@{DOMAIN}");
    let err = client
        .create_dm(&peer)
        .await
        .expect_err("an empty peer pool cannot establish");
    assert!(
        format!("{err}").contains("KeyPackages"),
        "the error names the exhausted pool, got: {err}"
    );

    // Retry: the failed attempt must NOT leave a local group behind
    // that short-circuits the claim.
    let _ = client.create_dm(&peer).await.expect_err("still empty");
    assert_eq!(
        claim.calls(),
        2,
        "the retry claims again — the local group was discarded"
    );
}
