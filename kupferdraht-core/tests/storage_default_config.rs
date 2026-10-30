// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Sub-step 1b: the SDK opens the default store from a base dir via
//! `ClientBuilder::base_dir`, so the consumer never constructs the
//! concrete `SqliteStore` (rusqlite stays internal). `Client::store()`
//! then exposes the SDK-opened store.

use std::sync::Arc;

use httpmock::MockServer;
use kupferdraht::ClientBuilder;
use kupferdraht::keystore::{InMemoryKeyStore, SecureKeyStore};
use tempfile::TempDir;

mod common;

#[tokio::test]
async fn sdk_opens_default_store_from_base_dir() {
    let server = MockServer::start();
    common::mount_full_login_flow(&server);

    let home = TempDir::new().expect("tempdir");
    let keystore: Arc<dyn SecureKeyStore> = Arc::new(InMemoryKeyStore::new());

    let client = ClientBuilder::new()
        .server(common::server_config(&server.base_url()))
        .permit_http()
        // No `SqliteStore::open`, no `.store()`: the SDK opens the
        // per-account `base_dir/accounts/<id>/kupferdraht.db` itself.
        .base_dir(home.path().to_path_buf())
        .keystore(keystore)
        .login("alice@example.test".into(), "pw".into(), None)
        .await
        .expect("login")
        .register_device()
        .await
        .expect("register device");

    let account_id = client.account().user_id().as_str().to_string();
    assert!(
        home.path()
            .join("accounts")
            .join(&account_id)
            .join("kupferdraht.db")
            .exists(),
        "the SDK must open base_dir/accounts/<id>/kupferdraht.db from base_dir",
    );
    assert!(
        client.store().is_some(),
        "Client::store() must expose the SDK-opened store",
    );
    let _ = Box::leak(Box::new(home));
}
