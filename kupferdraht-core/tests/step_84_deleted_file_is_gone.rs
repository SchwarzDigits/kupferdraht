// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 84 — a file deleted for everyone is gone on this device.
//!
//! A delete for everyone, own or received, cleared only the message's text:
//! the file's row (its key, token and locator) and its cached ciphertext
//! stayed, and the file could still be opened. Now the delete takes the row
//! in its transaction and the cached copy after it; an own delete also
//! removes the upload at the server, best effort. A handle the app took
//! before the delete refuses to download. A failed upload leaves no staged
//! copy, and a client's start removes what no row names any more (a crash
//! between the commit and the removal, a history cleared at login).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use httpmock::Method::{DELETE, GET, POST};
use httpmock::{Mock, MockServer};
use kupferdraht::keystore::{InMemoryKeyStore, SecureKeyStore};
use kupferdraht::storage::{AttachmentRecord, Store, TimelineEventRecord};
use kupferdraht::{AttachmentSummary, Client, Kupferdraht, KupferdrahtError};

mod common;

use common::drain_fixture::{self, BOB, DOMAIN, Fixture, Notif, room_id};

/// The fixture's own account (`common::login_mocks`).
const SELF: &str = "user-uuid@example.test";
const SELF_DEVICE: &str = "new-client-id";
const FILE_ID: &str = "msg-84-file";
const ATTACHMENT_ID: &str = "3-2-attachment-84";
const ATTACHMENT_TOKEN: &str = "asset-token-84";

fn bob() -> String {
    format!("{BOB}@{DOMAIN}")
}

// ---- the file ----

fn file_row(message_id: &str, room_id: &str, sender: &str, device: &str) -> TimelineEventRecord {
    TimelineEventRecord {
        message_id: message_id.into(),
        room_id: room_id.into(),
        sender_user_id: sender.into(),
        sender_device_id: device.into(),
        kind: "attachment".into(),
        body: "notes.txt".into(),
        sent_at_ms: 1,
        arrived_at_ms: 1,
        delivery_status: "received".into(),
        ..TimelineEventRecord::default()
    }
}

fn attachment(message_id: &str, room_id: &str) -> AttachmentRecord {
    AttachmentRecord {
        message_id: message_id.into(),
        room_id: room_id.into(),
        mime_type: "text/plain".into(),
        filename: Some("notes.txt".into()),
        size_bytes: 16,
        sha256: vec![0x84; 32],
        key_ct: vec![0x48; 32],
        attachment_id: ATTACHMENT_ID.into(),
        attachment_token: Some(ATTACHMENT_TOKEN.into()),
        attachment_domain: DOMAIN.into(),
        cipher: "AES_CBC".into(),
        cache_path: None,
        created_at_ms: 1,
    }
}

/// The file `sender` sent to `room_id`: its timeline row and its key's row.
async fn seed_file(store: &dyn Store, message_id: &str, room_id: &str, sender: &str, device: &str) {
    store
        .insert_timeline_event(&file_row(message_id, room_id, sender, device))
        .await
        .expect("seed the timeline row");
    store
        .insert_attachment(&attachment(message_id, room_id))
        .await
        .expect("seed the file row");
}

/// `<home>/accounts/<self>/attachments` — the account's attachment cache.
fn attachments_dir(home: &Path) -> PathBuf {
    home.join("accounts").join(SELF).join("attachments")
}

/// A cached ciphertext of the fixture's file, recorded on its row.
async fn cache_file(store: &dyn Store, home: &Path) -> PathBuf {
    let dir = attachments_dir(home);
    std::fs::create_dir_all(&dir).expect("attachments dir");
    let path = dir.join(format!("{FILE_ID}.enc"));
    std::fs::write(&path, b"ciphertext").expect("cache file");
    store
        .update_attachment_cache_path(FILE_ID, &room_id(), path.to_str())
        .await
        .expect("record the cache path");
    path
}

async fn file_row_of(fx: &Fixture) -> Option<AttachmentRecord> {
    fx.store
        .get_attachment(FILE_ID, &room_id())
        .await
        .expect("attachment lookup")
}

async fn file_handle(fx: &Fixture) -> Option<AttachmentSummary> {
    let room = fx.client.room(&room_id()).await.expect("room joined");
    room.timeline()
        .expect("timeline")
        .attachment(FILE_ID)
        .await
        .expect("attachment lookup")
}

// ---- the server ----

/// `DELETE /v14/assets/{domain}/{id}` of the fixture's file, answering `status`.
fn mount_server_delete(api: &MockServer, status: u16) -> Mock<'_> {
    api.mock(|when, then| {
        when.method(DELETE)
            .path(format!("/v14/assets/{DOMAIN}/{ATTACHMENT_ID}"));
        then.status(status);
    })
}

/// `GET /v14/assets/{domain}/{id}` of the fixture's file.
fn mount_download(api: &MockServer) -> Mock<'_> {
    api.mock(|when, then| {
        when.method(GET)
            .path(format!("/v14/assets/{DOMAIN}/{ATTACHMENT_ID}"));
        then.status(200)
            .header("content-type", "application/octet-stream")
            .body(vec![0u8; 48]);
    })
}

// ---- the deletes ----

async fn redact(fx: &Fixture) -> Result<(), KupferdrahtError> {
    let room = fx.client.room(&room_id()).await.expect("room joined");
    room.timeline()
        .expect("timeline")
        .redact(FILE_ID)
        .await
        .map(|_| ())
}

/// `GenericMessage { message_id = 1, deleted = 14 { message_id = 1 } }`.
fn delete_message(message_id: &str, target: &str) -> Vec<u8> {
    let mut deleted = Vec::new();
    drain_fixture::put_len_delimited(1, target.as_bytes(), &mut deleted);
    let mut out = Vec::new();
    drain_fixture::put_len_delimited(1, message_id.as_bytes(), &mut out);
    drain_fixture::put_len_delimited(14, &deleted, &mut out);
    out
}

/// Bob deletes the file for everyone; the client drains it.
async fn receive_bobs_delete(api: &MockServer, fx: &mut Fixture) {
    let tls = fx
        .bob
        .application_message(&delete_message("msg-84-delete", FILE_ID));
    drain_fixture::mount_page(
        api,
        Some("n1"),
        &[Notif {
            id: "n2",
            events: vec![drain_fixture::mls_event(&tls)],
        }],
    );
    drain_fixture::drain(&fx.client).await;
}

// ---- an own delete ----

#[tokio::test]
async fn an_own_delete_removes_the_files_key_and_cached_copy() {
    let api = MockServer::start();
    let fx = drain_fixture::joined_client(&api).await;
    drain_fixture::mount_message_upload(&api);
    mount_server_delete(&api, 200);
    seed_file(fx.store.as_ref(), FILE_ID, &room_id(), SELF, SELF_DEVICE).await;
    let cached = cache_file(fx.store.as_ref(), fx._home.path()).await;

    redact(&fx).await.expect("an own delete");

    assert!(
        file_row_of(&fx).await.is_none(),
        "the key goes with the message"
    );
    assert!(!cached.exists(), "the cached copy goes with the message");
    assert!(
        file_handle(&fx).await.is_none(),
        "the timeline offers no file"
    );
}

#[tokio::test]
async fn an_own_delete_removes_the_upload_at_the_server() {
    let api = MockServer::start();
    let fx = drain_fixture::joined_client(&api).await;
    drain_fixture::mount_message_upload(&api);
    let server_delete = mount_server_delete(&api, 200);
    seed_file(fx.store.as_ref(), FILE_ID, &room_id(), SELF, SELF_DEVICE).await;

    redact(&fx).await.expect("an own delete");

    server_delete.assert_calls(1);
}

/// The server's copy goes best effort: a refusal is logged, the delete stands.
#[tokio::test]
async fn a_refused_server_delete_does_not_fail_the_own_delete() {
    let api = MockServer::start();
    let fx = drain_fixture::joined_client(&api).await;
    drain_fixture::mount_message_upload(&api);
    mount_server_delete(&api, 403);
    seed_file(fx.store.as_ref(), FILE_ID, &room_id(), SELF, SELF_DEVICE).await;

    redact(&fx).await.expect("the delete stands");

    assert!(
        file_row_of(&fx).await.is_none(),
        "the key goes with the message"
    );
}

/// A cached copy the delete cannot remove is logged, not raised; the next
/// start removes what no row names.
#[tokio::test]
async fn a_cached_copy_that_cannot_be_removed_does_not_fail_the_delete() {
    let api = MockServer::start();
    let fx = drain_fixture::joined_client(&api).await;
    drain_fixture::mount_message_upload(&api);
    mount_server_delete(&api, 200);
    seed_file(fx.store.as_ref(), FILE_ID, &room_id(), SELF, SELF_DEVICE).await;
    let blocked = attachments_dir(fx._home.path()).join(format!("{FILE_ID}.enc"));
    std::fs::create_dir_all(&blocked).expect("a directory where the file would be");
    std::fs::write(blocked.join("inside"), b"x").expect("not empty");

    redact(&fx).await.expect("the delete stands");

    assert!(
        file_row_of(&fx).await.is_none(),
        "the key goes with the message"
    );
}

// ---- a received delete ----

#[tokio::test]
async fn a_received_delete_removes_the_files_key_and_cached_copy() {
    let api = MockServer::start();
    let mut fx = drain_fixture::joined_client(&api).await;
    let server_delete = mount_server_delete(&api, 200);
    seed_file(fx.store.as_ref(), FILE_ID, &room_id(), &bob(), "bobdev01").await;
    let cached = cache_file(fx.store.as_ref(), fx._home.path()).await;

    receive_bobs_delete(&api, &mut fx).await;

    assert!(
        file_row_of(&fx).await.is_none(),
        "the key goes with the message"
    );
    assert!(!cached.exists(), "the cached copy goes with the message");
    assert!(
        file_handle(&fx).await.is_none(),
        "the timeline offers no file"
    );
    server_delete.assert_calls(0);
}

#[tokio::test]
async fn a_received_delete_of_a_file_never_downloaded_removes_its_key() {
    let api = MockServer::start();
    let mut fx = drain_fixture::joined_client(&api).await;
    seed_file(fx.store.as_ref(), FILE_ID, &room_id(), &bob(), "bobdev01").await;

    receive_bobs_delete(&api, &mut fx).await;

    assert!(
        file_row_of(&fx).await.is_none(),
        "the key goes with the message"
    );
    assert!(
        fx.recorder.errors().is_empty(),
        "{:?}",
        fx.recorder.errors()
    );
}

// ---- a handle from before the delete ----

#[tokio::test]
async fn a_handle_from_before_the_delete_refuses_to_download() {
    let api = MockServer::start();
    let mut fx = drain_fixture::joined_client(&api).await;
    let download = mount_download(&api);
    seed_file(fx.store.as_ref(), FILE_ID, &room_id(), &bob(), "bobdev01").await;
    let handle = file_handle(&fx).await.expect("the file before its delete");

    receive_bobs_delete(&api, &mut fx).await;
    let err = handle.download().await.expect_err("a deleted file");

    assert!(
        matches!(err, KupferdrahtError::InvalidInput(_)),
        "got {err:?}"
    );
    download.assert_calls(0);
    let cached = attachments_dir(fx._home.path()).join(format!("{FILE_ID}.enc"));
    assert!(!cached.exists(), "nothing is cached again");
}

#[tokio::test]
async fn a_handle_from_before_the_delete_refuses_to_save() {
    let api = MockServer::start();
    let mut fx = drain_fixture::joined_client(&api).await;
    let download = mount_download(&api);
    seed_file(fx.store.as_ref(), FILE_ID, &room_id(), &bob(), "bobdev01").await;
    let handle = file_handle(&fx).await.expect("the file before its delete");
    let dest = tempfile::tempdir().expect("tempdir");
    let path = dest.path().join("notes.txt");

    receive_bobs_delete(&api, &mut fx).await;
    let err = handle.save_to(&path).await.expect_err("a deleted file");

    assert!(
        matches!(err, KupferdrahtError::InvalidInput(_)),
        "got {err:?}"
    );
    download.assert_calls(0);
    assert!(!path.exists(), "nothing is saved");
}

// ---- a failed upload ----

fn staged_files(home: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(attachments_dir(home).join("staging"))
        .map(|dir| dir.map(|entry| entry.expect("entry").path()).collect())
        .unwrap_or_default()
}

#[tokio::test]
async fn a_failed_upload_leaves_no_staged_copy() {
    let api = MockServer::start();
    let fx = drain_fixture::joined_client(&api).await;
    api.mock(|when, then| {
        when.method(POST).path("/v14/assets");
        then.status(500)
            .body(r#"{"code":500,"label":"server-error"}"#);
    });
    let source = tempfile::tempdir().expect("tempdir");
    let path = source.path().join("notes.txt");
    std::fs::write(&path, b"a file whose upload fails").expect("write");
    let room = fx.client.room(&room_id()).await.expect("room joined");

    room.timeline()
        .expect("timeline")
        .send_file(&path, "text/plain")
        .await
        .expect_err("the upload fails");

    assert_eq!(staged_files(fx._home.path()), Vec::<PathBuf>::new());
}

// ---- a client's start ----

/// The app's home in `home`, as an app opens it.
fn open_app(home: &Path) -> Kupferdraht {
    let keystore: Arc<dyn SecureKeyStore> = Arc::new(InMemoryKeyStore::new());
    Kupferdraht::open(home.to_path_buf(), keystore)
        .expect("open the home")
        .permit_http()
}

/// A client logged in to `api` through the app, which records the session.
async fn login(app: &Kupferdraht, api: &MockServer) -> Arc<Client> {
    app.login(
        common::server_config(&api.base_url()),
        "alice@example.test".into(),
        "pw".into(),
        None,
        None,
    )
    .await
    .expect("login")
    .register_device()
    .await
    .expect("register device")
}

/// `(kept, orphan, staged)` in the account's attachment cache: a file its row
/// names, one no row names, and a staged copy of an interrupted send.
fn leftovers(home: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let dir = attachments_dir(home);
    std::fs::create_dir_all(dir.join("staging")).expect("attachments dir");
    let files = (
        dir.join("kept-file.enc"),
        dir.join("orphan-file.enc"),
        dir.join("staging").join("attachment-0123.enc"),
    );
    for path in [&files.0, &files.1, &files.2] {
        std::fs::write(path, b"ciphertext").expect("a cached file");
    }
    files
}

#[tokio::test]
async fn a_start_removes_what_no_row_names_and_keeps_the_rest() {
    let api = MockServer::start();
    common::mount_full_login_flow(&api);
    let home = tempfile::tempdir().expect("home");
    let app = open_app(home.path());
    let client = login(&app, &api).await;
    let store = client.store().expect("client store");
    seed_file(
        store.as_ref(),
        "kept-file",
        "room@example.test",
        &bob(),
        "bobdev01",
    )
    .await;
    let (kept, orphan, staged) = leftovers(home.path());
    drop(store);
    drop(client);

    let _resumed = app.resume(SELF).await.expect("resume");

    assert!(kept.exists(), "a file its row names stays");
    assert!(!orphan.exists(), "a file no row names goes");
    assert!(
        !staged.exists(),
        "a staged copy of an interrupted send goes"
    );
}

/// A login starts a client too — and a device that replaced a vanished one
/// has just cleared every row.
#[tokio::test]
async fn a_login_removes_what_no_row_names() {
    let api = MockServer::start();
    common::mount_full_login_flow(&api);
    let home = tempfile::tempdir().expect("home");
    let app = open_app(home.path());
    let (kept, orphan, staged) = leftovers(home.path());

    let _client = login(&app, &api).await;

    for path in [kept, orphan, staged] {
        assert!(!path.exists(), "{} goes: no row names it", path.display());
    }
}
