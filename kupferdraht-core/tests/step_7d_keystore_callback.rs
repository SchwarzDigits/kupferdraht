// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 7d — `SecureKeyStore` is implementable from foreign code via
//! a `UniFFI` callback interface.
//!
//! 7a–7c shipped three native `SecureKeyStore` implementations
//! (`KeyringStore`, `InMemoryKeyStore`, `FileKeyStore`). 7d closes
//! the platform gap: mobile (iOS Keychain, Android Keystore) and Web
//! (`SubtleCrypto` + IndexedDB) cannot link the desktop `keyring-rs`
//! crate, so the embedder must instead provide a callback-style
//! implementation through `UniFFI`.
//!
//! These tests pin the contract a foreign-language callback must
//! satisfy:
//!
//! * Argument and return types are `Vec<u8>` / `Option<Vec<u8>>`
//!   (`UniFFI` callback interfaces don't take fixed-size byte arrays).
//! * The keystore stores the 32-byte database key **directly** under
//!   an id — no KEK, no wrapping. Kupferdraht hands the bytes over and
//!   reads them back, nothing more.
//! * State persists across simulated process restarts (the foreign
//!   keystore on a real device survives app kills).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use httpmock::MockServer;
use kupferdraht::keystore::SecureKeyStore;
use kupferdraht::{ClientBuilder, KupferdrahtError};
use tempfile::TempDir;

mod common;

/// Foreign-style mock keystore. Mimics a `UniFFI` callback impl
/// backed by Android Keystore / iOS Keychain / `SubtleCrypto`: the
/// embedder's secure storage holds the secret bytes under an id;
/// Kupferdraht hands them over and reads them back.
///
/// Every method takes/returns `Vec<u8>` / `Option<Vec<u8>>` by value
/// the way a `UniFFI`-marshalled callback would — no `&[u8; 32]`
/// borrow on the call interface.
#[derive(Default)]
struct ForeignStyleKeystore {
    secrets: Mutex<HashMap<String, Vec<u8>>>,
    /// Counters so the test can assert which methods fired.
    get_calls: Mutex<u32>,
    set_calls: Mutex<u32>,
}

impl ForeignStyleKeystore {
    fn new() -> Self {
        Self::default()
    }
}

impl SecureKeyStore for ForeignStyleKeystore {
    fn get_secret(&self, id: String) -> Result<Option<Vec<u8>>, KupferdrahtError> {
        *self.get_calls.lock().expect("get_calls") += 1;
        Ok(self.secrets.lock().expect("secrets").get(&id).cloned())
    }

    fn set_secret(&self, id: String, secret: Vec<u8>) -> Result<(), KupferdrahtError> {
        *self.set_calls.lock().expect("set_calls") += 1;
        self.secrets.lock().expect("secrets").insert(id, secret);
        Ok(())
    }

    fn delete_secret(&self, id: String) -> Result<(), KupferdrahtError> {
        self.secrets.lock().expect("secrets").remove(&id);
        Ok(())
    }
}

#[test]
fn foreign_keystore_set_get_round_trips() {
    let ks = ForeignStyleKeystore::new();
    let key = vec![7u8; 32];
    ks.set_secret("account-alice@wire.test".to_owned(), key.clone())
        .expect("set_secret");
    assert_eq!(
        ks.get_secret("account-alice@wire.test".to_owned())
            .expect("get_secret"),
        Some(key),
        "get_secret must return exactly the stored bytes"
    );
}

#[test]
fn foreign_keystore_persists_across_simulated_process_restart() {
    // The SAME foreign instance handed to two independent callers
    // mirrors an embedder whose secure-storage backend outlives the
    // process: re-opening the app finds the same secret.
    let foreign: Arc<dyn SecureKeyStore> = Arc::new(ForeignStyleKeystore::new());
    let key = vec![3u8; 32];
    foreign
        .set_secret("account-alice@wire.test".to_owned(), key.clone())
        .expect("set_secret");

    let after_restart: Arc<dyn SecureKeyStore> = foreign.clone();
    assert_eq!(
        after_restart
            .get_secret("account-alice@wire.test".to_owned())
            .expect("get_secret"),
        Some(key),
        "secret must survive across a simulated restart"
    );
}

#[test]
fn foreign_keystore_isolates_per_id() {
    let ks = ForeignStyleKeystore::new();
    ks.set_secret("account-alice@wire.test".to_owned(), vec![1u8; 32])
        .expect("alice");
    ks.set_secret("account-bob@wire.test".to_owned(), vec![2u8; 32])
        .expect("bob");
    assert_ne!(
        ks.get_secret("account-alice@wire.test".to_owned()).unwrap(),
        ks.get_secret("account-bob@wire.test".to_owned()).unwrap(),
        "different id ⇒ different secret"
    );
}

#[tokio::test]
async fn login_stores_account_key_in_foreign_keystore_no_dek_bin() {
    let home = TempDir::new().unwrap();
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    let foreign: Arc<dyn SecureKeyStore> = Arc::new(ForeignStyleKeystore::new());

    let _client = ClientBuilder::new()
        .server(common::server_config(&server.base_url()))
        .permit_http()
        .base_dir(home.path().to_path_buf())
        .keystore(foreign.clone())
        .login("alice@example.test".into(), "pw".into(), None)
        .await
        .expect("login")
        .register_device()
        .await
        .expect("register device");

    // The account key was stored through the foreign keystore under
    // `kupferdraht.<home-id>.account-<id>` (home-scoped since Step 93) —
    // confirms the set path actually fired. No wrapped blob on disk.
    let home_id = std::fs::read_to_string(home.path().join("home.id"))
        .expect("home.id")
        .trim()
        .to_owned();
    let key = foreign
        .get_secret(format!(
            "kupferdraht.{home_id}.account-user-uuid@example.test"
        ))
        .expect("get_secret");
    assert!(
        key.is_some(),
        "login must store the account key via the foreign keystore"
    );
    assert_eq!(
        key.unwrap().len(),
        32,
        "stored account key material is the raw 32-byte key (ADR-0021)"
    );

    let dek_bin = home
        .path()
        .join("accounts")
        .join("user-uuid@example.test")
        .join("dek.bin");
    assert!(
        !dek_bin.exists(),
        "no wrapped dek.bin envelope on disk — the key lives in the keystore"
    );
}
