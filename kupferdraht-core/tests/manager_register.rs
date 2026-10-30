// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! `Kupferdraht::register` → `PendingAccount::activate` integration tests.
//!
//! Activation through the manager must close the registration loop the
//! way `Kupferdraht::login` does: write the device-registry `AccountEntry`,
//! mark the account active, and persist the session so a later
//! `Kupferdraht::resume` works without a Wire login. The builder-level
//! `ClientBuilder::register` flow (Step 16c) deliberately does none of
//! that — it stays the registry-less, process-separated CLI path.

use std::sync::Arc;

use httpmock::Method::POST;
use httpmock::MockServer;
use kupferdraht::keystore::{InMemoryKeyStore, SecureKeyStore};
use kupferdraht::{Kupferdraht, KupferdrahtError};
use tempfile::TempDir;

mod common;

/// Wire's `/register` returns 201 with a `User`-schema body. The
/// `user_id` for Kupferdraht comes from `qualified_id.id`.
fn mount_register_ok(server: &MockServer, qualified_user_id: &str, name: &str, email: &str) {
    let body = format!(
        r#"{{
          "qualified_id": {{"id":"{qualified_user_id}","domain":"example.test"}},
          "name":"{name}",
          "accent_id":0,
          "status":"ephemeral",
          "locale":"en",
          "email":"{email}"
        }}"#
    );
    server.mock(|when, then| {
        when.method(POST).path("/register");
        then.status(201)
            .header("content-type", "application/json")
            .body(body);
    });
}

fn mount_activate_ok(server: &MockServer, email: &str) {
    let body = format!(r#"{{"email":"{email}","first":true}}"#);
    server.mock(|when, then| {
        when.method(POST).path("/activate");
        then.status(200)
            .header("content-type", "application/json")
            .body(body);
    });
}

fn open_manager(dir: &TempDir) -> Kupferdraht {
    let keystore: Arc<dyn SecureKeyStore> = Arc::new(InMemoryKeyStore::new());
    Kupferdraht::open(dir.path().to_path_buf(), keystore)
        .expect("open kupferdraht")
        .permit_http()
}

async fn run_register(app: &Kupferdraht, server: &MockServer) -> kupferdraht::PendingAccount {
    app.register(
        common::server_config(&server.base_url()),
        "Alice".into(),
        "alice@example.test".into(),
        "hunter2hunter2".into(),
    )
    .await
    .expect("register must succeed")
}

#[tokio::test]
async fn kupferdraht_register_activate_registers_account_and_marks_active() {
    let dir = TempDir::new().expect("tempdir");
    let server = MockServer::start();
    mount_register_ok(&server, "user-uuid", "Alice", "alice@example.test");
    mount_activate_ok(&server, "alice@example.test");
    common::mount_full_login_flow(&server);
    let app = open_manager(&dir);

    let pending = run_register(&app, &server).await;

    // `register` alone must not touch the registry — the account is
    // unusable until activated.
    assert!(
        app.accounts().await.expect("accounts").is_empty(),
        "register must not write the registry before activation"
    );

    let client = pending.activate("good-code").await.expect("activate");

    let accounts = app.accounts().await.expect("accounts");
    assert_eq!(accounts.len(), 1, "activate must register the account");
    assert_eq!(
        accounts[0].account_id,
        client.user_id().expect("user_id").as_str()
    );
    assert_eq!(
        app.active_account().await.expect("active").as_deref(),
        Some(client.user_id().expect("user_id").as_str())
    );
}

#[tokio::test]
async fn kupferdraht_register_activate_persists_session_for_resume() {
    let dir = TempDir::new().expect("tempdir");
    let server = MockServer::start();
    mount_register_ok(&server, "user-uuid", "Alice", "alice@example.test");
    mount_activate_ok(&server, "alice@example.test");
    common::mount_full_login_flow(&server);
    let app = open_manager(&dir);

    let pending = run_register(&app, &server).await;
    let client = pending.activate("good-code").await.expect("activate");
    let account_id = client.user_id().expect("user_id").as_str().to_owned();
    drop(client);

    // The headline defect: a registered-then-activated account must be
    // resumable like a logged-in one (proves the session was persisted).
    let resumed = app.resume(&account_id).await.expect("resume");
    assert_eq!(resumed.user_id().expect("user_id").as_str(), account_id);
}

#[tokio::test]
async fn kupferdraht_register_failed_activation_leaves_registry_empty_and_handle_retryable() {
    let dir = TempDir::new().expect("tempdir");
    let server = MockServer::start();
    mount_register_ok(&server, "user-uuid", "Alice", "alice@example.test");
    let mut bad_activate = server.mock(|when, then| {
        when.method(POST).path("/activate");
        then.status(404)
            .header("content-type", "application/json")
            .body(r#"{"code":404,"label":"invalid-code","message":"Invalid activation code"}"#);
    });
    let app = open_manager(&dir);
    let pending = run_register(&app, &server).await;

    let err = pending
        .activate("wrong-code")
        .await
        .expect_err("wrong code must propagate");
    assert!(matches!(err, KupferdrahtError::BackendRejected(_)));
    assert!(
        app.accounts().await.expect("accounts").is_empty(),
        "failed activation must not register the account"
    );

    // Swap in a successful backend; the same handle retries cleanly.
    bad_activate.delete();
    mount_activate_ok(&server, "alice@example.test");
    common::mount_full_login_flow(&server);
    let client = pending.activate("good-code").await.expect("retry");
    assert_eq!(app.accounts().await.expect("accounts").len(), 1);
    assert_eq!(
        app.active_account().await.expect("active").as_deref(),
        client.user_id().map(kupferdraht::UserId::as_str)
    );
}

#[tokio::test]
async fn pending_account_exposes_user_id_and_email() {
    let dir = TempDir::new().expect("tempdir");
    let server = MockServer::start();
    mount_register_ok(&server, "user-uuid", "Alice", "alice@example.test");
    let app = open_manager(&dir);

    let pending = run_register(&app, &server).await;
    assert_eq!(pending.user_id(), "user-uuid");
    assert_eq!(pending.email(), "alice@example.test");
}
