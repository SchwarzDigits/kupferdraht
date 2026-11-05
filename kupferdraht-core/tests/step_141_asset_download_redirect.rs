// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 141 — a received file downloads through the backend's redirect.
//!
//! Wire answers `GET /v14/assets/{domain}/{id}` with `302` to a pre-signed
//! URL on the asset store; the bytes never come from the API host. These
//! tests take the consumer's path (`Timeline::asset` → `download` /
//! `save_to`) against an API mock that redirects to a second mock server.
//! The transport rules of the hop (one hop, no credentials, the scheme
//! check) are unit tests next to the code (`wire/http/asset_redirect*.rs`).
//! Integrity stays with the digest in the MLS message (ADR-0011): a hop
//! that serves other bytes fails `IntegrityCheck`, it never substitutes.

use aes::Aes256;
use aes::cipher::block_padding::Pkcs7;
use aes::cipher::{BlockEncryptMut as _, KeyIvInit as _};
use httpmock::Method::GET;
use httpmock::MockServer;
use kupferdraht::KupferdrahtError;
use kupferdraht::storage::{AttachmentRecord, TimelineEventRecord};
use sha2::{Digest as _, Sha256};

mod common;

use common::drain_fixture::{self, BOB, DOMAIN, room_id};

const MESSAGE_ID: &str = "msg-141-file";
const ASSET_ID: &str = "3-2-asset-141";
const ASSET_TOKEN: &str = "asset-token-141";
const KEY: [u8; 32] = [0x41; 32];
const IV: [u8; 16] = [0x14; 16];
const PLAINTEXT: &[u8] = b"the file bob sent, fetched from the asset store";
/// The signature part of the pre-signed URL — a credential for its
/// lifetime, never in an error.
const SIGNATURE: &str = "X-Amz-Signature=sig141";

/// `iv(16) || aes-256-cbc-pkcs7(plaintext)` — the ADR-0011 blob.
fn encrypt(plaintext: &[u8]) -> Vec<u8> {
    let cipher = cbc::Encryptor::<Aes256>::new(&KEY.into(), &IV.into());
    let mut blob = IV.to_vec();
    blob.extend(cipher.encrypt_padded_vec_mut::<Pkcs7>(plaintext));
    blob
}

/// The received file's rows, as the receive path persists them: the
/// digest covers the blob the sender uploaded.
async fn seed_received_file(store: &dyn kupferdraht::storage::Store, blob: &[u8]) {
    store
        .insert_timeline_event(&TimelineEventRecord {
            message_id: MESSAGE_ID.into(),
            room_id: room_id(),
            sender_user_id: format!("{BOB}@{DOMAIN}"),
            sender_device_id: "bobdev01".into(),
            kind: "asset".into(),
            body: "notes.txt".into(),
            sent_at_ms: 1,
            delivery_status: "received".into(),
            ..TimelineEventRecord::default()
        })
        .await
        .expect("seed the timeline row");
    store
        .insert_attachment(&AttachmentRecord {
            message_id: MESSAGE_ID.into(),
            room_id: room_id(),
            mime_type: "text/plain".into(),
            filename: Some("notes.txt".into()),
            size_bytes: PLAINTEXT.len() as u64,
            sha256: Sha256::digest(blob).to_vec(),
            otr_key_ct: KEY.to_vec(),
            asset_id: ASSET_ID.into(),
            asset_token: Some(ASSET_TOKEN.into()),
            asset_domain: DOMAIN.into(),
            encryption_algo: "AES_CBC".into(),
            cache_path: None,
            created_at_ms: 1,
        })
        .await
        .expect("seed the attachment row");
}

/// The API host answers the download with `302` to `asset_store`.
fn mount_redirecting_download<'a>(
    api: &'a MockServer,
    asset_store: &MockServer,
) -> httpmock::Mock<'a> {
    let location = asset_store.url(format!("/wire-assets/v3/otr/{ASSET_ID}?{SIGNATURE}"));
    api.mock(|when, then| {
        when.method(GET)
            .path(format!("/v14/assets/{DOMAIN}/{ASSET_ID}"))
            .header("asset-token", ASSET_TOKEN);
        then.status(302).header("location", location);
    })
}

/// The asset store serves `blob` at the pre-signed path.
fn mount_asset_store<'a>(asset_store: &'a MockServer, blob: &[u8]) -> httpmock::Mock<'a> {
    asset_store.mock(|when, then| {
        when.method(GET)
            .path(format!("/wire-assets/v3/otr/{ASSET_ID}"))
            .query_param("X-Amz-Signature", "sig141");
        then.status(200)
            .header("content-type", "application/octet-stream")
            .body(blob);
    })
}

async fn received_file(fx: &drain_fixture::Fixture) -> kupferdraht::AssetSummary {
    let room = fx.client.room(&room_id()).await.expect("room joined");
    room.timeline()
        .expect("timeline")
        .asset(MESSAGE_ID)
        .await
        .expect("asset lookup")
        .expect("the seeded file")
}

#[tokio::test]
async fn a_received_file_downloads_through_the_redirect() {
    let api = MockServer::start();
    let asset_store = MockServer::start();
    let fx = drain_fixture::joined_client(&api).await;
    let blob = encrypt(PLAINTEXT);
    seed_received_file(fx.store.as_ref(), &blob).await;
    let redirect = mount_redirecting_download(&api, &asset_store);
    let bytes = mount_asset_store(&asset_store, &blob);

    let plaintext = received_file(&fx)
        .await
        .download()
        .await
        .expect("the redirected download decrypts");

    assert_eq!(plaintext, PLAINTEXT);
    redirect.assert_calls(1);
    bytes.assert_calls(1);
}

#[tokio::test]
async fn a_received_file_saves_through_the_redirect() {
    let api = MockServer::start();
    let asset_store = MockServer::start();
    let fx = drain_fixture::joined_client(&api).await;
    let blob = encrypt(PLAINTEXT);
    seed_received_file(fx.store.as_ref(), &blob).await;
    mount_redirecting_download(&api, &asset_store);
    mount_asset_store(&asset_store, &blob);
    let dest = tempfile::tempdir().expect("tempdir");
    let path = dest.path().join("notes.txt");

    received_file(&fx)
        .await
        .save_to(&path)
        .await
        .expect("save_to through the redirect");

    assert_eq!(std::fs::read(&path).expect("saved file"), PLAINTEXT);
}

/// The hop's host is the backend's choice; what it serves is checked
/// against the digest the sender put in the MLS message.
#[tokio::test]
async fn other_bytes_behind_the_redirect_fail_the_digest() {
    let api = MockServer::start();
    let asset_store = MockServer::start();
    let fx = drain_fixture::joined_client(&api).await;
    seed_received_file(fx.store.as_ref(), &encrypt(PLAINTEXT)).await;
    mount_redirecting_download(&api, &asset_store);
    let substitute = encrypt(b"a file the sender never sent");
    let bytes = mount_asset_store(&asset_store, &substitute);

    let err = received_file(&fx)
        .await
        .download()
        .await
        .expect_err("other bytes must not decrypt");

    assert!(
        matches!(err, KupferdrahtError::IntegrityCheck(_)),
        "a substituted blob fails the digest, got {err:?}"
    );
    bytes.assert_calls(1);
}
