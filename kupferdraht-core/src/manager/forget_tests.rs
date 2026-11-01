// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 78 — signing out and forgetting an account natively. TDD red.
//!
//! A native home: random database keys, no seed; the Wire backend is the
//! login mocks plus the session revocation. `logout` without a purge marks
//! the account signed out — the active account is cleared, the client
//! dropped from the manager's cache — and keeps everything else;
//! `forget_account` removes every local trace of an account without a
//! session. The browser's purge options themselves are `purge_tests`.

use std::path::PathBuf;
use std::sync::Arc;

use httpmock::Method::{GET, POST};
use httpmock::{Mock, MockServer};
use tempfile::TempDir;

use super::login_mocks::{mount_device_registration, mount_login};
use super::{Kupferdraht, PurgeOptions};
use crate::client::{Client, ServerConfig};
use crate::error::KupferdrahtError;
use crate::keystore::{InMemoryKeyStore, SecureKeyStore, key_id};
use crate::storage::AccountEntry;

/// The account the login mocks authenticate (`/self`).
const ACCOUNT: &str = "user-uuid@example.test";
/// A second account that exists in the registry, the keystore and on disk
/// only — never logged in here.
const OTHER: &str = "other-uuid@example.test";

const PURGE_LOCAL: PurgeOptions = PurgeOptions {
    purge_device: false,
    purge_local: true,
};

/// One native home with its keystore and backend.
struct Home {
    dir: TempDir,
    keystore: Arc<dyn SecureKeyStore>,
    kupferdraht: Kupferdraht,
    wire: MockServer,
}

impl Home {
    fn new() -> Self {
        let dir = TempDir::new().expect("tempdir");
        let keystore: Arc<dyn SecureKeyStore> = Arc::new(InMemoryKeyStore::new());
        let kupferdraht = Kupferdraht::builder()
            .base_dir(dir.path().to_path_buf())
            .keystore(keystore.clone())
            .permit_http()
            .build()
            .expect("kupferdraht");
        let wire = MockServer::start();
        mount_login(&wire);
        wire.mock(|when, then| {
            when.method(GET).path("/v14/clients");
            then.status(200)
                .header("content-type", "application/json")
                .body("[]");
        });
        mount_device_registration(&wire);
        Self {
            dir,
            keystore,
            kupferdraht,
            wire,
        }
    }

    /// Log the account in and register its device: the registry row, the
    /// two databases with their keys.
    async fn login(&self) -> Arc<Client> {
        let config = ServerConfig {
            api_base_url: self.wire.base_url(),
            websocket_url: self.wire.base_url(),
            title: "mock".into(),
        };
        self.kupferdraht
            .login(config, "alice@example.test".into(), "pw".into(), None, None)
            .await
            .expect("login")
            .register_device()
            .await
            .expect("device registered")
    }

    /// Resume the account: the client the manager holds (only `resume`
    /// enters the manager's cache — a client from `login` is the caller's).
    async fn resume(&self) -> Arc<Client> {
        self.kupferdraht.resume(ACCOUNT).await.expect("resumed")
    }

    /// `POST /access/logout` — the session revocation.
    fn mount_revocation(&self) -> Mock<'_> {
        self.wire.mock(|when, then| {
            when.method(POST).path("/v14/access/logout");
            then.status(200);
        })
    }

    fn secret(&self, id: String) -> Option<Vec<u8>> {
        self.keystore.get_secret(id).expect("keystore read")
    }

    fn database_key(&self, account: &str) -> Option<Vec<u8>> {
        self.secret(
            key_id::account_db(self.kupferdraht.home_id(), account)
                .namespaced()
                .to_owned(),
        )
    }

    fn mls_key(&self, account: &str) -> Option<Vec<u8>> {
        self.secret(
            key_id::account_mls(self.kupferdraht.home_id(), account)
                .namespaced()
                .to_owned(),
        )
    }

    fn account_dir(&self, account: &str) -> PathBuf {
        self.dir.path().join("accounts").join(account)
    }

    async fn listed(&self, account: &str) -> bool {
        self.kupferdraht
            .accounts()
            .await
            .expect("accounts")
            .iter()
            .any(|a| a.account_id == account)
    }

    async fn active(&self) -> Option<String> {
        self.kupferdraht
            .active_account()
            .await
            .expect("active account")
    }

    /// Whether the manager still hands out a client for the account.
    async fn caches_client(&self) -> bool {
        self.kupferdraht
            .clients
            .lock()
            .await
            .get(ACCOUNT)
            .and_then(std::sync::Weak::upgrade)
            .is_some()
    }

    /// Put `OTHER` beside the logged-in account: a registry row, a
    /// database key, a directory.
    async fn add_other_account(&self) {
        self.kupferdraht
            .global
            .upsert_account(&AccountEntry {
                account_id: OTHER.to_owned(),
                server: self.wire.base_url(),
                handle: None,
                display_name: None,
                last_active_ms: 1,
            })
            .await
            .expect("registry row");
        self.keystore
            .set_secret(
                key_id::account_db(self.kupferdraht.home_id(), OTHER)
                    .namespaced()
                    .to_owned(),
                vec![7; 32],
            )
            .expect("keystore write");
        std::fs::create_dir_all(self.account_dir(OTHER)).expect("directory");
    }
}

/// Whether `client`'s store refuses as a closed one does.
async fn store_is_closed(client: &Client) -> bool {
    let store = client.store().expect("a store");
    matches!(
        store.load_recovery_state().await,
        Err(KupferdrahtError::SessionExpired)
    )
}

/// A plain `logout` signs the account out and keeps its data: the
/// registry no longer names it active, the manager no longer hands the
/// client out, the databases and keys stay for the next login.
#[tokio::test]
async fn logout_marks_the_account_signed_out() {
    let home = Home::new();
    drop(home.login().await);
    let client = home.resume().await;
    let revocation = home.mount_revocation();
    assert_eq!(home.active().await.as_deref(), Some(ACCOUNT));
    assert!(
        home.caches_client().await,
        "precondition: the manager holds it"
    );

    home.kupferdraht
        .logout(&client, PurgeOptions::default(), None)
        .await
        .expect("logged out");

    revocation.assert_calls(1);
    assert_eq!(home.active().await, None, "signed out");
    assert!(home.listed(ACCOUNT).await, "still listed");
    assert!(home.account_dir(ACCOUNT).exists(), "the databases stay");
    assert!(home.database_key(ACCOUNT).is_some(), "the keys stay");
    assert!(!home.caches_client().await, "the manager forgot the client");
    assert!(
        !store_is_closed(&client).await,
        "the client itself is not closed"
    );
}

/// `purge_local` on a native home forgets the account: both database keys,
/// the directory, the registry entry.
#[tokio::test]
async fn logout_with_purge_local_forgets_a_native_account() {
    let home = Home::new();
    let client = home.login().await;
    home.mount_revocation();
    assert!(home.mls_key(ACCOUNT).is_some(), "precondition: an mls key");

    home.kupferdraht
        .logout(&client, PURGE_LOCAL, None)
        .await
        .expect("purged");

    assert!(
        home.database_key(ACCOUNT).is_none(),
        "the database key is gone"
    );
    assert!(home.mls_key(ACCOUNT).is_none(), "the mls key is gone");
    assert!(
        !home.account_dir(ACCOUNT).exists(),
        "the databases are gone"
    );
    assert!(!home.listed(ACCOUNT).await, "no longer listed");
    assert_eq!(home.active().await, None);
    assert!(store_is_closed(&client).await, "the client is closed");
}

/// Forgetting needs no session: the account's client is gone, the
/// registry alone knows the account.
#[tokio::test]
async fn forget_account_needs_no_session() {
    let home = Home::new();
    drop(home.login().await);

    home.kupferdraht
        .forget_account(ACCOUNT)
        .await
        .expect("forgotten");

    assert!(!home.listed(ACCOUNT).await, "no longer listed");
    assert!(
        !home.account_dir(ACCOUNT).exists(),
        "the databases are gone"
    );
    assert!(
        home.database_key(ACCOUNT).is_none(),
        "the database key is gone"
    );
    assert!(home.mls_key(ACCOUNT).is_none(), "the mls key is gone");
    assert_eq!(home.active().await, None);
}

/// A client this manager still holds is closed first, so nothing writes
/// into the deleted databases.
#[tokio::test]
async fn forget_account_closes_a_live_client() {
    let home = Home::new();
    drop(home.login().await);
    let client = home.resume().await;
    assert!(
        home.caches_client().await,
        "precondition: the manager holds it"
    );

    home.kupferdraht
        .forget_account(ACCOUNT)
        .await
        .expect("forgotten");

    assert!(store_is_closed(&client).await, "the store refuses");
    assert!(!home.caches_client().await, "the manager forgot it");
    assert!(
        !home.account_dir(ACCOUNT).exists(),
        "the databases are gone"
    );
}

/// A half-forgotten account — the directory already gone, keys and entry
/// still there — is finished, not refused.
#[tokio::test]
async fn forget_account_finishes_a_half_forgotten_account() {
    let home = Home::new();
    drop(home.login().await);
    std::fs::remove_dir_all(home.account_dir(ACCOUNT)).expect("directory removed by hand");
    assert!(
        home.database_key(ACCOUNT).is_some(),
        "precondition: the keys are still there"
    );

    home.kupferdraht
        .forget_account(ACCOUNT)
        .await
        .expect("the rest goes");

    assert!(
        home.database_key(ACCOUNT).is_none(),
        "the database key is gone"
    );
    assert!(home.mls_key(ACCOUNT).is_none(), "the mls key is gone");
    assert!(!home.listed(ACCOUNT).await, "no longer listed");
}

/// An unknown account, and an account already forgotten, are
/// `InvalidInput` — as `resume` answers them.
#[tokio::test]
async fn forget_account_refuses_an_unknown_account() {
    let home = Home::new();
    drop(home.login().await);

    let unknown = home.kupferdraht.forget_account("nobody@example.test").await;
    assert!(
        matches!(unknown, Err(KupferdrahtError::InvalidInput(_))),
        "{unknown:?}"
    );
    assert!(home.listed(ACCOUNT).await, "nothing else touched");

    home.kupferdraht
        .forget_account(ACCOUNT)
        .await
        .expect("forgotten");
    let again = home.kupferdraht.forget_account(ACCOUNT).await;
    assert!(
        matches!(again, Err(KupferdrahtError::InvalidInput(_))),
        "{again:?}"
    );
}

/// Forgetting one account leaves the others' rows, keys and directories.
#[tokio::test]
async fn forget_account_spares_the_other_accounts() {
    let home = Home::new();
    drop(home.login().await);
    home.add_other_account().await;

    home.kupferdraht
        .forget_account(ACCOUNT)
        .await
        .expect("forgotten");

    assert!(!home.listed(ACCOUNT).await, "the forgotten one is gone");
    assert!(home.listed(OTHER).await, "the other row stays");
    assert!(home.database_key(OTHER).is_some(), "the other key stays");
    assert!(
        home.account_dir(OTHER).exists(),
        "the other directory stays"
    );
}
