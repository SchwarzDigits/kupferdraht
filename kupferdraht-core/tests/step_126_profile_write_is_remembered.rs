// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 126 — a profile write is remembered locally, not only sent.
//!
//! `set_handle` / `set_display_name` send the change and keep nothing, and
//! nothing else refreshes the account: `Account` is immutable, built at login
//! from `GET /self` and at `resume` from the persisted `self_user` row, which
//! only the post-login persistence writes. So a changed handle is invisible
//! until the next login. These pin that a write the backend accepted lands
//! in the row `resume` reads.
//!
//! The row is written whole, so each test also asserts the fields it did
//! **not** change — a careless helper would clobber them.

use std::sync::Arc;

use httpmock::Method::PUT;
use httpmock::MockServer;
use kupferdraht::Kupferdraht;
use kupferdraht::keystore::{InMemoryKeyStore, SecureKeyStore};
use tempfile::TempDir;

mod common;

const QUALIFIED: &str = "user-uuid@example.test";

/// Log in against the mocked backend, which reports `handle: "alice"` and
/// `name: "Alice Example"` for the self user.
async fn logged_in(server: &MockServer, home: &TempDir) -> Arc<kupferdraht::Client> {
    common::mount_full_login_flow(server);
    let keystore: Arc<dyn SecureKeyStore> = Arc::new(InMemoryKeyStore::new());
    Kupferdraht::open(home.path().to_path_buf(), keystore)
        .expect("open kupferdraht")
        .permit_http()
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
        .expect("register device")
}

#[tokio::test]
async fn set_handle_is_remembered_in_the_self_user_row() {
    let home = TempDir::new().expect("tempdir");
    let server = MockServer::start();
    let client = logged_in(&server, &home).await;
    server.mock(|when, then| {
        when.method(PUT).path("/v14/self/handle");
        then.status(200);
    });

    client.set_handle("alice2").await.expect("set_handle");

    let store = client.store().expect("login attached a store");
    let row = store
        .load_self_user(QUALIFIED)
        .await
        .expect("load_self_user")
        .expect("self_user row present");
    assert_eq!(
        row.handle.as_deref(),
        Some("alice2"),
        "the handle the backend accepted is the handle `resume` will rebuild from"
    );
    assert_eq!(
        row.name, "Alice Example",
        "the name is not this call's business"
    );
    assert_eq!(
        row.email.as_deref(),
        Some("alice@example.test"),
        "nor is the email"
    );
}

#[tokio::test]
async fn set_display_name_is_remembered_in_the_self_user_row() {
    let home = TempDir::new().expect("tempdir");
    let server = MockServer::start();
    let client = logged_in(&server, &home).await;
    server.mock(|when, then| {
        when.method(PUT).path("/v14/self");
        then.status(200);
    });

    client
        .set_display_name("Alice Two")
        .await
        .expect("set_display_name");

    let store = client.store().expect("login attached a store");
    let row = store
        .load_self_user(QUALIFIED)
        .await
        .expect("load_self_user")
        .expect("self_user row present");
    assert_eq!(row.name, "Alice Two");
    assert_eq!(
        row.handle.as_deref(),
        Some("alice"),
        "the handle is not this call's business"
    );
}

/// Two writes in one session: the second must not put back what the first
/// changed. The live account snapshot keeps the login's values, so a row
/// rebuilt from it alone would revert the first write.
#[tokio::test]
async fn two_profile_writes_in_one_session_both_survive() {
    let home = TempDir::new().expect("tempdir");
    let server = MockServer::start();
    let client = logged_in(&server, &home).await;
    server.mock(|when, then| {
        when.method(PUT).path("/v14/self/handle");
        then.status(200);
    });
    server.mock(|when, then| {
        when.method(PUT).path("/v14/self");
        then.status(200);
    });

    client.set_handle("alice2").await.expect("set_handle");
    client
        .set_display_name("Alice Two")
        .await
        .expect("set_display_name");

    let store = client.store().expect("login attached a store");
    let row = store
        .load_self_user(QUALIFIED)
        .await
        .expect("load_self_user")
        .expect("self_user row present");
    assert_eq!(
        row.handle.as_deref(),
        Some("alice2"),
        "the first write stays"
    );
    assert_eq!(row.name, "Alice Two");
}
