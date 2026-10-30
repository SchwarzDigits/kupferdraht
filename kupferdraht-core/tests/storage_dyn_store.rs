// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Sub-step 1g: storage is injectable through a `StorageProvider`. The
//! builder's `.storage_provider(...)` accepts a consumer backend, so a
//! cloud/bot consumer can plug in their own stores while the SDK ships
//! the default SQLite one. (Replaces the old `.store(Arc<dyn Store>)`
//! seam, which assumed a single account and a SDK-shaped store.)

use std::sync::{Arc, Mutex};

use httpmock::MockServer;
use kupferdraht::keystore::{InMemoryKeyStore, SecureKeyStore};
use kupferdraht::storage::{
    GlobalStore, StorageProvider, Store, open_default_global_store, open_default_store,
};
use kupferdraht::{ClientBuilder, KupferdrahtError};
use tempfile::TempDir;

mod common;

/// A consumer provider that records how often `open_store` fired, then
/// delegates to the shipped default SQLite stores.
struct CountingProvider {
    base_dir: std::path::PathBuf,
    keystore: Arc<dyn SecureKeyStore>,
    opened: Mutex<u32>,
}

impl StorageProvider for CountingProvider {
    fn open_global(&self) -> Result<Arc<dyn GlobalStore>, KupferdrahtError> {
        open_default_global_store(&self.base_dir, &self.keystore)
    }

    fn open_store(&self, account_id: &str) -> Result<Arc<dyn Store>, KupferdrahtError> {
        *self.opened.lock().expect("opened lock") += 1;
        open_default_store(&self.base_dir, &self.keystore, account_id)
    }
}

#[tokio::test]
async fn client_builder_accepts_a_custom_storage_provider() {
    let server = MockServer::start();
    common::mount_full_login_flow(&server);

    let home = TempDir::new().expect("tempdir");
    let keystore: Arc<dyn SecureKeyStore> = Arc::new(InMemoryKeyStore::new());
    let provider = Arc::new(CountingProvider {
        base_dir: home.path().to_path_buf(),
        keystore: keystore.clone(),
        opened: Mutex::new(0),
    });

    let client = ClientBuilder::new()
        .server(common::server_config(&server.base_url()))
        .permit_http()
        .base_dir(home.path().to_path_buf())
        .storage_provider(provider.clone() as Arc<dyn StorageProvider>)
        .keystore(keystore)
        .login("alice@example.test".into(), "pw".into(), None)
        .await
        .expect("login")
        .register_device()
        .await
        .expect("register device");

    assert!(
        client.user_id().is_some(),
        "a client built over a custom provider must work like one over the default",
    );
    assert!(
        *provider.opened.lock().expect("opened lock") >= 1,
        "login must open the per-account store via the custom provider",
    );
    let _ = Box::leak(Box::new(home));
}
