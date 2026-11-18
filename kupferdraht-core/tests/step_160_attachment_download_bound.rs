// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 160 — a received file downloads up to the size its sender declared.
//!
//! `Asset.Original.size` travels in the MLS-protected message, which the
//! server cannot change; the ciphertext is at most 32 bytes longer (the IV
//! and a block of padding, ADR-0011). A download past that bound stops with
//! `IntegrityCheck` before the digest is reached, so a hostile server cannot
//! fill the disk or memory, and a sender that declares less than it uploads
//! has its file refused. These tests take the consumer's path
//! (`Timeline::attachment` → `download` / `save_to`); the transport's own bound
//! and the browser's ceiling are unit tests beside the code
//! (`wire/http/attachment_bound_tests.rs`).

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

const MESSAGE_ID: &str = "msg-160-file";
const ATTACHMENT_ID: &str = "3-2-attachment-160";
const ATTACHMENT_TOKEN: &str = "asset-token-160";
const KEY: [u8; 32] = [0x60; 32];
const IV: [u8; 16] = [0x16; 16];
/// Three whole blocks: its ciphertext is the bound exactly, 48 + 32 bytes.
const THREE_BLOCKS: &[u8] = b"forty-eight bytes of a file that fills 3 blocks.";

/// `iv(16) || aes-256-cbc-pkcs7(plaintext)` — the ADR-0011 blob.
fn encrypt(plaintext: &[u8]) -> Vec<u8> {
    let cipher = cbc::Encryptor::<Aes256>::new(&KEY.into(), &IV.into());
    let mut blob = IV.to_vec();
    blob.extend(cipher.encrypt_padded_vec_mut::<Pkcs7>(plaintext));
    blob
}

/// The received file's rows: the digest covers `blob`, the sender declared
/// `declared_size` bytes.
async fn seed_received_file(
    store: &dyn kupferdraht::storage::Store,
    blob: &[u8],
    declared_size: u64,
) {
    store
        .insert_timeline_event(&TimelineEventRecord {
            message_id: MESSAGE_ID.into(),
            room_id: room_id(),
            sender_user_id: format!("{BOB}@{DOMAIN}"),
            sender_device_id: "bobdev01".into(),
            kind: "attachment".into(),
            body: "notes.txt".into(),
            sent_at_ms: 1,
            arrived_at_ms: 1,
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
            size_bytes: declared_size,
            sha256: Sha256::digest(blob).to_vec(),
            key_ct: KEY.to_vec(),
            attachment_id: ATTACHMENT_ID.into(),
            attachment_token: Some(ATTACHMENT_TOKEN.into()),
            attachment_domain: DOMAIN.into(),
            cipher: "AES_CBC".into(),
            cache_path: None,
            created_at_ms: 1,
        })
        .await
        .expect("seed the attachment row");
}

/// The API host serves `blob`.
fn mount_download<'a>(api: &'a MockServer, blob: &[u8]) -> httpmock::Mock<'a> {
    api.mock(|when, then| {
        when.method(GET)
            .path(format!("/v14/assets/{DOMAIN}/{ATTACHMENT_ID}"))
            .header("asset-token", ATTACHMENT_TOKEN);
        then.status(200)
            .header("content-type", "application/octet-stream")
            .body(blob);
    })
}

async fn received_file(fx: &drain_fixture::Fixture) -> kupferdraht::AttachmentSummary {
    let room = fx.client.room(&room_id()).await.expect("room joined");
    room.timeline()
        .expect("timeline")
        .attachment(MESSAGE_ID)
        .await
        .expect("attachment lookup")
        .expect("the seeded file")
}

/// The bound is the declared size plus 32, not less: a plaintext of whole
/// blocks pads a block of its own.
#[tokio::test]
async fn a_file_of_whole_blocks_downloads_at_the_bound() {
    let api = MockServer::start();
    let fx = drain_fixture::joined_client(&api).await;
    let blob = encrypt(THREE_BLOCKS);
    assert_eq!(blob.len(), THREE_BLOCKS.len() + 32, "the bound exactly");
    seed_received_file(fx.store.as_ref(), &blob, THREE_BLOCKS.len() as u64).await;
    mount_download(&api, &blob);

    let plaintext = received_file(&fx)
        .await
        .download()
        .await
        .expect("a file at the bound downloads");

    assert_eq!(plaintext, THREE_BLOCKS);
}

/// The digest matches, the blob decrypts — but it is longer than the
/// sender said: refused.
#[tokio::test]
async fn a_file_longer_than_its_declared_size_is_refused() {
    let api = MockServer::start();
    let fx = drain_fixture::joined_client(&api).await;
    let blob = encrypt(THREE_BLOCKS);
    seed_received_file(fx.store.as_ref(), &blob, 16).await;
    mount_download(&api, &blob);

    let err = received_file(&fx)
        .await
        .download()
        .await
        .expect_err("more bytes than the sender declared");

    assert!(
        matches!(err, KupferdrahtError::IntegrityCheck(_)),
        "longer than declared, got {err:?}"
    );
}

#[tokio::test]
async fn a_file_longer_than_its_declared_size_is_not_saved() {
    let api = MockServer::start();
    let fx = drain_fixture::joined_client(&api).await;
    let blob = encrypt(THREE_BLOCKS);
    seed_received_file(fx.store.as_ref(), &blob, 16).await;
    mount_download(&api, &blob);
    let dest = tempfile::tempdir().expect("tempdir");
    let path = dest.path().join("notes.txt");

    let err = received_file(&fx)
        .await
        .save_to(&path)
        .await
        .expect_err("more bytes than the sender declared");

    assert!(
        matches!(err, KupferdrahtError::IntegrityCheck(_)),
        "longer than declared, got {err:?}"
    );
    assert!(!path.exists(), "nothing is saved");
}
