// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Slice 2 — the `Kupferdraht` device manager: logging in through the
//! manager registers the account in the device `GlobalStore` and marks
//! it active, so an account picker can later list and resume it.

use std::sync::Arc;

use httpmock::MockServer;
use kupferdraht::Kupferdraht;
use kupferdraht::keystore::{InMemoryKeyStore, SecureKeyStore};
use tempfile::TempDir;

mod common;

#[tokio::test]
async fn kupferdraht_login_registers_account_and_marks_active() {
    let dir = TempDir::new().expect("tempdir");
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    let keystore: Arc<dyn SecureKeyStore> = Arc::new(InMemoryKeyStore::new());

    let app = Kupferdraht::open(dir.path().to_path_buf(), keystore)
        .expect("open kupferdraht")
        .permit_http();

    // Nothing registered before the first login.
    assert!(app.accounts().await.expect("accounts").is_empty());
    assert!(app.active_account().await.expect("active").is_none());

    let client = app
        .login(
            common::server_config(&server.base_url()),
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

    // The account now appears in the device registry, marked active.
    let accounts = app.accounts().await.expect("accounts");
    assert_eq!(accounts.len(), 1, "login must register the account");
    assert_eq!(
        accounts[0].account_id,
        client.user_id().expect("user_id").as_str()
    );
    assert!(
        !accounts[0].server.is_empty(),
        "registry must record the account's server"
    );
    assert_eq!(
        app.active_account().await.expect("active").as_deref(),
        Some(client.user_id().expect("user_id").as_str())
    );
}

#[tokio::test]
async fn kupferdraht_login_uses_per_account_db_directory() {
    let dir = TempDir::new().expect("tempdir");
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    let keystore: Arc<dyn SecureKeyStore> = Arc::new(InMemoryKeyStore::new());

    let app = Kupferdraht::open(dir.path().to_path_buf(), keystore)
        .expect("open kupferdraht")
        .permit_http();

    let client = app
        .login(
            common::server_config(&server.base_url()),
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

    // The per-account databases live under accounts/<account_id>/ —
    // not at the shared base. The per-account database key lives in
    // the keystore (under `account-<id>`), not on disk.
    let account_dir = dir
        .path()
        .join("accounts")
        .join(client.user_id().expect("user_id").as_str());
    assert!(
        account_dir.join("kupferdraht.db").is_file(),
        "expected per-account kupferdraht.db under {}",
        account_dir.display()
    );
    assert!(
        account_dir.join("mls.db").is_file(),
        "expected per-account mls.db under {}",
        account_dir.display()
    );
    assert!(
        !dir.path().join("kupferdraht.db").exists(),
        "the shared single-account kupferdraht.db must not be used"
    );
}

#[tokio::test]
async fn kupferdraht_resume_returns_registered_account_and_rejects_unknown() {
    let dir = TempDir::new().expect("tempdir");
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    let keystore: Arc<dyn SecureKeyStore> = Arc::new(InMemoryKeyStore::new());

    let app = Kupferdraht::open(dir.path().to_path_buf(), keystore)
        .expect("open kupferdraht")
        .permit_http();

    let account_id = {
        let client = app
            .login(
                common::server_config(&server.base_url()),
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
        client.user_id().expect("user_id").as_str().to_string()
    };

    // Resume the registered account from disk (no Wire login).
    let resumed = app.resume(&account_id).await.expect("resume");
    assert_eq!(resumed.user_id().expect("user_id").as_str(), account_id);

    // An account that was never registered is rejected.
    assert!(
        app.resume("ghost@example.test").await.is_err(),
        "resuming an unknown account must fail"
    );
}

#[tokio::test]
async fn kupferdraht_resume_returns_the_same_live_client_instance() {
    // Step 39: two resumes of one account must hand back the *same* live
    // `Client`, not two racing instances — the second is served from the
    // manager's per-account cache. Two Clients would each rebind the `zuid`
    // and one would invalidate the other's cookie (403).
    let dir = TempDir::new().expect("tempdir");
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    let keystore: Arc<dyn SecureKeyStore> = Arc::new(InMemoryKeyStore::new());

    let app = Kupferdraht::open(dir.path().to_path_buf(), keystore)
        .expect("open kupferdraht")
        .permit_http();

    let account_id = {
        let client = app
            .login(
                common::server_config(&server.base_url()),
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
        client.user_id().expect("user_id").as_str().to_string()
    };

    let first = app.resume(&account_id).await.expect("first resume");
    let second = app.resume(&account_id).await.expect("second resume");
    assert!(
        Arc::ptr_eq(&first, &second),
        "two resumes of one account must return the same live Client"
    );
}

#[tokio::test]
async fn kupferdraht_relogin_reuses_existing_device() {
    use httpmock::Method::GET;
    // Step 40: a re-login on an existing account must REUSE the registered
    // device, not register a new one. The reuse path looks up the persisted
    // session by the qualified `<uuid>@<domain>` user id; the regression it
    // guards is that lookup using the bare uuid, missing the row, and silently
    // registering a fresh device on every re-login (piling up devices).
    let dir = TempDir::new().expect("tempdir");
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    // The reuse path verifies the persisted device still exists on Wire. Only
    // the reuse branch reaches this call — the buggy bare-key lookup bails at
    // `load_session` first, so it never gets here.
    let client_exists = server.mock(|when, then| {
        when.method(GET).path("/v14/clients/new-client-id");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"id":"new-client-id","type":"permanent"}"#);
    });
    let keystore: Arc<dyn SecureKeyStore> = Arc::new(InMemoryKeyStore::new());
    let app = Kupferdraht::open(dir.path().to_path_buf(), keystore)
        .expect("open kupferdraht")
        .permit_http();

    // First login registers the device + persists the session.
    let device1 = {
        let client = app
            .login(
                common::server_config(&server.base_url()),
                "alice@example.test".into(),
                "pw".into(),
                None,
                None,
            )
            .await
            .expect("login 1")
            .register_device()
            .await
            .expect("register device 1");
        client.device_id().expect("device_id 1").to_string()
    };

    // Second login (same account) must reuse — reaching client_exists.
    let device2 = app
        .login(
            common::server_config(&server.base_url()),
            "alice@example.test".into(),
            "pw".into(),
            None,
            None,
        )
        .await
        .expect("login 2")
        .register_device()
        .await
        .expect("register device 2")
        .device_id()
        .expect("device_id 2")
        .to_string();

    client_exists.assert_calls(1);
    assert_eq!(device1, device2, "re-login must reuse the same device id");
    assert_eq!(
        app.accounts().await.expect("accounts").len(),
        1,
        "re-login must not create a second account/device entry"
    );
}

#[tokio::test]
async fn kupferdraht_resume_selects_the_named_account_among_several() {
    let dir = TempDir::new().expect("tempdir");
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    let keystore: Arc<dyn SecureKeyStore> = Arc::new(InMemoryKeyStore::new());

    let app = Kupferdraht::open(dir.path().to_path_buf(), keystore)
        .expect("open kupferdraht")
        .permit_http();

    let account_id = {
        let client = app
            .login(
                common::server_config(&server.base_url()),
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
        client.user_id().expect("user_id").as_str().to_string()
    };

    // A second account directory exists on disk, so single-account
    // discovery is ambiguous. Resume must pick the named account by id
    // (open accounts/<id>/kupferdraht.db directly), not scan-and-guess.
    std::fs::create_dir_all(dir.path().join("accounts").join("bob-uuid@example.test"))
        .expect("decoy account dir");

    let resumed = app
        .resume(&account_id)
        .await
        .expect("resume the named account despite the decoy");
    assert_eq!(resumed.user_id().expect("user_id").as_str(), account_id);
}
