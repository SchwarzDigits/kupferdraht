// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! `ClientBuilder::login` returns a ready, usable `Client`.
//!
//! The per-account store is opened (ready, self-keyed) by the
//! `StorageProvider` during login — there is no `LockedClient` / unlock
//! step (removed in 1h; the passphrase mode, if it ever lands, is a
//! feature of a passphrase-backed keystore, not a client state).
//!
//! These integration tests pin: login yields a usable `Client`; the
//! per-account database key lands in the keystore (no on-disk envelope);
//! a re-login reuses the same key; and `mls.db` is encrypted at rest.

use std::sync::Arc;

use httpmock::MockServer;
use kupferdraht::keystore::{InMemoryKeyStore, SecureKeyStore};
use kupferdraht::{Client, ClientBuilder};
use tempfile::TempDir;

mod common;

#[tokio::test]
async fn login_returns_a_usable_client() {
    // Type-level check: `login` resolves to `Arc<Client>` — usable
    // straight away, no unlock step.
    let f = login_fixture().await;
    let client: Arc<Client> = f.client.clone();
    assert!(client.user_id().is_some());
    assert!(client.device_id().is_some());
}

#[tokio::test]
async fn login_stores_account_key_in_keystore_no_dek_bin() {
    // The per-account database key lands in the keystore under
    // `kupferdraht.<home-id>.account-<id>` during login (home-scoped since
    // Step 93) — stored directly, no wrapped blob on disk.
    let f = login_fixture().await;
    let home_id = std::fs::read_to_string(f.home.path().join("home.id"))
        .expect("home.id")
        .trim()
        .to_owned();
    let key_id = format!("kupferdraht.{home_id}.account-{}", f.account_id());
    let key = f.keystore.get_secret(key_id).expect("get_secret");
    assert!(
        key.is_some(),
        "login must store the account key in the keystore"
    );
    assert_eq!(
        key.unwrap().len(),
        32,
        "stored account key material is the raw 32-byte key (ADR-0021)"
    );

    let dek_path = f
        .home
        .path()
        .join("accounts")
        .join(f.account_id())
        .join("dek.bin");
    assert!(
        !dek_path.exists(),
        "no dek.bin envelope — the key lives in the keystore"
    );
}

#[tokio::test]
async fn relogin_reuses_the_same_per_account_key() {
    // A fresh `ClientBuilder` against the same base_dir + keystore must
    // reopen the on-disk DB cleanly with the same per-account key, not
    // re-key or fail.
    let f = login_fixture().await;
    drop(f.client.clone());
    let client2 = relogin(&f).await;
    assert!(client2.user_id().is_some());
}

/// Engine-swap sub-step 2a (mls.db): the openmls state database must be
/// encrypted whole-file at rest, under its **own** key (`account-<id>-mls`),
/// separate from the `kupferdraht.db` key (ADR-0013).
#[tokio::test]
async fn login_encrypts_mls_db_at_rest() {
    let f = login_fixture().await;
    let mls_path = f
        .home
        .path()
        .join("accounts")
        .join(f.account_id())
        .join("mls.db");
    assert!(mls_path.is_file(), "login must create mls.db");
    let bytes = std::fs::read(&mls_path).unwrap_or_default();
    assert!(
        bytes.starts_with(b"SQLite format 3\0"),
        "mls.db header must be plaintext for iOS WAL identification (ADR-0013)"
    );
    assert!(
        !bytes.windows(12).any(|w| w == b"CREATE TABLE"),
        "mls.db content must be encrypted at rest — found a plaintext schema"
    );
}

// ---- fixture --------------------------------------------------------

struct LoginFixture {
    home: TempDir,
    keystore: Arc<dyn SecureKeyStore>,
    server: MockServer,
    client: Arc<Client>,
}

impl LoginFixture {
    fn account_id(&self) -> &str {
        // The account-id is the qualified user-id `<uuid>@<domain>`; the
        // per-account database key is stored in the keystore under
        // `account-<id>`. The Wire mock returns the canonical fixture user.
        FIXTURE_QUALIFIED_USER_ID
    }
}

async fn login_fixture() -> LoginFixture {
    let home = TempDir::new().unwrap();
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    let keystore: Arc<dyn SecureKeyStore> = Arc::new(InMemoryKeyStore::new());
    let client = ClientBuilder::new()
        .server(common::server_config(&server.base_url()))
        .permit_http()
        .base_dir(home.path().to_path_buf())
        .keystore(keystore.clone())
        .login("alice@example.test".into(), "pw".into(), None)
        .await
        .expect("login")
        .register_device()
        .await
        .expect("register device");
    LoginFixture {
        home,
        keystore,
        server,
        client,
    }
}

async fn relogin(f: &LoginFixture) -> Arc<Client> {
    common::mount_full_login_flow(&f.server);
    ClientBuilder::new()
        .server(common::server_config(&f.server.base_url()))
        .permit_http()
        .base_dir(f.home.path().to_path_buf())
        .keystore(f.keystore.clone())
        .login("alice@example.test".into(), "pw".into(), None)
        .await
        .expect("re-login")
        .register_device()
        .await
        .expect("register device")
}

const FIXTURE_QUALIFIED_USER_ID: &str = "user-uuid@example.test";
