// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 1g — storage is consumer-pluggable through a single
//! `StorageProvider`, and the per-account databases share one directory.
//!
//! The provider is the one storage seam: `open_global()` for the
//! device-level account registry and `open_store(account_id)` for the
//! per-account app store — both returning **ready** stores (the concrete
//! store does its own encryption; the trait is CRUD-only, no `unlock`).
//! A cloud/bot backend supplies its own provider; the SDK ships the
//! default SQLite one. These tests pin that the SDK routes through the
//! provider, and that `kupferdraht.db` + `mls.db` land in the same
//! `accounts/<id>/` directory.
//!
//! RED until `StorageProvider`, `Kupferdraht::builder().storage_provider()`,
//! `ClientBuilder::base_dir`, and the ready-store `open_default_store`
//! signature exist.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use httpmock::MockServer;
use kupferdraht::Kupferdraht;
use kupferdraht::KupferdrahtError;
use kupferdraht::keystore::{InMemoryKeyStore, SecureKeyStore};
use kupferdraht::storage::{
    GlobalStore, StorageProvider, Store, open_default_global_store, open_default_store,
};
use tempfile::TempDir;

mod common;

const FIXTURE_ACCOUNT_ID: &str = "user-uuid@example.test";

/// A custom `StorageProvider` that records which seams the SDK opened,
/// then delegates to the shipped default SQLite stores. A real cloud
/// backend would return its own `Store` / `GlobalStore` here; the spy
/// only needs to prove the SDK routes through the provider.
struct SpyProvider {
    base_dir: PathBuf,
    keystore: Arc<dyn SecureKeyStore>,
    opened_stores: Mutex<Vec<String>>,
    opened_global: Mutex<u32>,
}

impl SpyProvider {
    fn new(base_dir: PathBuf, keystore: Arc<dyn SecureKeyStore>) -> Self {
        Self {
            base_dir,
            keystore,
            opened_stores: Mutex::new(Vec::new()),
            opened_global: Mutex::new(0),
        }
    }
}

impl StorageProvider for SpyProvider {
    fn open_global(&self) -> Result<Arc<dyn GlobalStore>, KupferdrahtError> {
        *self.opened_global.lock().expect("global lock") += 1;
        open_default_global_store(&self.base_dir, &self.keystore)
    }

    fn open_store(&self, account_id: &str) -> Result<Arc<dyn Store>, KupferdrahtError> {
        self.opened_stores
            .lock()
            .expect("stores lock")
            .push(account_id.to_string());
        open_default_store(&self.base_dir, &self.keystore, account_id)
    }
}

#[tokio::test]
async fn kupferdraht_routes_account_and_global_through_a_custom_provider() {
    let dir = TempDir::new().expect("tempdir");
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    let keystore: Arc<dyn SecureKeyStore> = Arc::new(InMemoryKeyStore::new());

    let spy = Arc::new(SpyProvider::new(dir.path().to_path_buf(), keystore.clone()));

    let app = Kupferdraht::builder()
        .base_dir(dir.path().to_path_buf())
        .keystore(keystore)
        .storage_provider(spy.clone() as Arc<dyn StorageProvider>)
        .permit_http()
        .build()
        .expect("build kupferdraht with custom provider");

    // Opening the device manager already opens the registry via the provider.
    assert!(
        *spy.opened_global.lock().expect("global lock") >= 1,
        "Kupferdraht must open the global store via the provider"
    );

    let _client = app
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

    // Login must open the per-account store via the same provider.
    let opened = spy.opened_stores.lock().expect("stores lock").clone();
    assert!(
        opened.iter().any(|id| id == FIXTURE_ACCOUNT_ID),
        "login must open the account store via the provider; opened = {opened:?}"
    );
}

#[tokio::test]
async fn kupferdraht_db_and_mls_db_share_the_per_account_directory() {
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

    let account_dir = dir
        .path()
        .join("accounts")
        .join(client.user_id().expect("user_id").as_str());
    assert!(
        account_dir.join("kupferdraht.db").is_file(),
        "kupferdraht.db must live under accounts/<id>/"
    );
    assert!(
        account_dir.join("mls.db").is_file(),
        "mls.db must live under accounts/<id>/ — same per-account dir as kupferdraht.db"
    );
}

#[test]
fn default_store_helper_returns_a_ready_encrypted_store() {
    // The default provider's building block: `open_default_store` opens a
    // READY store (the concrete SQLite store fetches its key from the
    // keystore and keys itself — no trait-level `unlock`).
    let dir = TempDir::new().expect("tempdir");
    let keystore: Arc<dyn SecureKeyStore> = Arc::new(InMemoryKeyStore::new());

    let store = open_default_store(dir.path(), &keystore, FIXTURE_ACCOUNT_ID)
        .expect("open ready default store");

    // The store is usable immediately (no unlock step). The file is on disk.
    let _: &Arc<dyn Store> = &store;
    assert!(
        dir.path()
            .join("accounts")
            .join(FIXTURE_ACCOUNT_ID)
            .join("kupferdraht.db")
            .is_file(),
        "open_default_store must materialize accounts/<id>/kupferdraht.db"
    );
}
