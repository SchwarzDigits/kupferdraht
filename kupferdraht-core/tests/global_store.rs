// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Device-level `GlobalStore` — the account registry + app settings
//! that live outside any single account, so an account picker can list
//! accounts without opening any of them.
//!
//! The default backend is a SQLCipher `global.db` encrypted under a
//! device-level key (a reserved keystore entry, openable at app start
//! without opening an account). These tests pin the registry/settings
//! round-trip plus the at-rest encryption + persistence.

use std::sync::Arc;

use kupferdraht::keystore::{InMemoryKeyStore, SecureKeyStore};
use kupferdraht::storage::{AccountEntry, open_default_global_store};
use tempfile::TempDir;

fn entry(account_id: &str, handle: &str) -> AccountEntry {
    AccountEntry {
        account_id: account_id.into(),
        server: "https://wire.example".into(),
        handle: Some(handle.into()),
        display_name: Some(handle.into()),
        last_active_ms: 1_700_000_000_000,
    }
}

#[tokio::test]
async fn global_store_registers_lists_and_removes_accounts() {
    let dir = TempDir::new().expect("tempdir");
    let keystore: Arc<dyn SecureKeyStore> = Arc::new(InMemoryKeyStore::new());
    let global = open_default_global_store(dir.path(), &keystore).expect("open global store");

    assert!(global.list_accounts().await.expect("list").is_empty());

    global
        .upsert_account(&entry("alice@example.test", "alice"))
        .await
        .expect("upsert alice");
    global
        .upsert_account(&entry("bob@example.test", "bob"))
        .await
        .expect("upsert bob");
    assert_eq!(global.list_accounts().await.expect("list").len(), 2);

    // upsert is idempotent on account_id (update, not duplicate).
    global
        .upsert_account(&entry("alice@example.test", "alice-renamed"))
        .await
        .expect("re-upsert alice");
    let accounts = global.list_accounts().await.expect("list");
    assert_eq!(accounts.len(), 2);
    let alice = accounts
        .iter()
        .find(|a| a.account_id == "alice@example.test")
        .expect("alice present");
    assert_eq!(alice.handle.as_deref(), Some("alice-renamed"));

    global
        .remove_account("bob@example.test")
        .await
        .expect("remove bob");
    assert_eq!(global.list_accounts().await.expect("list").len(), 1);
}

#[tokio::test]
async fn global_store_tracks_active_account_and_settings() {
    let dir = TempDir::new().expect("tempdir");
    let keystore: Arc<dyn SecureKeyStore> = Arc::new(InMemoryKeyStore::new());
    let global = open_default_global_store(dir.path(), &keystore).expect("open global store");

    assert!(global.active_account().await.expect("active").is_none());
    global
        .upsert_account(&entry("alice@example.test", "alice"))
        .await
        .expect("upsert");
    global
        .set_active_account(Some("alice@example.test"))
        .await
        .expect("set active");
    assert_eq!(
        global.active_account().await.expect("active").as_deref(),
        Some("alice@example.test")
    );

    assert!(global.get_setting("theme").await.expect("get").is_none());
    global.set_setting("theme", "dark").await.expect("set");
    assert_eq!(
        global.get_setting("theme").await.expect("get").as_deref(),
        Some("dark")
    );
}

#[tokio::test]
async fn global_db_is_encrypted_at_rest_and_persists_across_reopen() {
    let dir = TempDir::new().expect("tempdir");
    let keystore: Arc<dyn SecureKeyStore> = Arc::new(InMemoryKeyStore::new());

    {
        let global = open_default_global_store(dir.path(), &keystore).expect("open global store");
        global
            .upsert_account(&entry("alice@example.test", "alice"))
            .await
            .expect("upsert");
    }

    let bytes = std::fs::read(dir.path().join("global.db")).unwrap_or_default();
    assert!(
        bytes.starts_with(b"SQLite format 3\0"),
        "global.db header must be plaintext for iOS WAL identification (ADR-0013)"
    );
    assert!(
        !bytes.windows(12).any(|w| w == b"CREATE TABLE"),
        "global.db content must be encrypted at rest — found a plaintext schema"
    );

    // Reopen with the same keystore (the device key is recovered directly
    // from the keystore under `global-db`) and the registry persists.
    let global = open_default_global_store(dir.path(), &keystore).expect("reopen global store");
    assert_eq!(global.list_accounts().await.expect("list").len(), 1);
}
