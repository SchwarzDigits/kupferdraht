// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 16d — `ClientBuilder::activate` integration tests.
//!
//! The CLI's `kupferdraht-cli activate` subcommand runs in a fresh process
//! that cannot share `PendingClient` state with the prior `register`
//! invocation. `ClientBuilder::activate(email, code, password)` is the
//! process-symmetric entry point: it takes the same inputs explicitly
//! and threads them through `POST /activate` + the existing login
//! pipeline, returning a ready `Arc<Client>`.

use std::sync::Arc;

use httpmock::Method::POST;
use httpmock::MockServer;
use kupferdraht::ClientBuilder;
use kupferdraht::keystore::{InMemoryKeyStore, SecureKeyStore};
use tempfile::TempDir;

mod common;

fn fresh_builder(server: &MockServer) -> ClientBuilder {
    let home = TempDir::new().expect("TempDir");
    let keystore: Arc<dyn SecureKeyStore> = Arc::new(InMemoryKeyStore::new());
    let base = home.path().to_path_buf();
    let _ = Box::leak(Box::new(home));
    ClientBuilder::new()
        .server(common::server_config(&server.base_url()))
        .permit_http()
        .base_dir(base)
        .keystore(keystore)
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

#[tokio::test]
async fn client_builder_activate_runs_activate_then_login_returns_ready_client() {
    let server = MockServer::start();
    mount_activate_ok(&server, "alice@example.test");
    common::mount_full_login_flow(&server);
    let client = fresh_builder(&server)
        .activate(
            "alice@example.test".into(),
            "good-code".into(),
            "hunter2hunter2".into(),
        )
        .await
        .expect("activate must succeed");
    assert_eq!(
        client.user_id().map(|u| u.as_str()),
        Some("user-uuid@example.test")
    );
}

#[tokio::test]
async fn client_builder_activate_404_invalid_code_surfaces_as_backend_rejected() {
    use kupferdraht::KupferdrahtError;
    let server = MockServer::start();
    // /activate returns 404 invalid-code — the login pipeline must
    // not run; the error must propagate untouched.
    server.mock(|when, then| {
        when.method(POST).path("/activate");
        then.status(404)
            .header("content-type", "application/json")
            .body(r#"{"code":404,"label":"invalid-code","message":"Invalid activation code"}"#);
    });
    let err = fresh_builder(&server)
        .activate(
            "alice@example.test".into(),
            "wrong-code".into(),
            "hunter2hunter2".into(),
        )
        .await
        .expect_err("wrong code must propagate");
    let KupferdrahtError::BackendRejected(msg) = err else {
        panic!("expected BackendRejected, got {err:?}");
    };
    assert!(
        msg.contains("invalid-code"),
        "Wire label must be preserved; got {msg:?}"
    );
}
