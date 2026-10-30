// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 16c — `ClientBuilder::register` → `PendingClient` →
//! `PendingClient::activate` integration tests.
//!
//! Drives the end-to-end register-then-activate flow against an
//! httpmock-backed Wire stand-in. The activate step internally runs
//! the same login pipeline `ClientBuilder::login` runs, so each
//! happy-path test also mounts the eight login-flow endpoints via
//! `common::mount_full_login_flow`.

use std::sync::Arc;

use httpmock::Method::POST;
use httpmock::MockServer;
use kupferdraht::keystore::{InMemoryKeyStore, SecureKeyStore};
use kupferdraht::{ClientBuilder, PendingClient};
use tempfile::TempDir;

mod common;

/// Common 16c register-handle setup. The caller picks a
/// `MockServer` and mounts the registration endpoints it needs;
/// this helper builds the `ClientBuilder` with a tempdir-backed
/// store + in-memory keystore, drives `register`, and returns the
/// `PendingClient`.
async fn run_register(
    server: &MockServer,
    name: &str,
    email: &str,
    password: &str,
) -> Arc<PendingClient> {
    let home = TempDir::new().expect("TempDir");
    let keystore: Arc<dyn SecureKeyStore> = Arc::new(InMemoryKeyStore::new());
    let base = home.path().to_path_buf();
    let _ = Box::leak(Box::new(home));
    ClientBuilder::new()
        .server(common::server_config(&server.base_url()))
        .permit_http()
        .base_dir(base)
        .keystore(keystore)
        .register(name.into(), email.into(), password.into())
        .await
        .expect("register must succeed")
}

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

// ----------------------------------------------------------------
// register
// ----------------------------------------------------------------

#[tokio::test]
async fn register_returns_pending_client_with_user_id_and_email() {
    let server = MockServer::start();
    // The login-flow's `/v14/self` response carries `user-uuid` as
    // its qualified id; mount /register to mint the same id so the
    // post-activate login lines up.
    mount_register_ok(&server, "user-uuid", "Alice", "alice@example.test");
    let pending = run_register(&server, "Alice", "alice@example.test", "hunter2hunter2").await;
    assert_eq!(pending.user_id(), "user-uuid");
    assert_eq!(pending.email(), "alice@example.test");
}

#[tokio::test]
async fn register_409_key_exists_surfaces_as_backend_rejected() {
    use kupferdraht::KupferdrahtError;
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/register");
        then.status(409)
            .header("content-type", "application/json")
            .body(r#"{"code":409,"label":"key-exists","message":"The given e-mail address is in use."}"#);
    });
    let home = TempDir::new().expect("TempDir");
    let keystore: Arc<dyn SecureKeyStore> = Arc::new(InMemoryKeyStore::new());
    let base = home.path().to_path_buf();
    let _ = Box::leak(Box::new(home));
    let err = ClientBuilder::new()
        .server(common::server_config(&server.base_url()))
        .permit_http()
        .base_dir(base)
        .keystore(keystore)
        .register(
            "Alice".into(),
            "dup@example.test".into(),
            "hunter2hunter2".into(),
        )
        .await
        .expect_err("duplicate email must propagate");
    let KupferdrahtError::BackendRejected(msg) = err else {
        panic!("expected BackendRejected, got {err:?}");
    };
    assert!(
        msg.contains("key-exists"),
        "Wire label must be preserved; got {msg:?}"
    );
}

// ----------------------------------------------------------------
// activate
// ----------------------------------------------------------------

#[tokio::test]
async fn pending_activate_drives_activate_then_login_pipeline_and_returns_client() {
    let server = MockServer::start();
    // Order matters only for assertions; httpmock matches on path
    // regardless of mount order.
    mount_register_ok(&server, "user-uuid", "Alice", "alice@example.test");
    mount_activate_ok(&server, "alice@example.test");
    common::mount_full_login_flow(&server);
    let pending = run_register(&server, "Alice", "alice@example.test", "hunter2hunter2").await;
    let client = pending
        .clone()
        .activate("good-code")
        .await
        .expect("activate must succeed");
    // Post-activate Client's user_id must match what /v14/self
    // returned during the embedded login pipeline (the mock common
    // helper returns `user-uuid` — same id we asked /register to
    // mint above, so they line up).
    assert_eq!(
        client.user_id().map(|u| u.as_str()),
        Some("user-uuid@example.test")
    );
}

#[tokio::test]
async fn pending_activate_bad_code_leaves_pending_reusable() {
    use kupferdraht::KupferdrahtError;
    let server = MockServer::start();
    mount_register_ok(&server, "user-uuid", "Alice", "alice@example.test");
    server.mock(|when, then| {
        when.method(POST).path("/activate");
        then.status(404)
            .header("content-type", "application/json")
            .body(r#"{"code":404,"label":"invalid-code","message":"Invalid activation code"}"#);
    });
    let pending = run_register(&server, "Alice", "alice@example.test", "hunter2hunter2").await;
    let err = pending
        .clone()
        .activate("wrong-code")
        .await
        .expect_err("wrong code must propagate");
    assert!(matches!(err, KupferdrahtError::BackendRejected(_)));
    // The Arc clone the helper held survived — so we can still read
    // the accessors. This verifies the "Arc-clone retry" semantics
    // promised in the plan-doc.
    assert_eq!(pending.user_id(), "user-uuid");
    assert_eq!(pending.email(), "alice@example.test");
}

// ----------------------------------------------------------------
// resend_activation_email
// ----------------------------------------------------------------

#[tokio::test]
async fn pending_resend_activation_email_posts_to_activate_send() {
    let server = MockServer::start();
    mount_register_ok(&server, "user-uuid", "Alice", "alice@example.test");
    let resend_mock = server.mock(|when, then| {
        when.method(POST).path("/activate/send").is_true(|req| {
            let bytes = req.body_ref();
            let Ok(v): Result<serde_json::Value, _> = serde_json::from_slice(bytes) else {
                return false;
            };
            v.get("email").and_then(|x| x.as_str()) == Some("alice@example.test")
        });
        then.status(200);
    });
    let pending = run_register(&server, "Alice", "alice@example.test", "hunter2hunter2").await;
    pending
        .resend_activation_email()
        .await
        .expect("resend must succeed");
    resend_mock.assert_calls(1);
}
