// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 114 (ADR-0035) — the browser's `logout` with its purge options, in
//! a seed home. TDD red.
//!
//! Driven natively, so with the local mode's semantics: `purgeLocal`
//! deletes the account's databases here, `purgeDevice` deletes the device
//! at the backend and keeps them (in the server mode it deletes them on
//! the storage server — the smoke's part). The Wire backend is the login
//! mocks plus the device deletion and the session revocation.

use std::path::PathBuf;
use std::sync::Arc;

use httpmock::Method::{DELETE, GET, POST};
use httpmock::{Mock, MockServer};
use tempfile::TempDir;

use super::login_mocks::{mount_device_registration, mount_login};
use super::{Kupferdraht, PurgeOptions, RegisterOptions};
use crate::client::{Client, ServerConfig};
use crate::error::KupferdrahtError;
use crate::keystore::{InMemoryKeyStore, SecureKeyStore, key_id};
use crate::storage::account_keys::AccountKeySource;

/// The account the login mocks authenticate (`/self`).
const ACCOUNT: &str = "user-uuid@example.test";
const PASSWORD: &str = "pw";

const FIRST_SETUP: RegisterOptions = RegisterOptions { new_setup: true };
const PURGE_LOCAL: PurgeOptions = PurgeOptions {
    purge_device: false,
    purge_local: true,
};
const PURGE_DEVICE: PurgeOptions = PurgeOptions {
    purge_device: true,
    purge_local: false,
};
const PURGE_BOTH: PurgeOptions = PurgeOptions {
    purge_device: true,
    purge_local: true,
};

/// One browser with the account set up: its home, keystore, backend.
struct Browser {
    dir: TempDir,
    keystore: Arc<dyn SecureKeyStore>,
    kupferdraht: Kupferdraht,
    wire: MockServer,
}

impl Browser {
    fn new() -> Self {
        let dir = TempDir::new().expect("tempdir");
        let keystore: Arc<dyn SecureKeyStore> = Arc::new(InMemoryKeyStore::new());
        let kupferdraht = Kupferdraht::builder()
            .base_dir(dir.path().to_path_buf())
            .keystore(keystore.clone())
            .permit_http()
            .account_keys(AccountKeySource::Seed)
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

    /// Log in and set the account up.
    async fn first_setup(&self) -> Arc<Client> {
        let config = ServerConfig {
            api_base_url: self.wire.base_url(),
            websocket_url: self.wire.base_url(),
            title: "mock".into(),
        };
        self.kupferdraht
            .login(
                config,
                "alice@example.test".into(),
                PASSWORD.into(),
                None,
                None,
            )
            .await
            .expect("login")
            .register_device_with(FIRST_SETUP)
            .await
            .expect("first setup")
    }

    /// `POST /access/logout` — the session revocation.
    fn mount_revocation(&self) -> Mock<'_> {
        self.wire.mock(|when, then| {
            when.method(POST).path("/v14/access/logout");
            then.status(200);
        })
    }

    /// `DELETE /clients/<id>` — answered with `status`.
    fn mount_device_deletion(&self, status: u16) -> Mock<'_> {
        self.wire.mock(|when, then| {
            when.method(DELETE).path("/v14/clients/new-client-id");
            then.status(status)
                .header("content-type", "application/json")
                .body(if status == 200 {
                    "{}"
                } else {
                    r#"{"code":403,"label":"invalid-credentials","message":"wrong password"}"#
                });
        })
    }

    fn secret(&self, id: String) -> Option<Vec<u8>> {
        self.keystore.get_secret(id).expect("keystore read")
    }

    fn seed(&self) -> Option<Vec<u8>> {
        self.secret(key_id::account_seed(self.kupferdraht.home_id(), ACCOUNT))
    }

    fn database_key(&self) -> Option<Vec<u8>> {
        self.secret(
            key_id::account_db(self.kupferdraht.home_id(), ACCOUNT)
                .namespaced()
                .to_owned(),
        )
    }

    fn account_dir(&self) -> PathBuf {
        self.dir.path().join("accounts").join(ACCOUNT)
    }

    async fn listed(&self) -> bool {
        self.kupferdraht
            .accounts()
            .await
            .expect("accounts")
            .iter()
            .any(|a| a.account_id == ACCOUNT)
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
}

/// Whether `client`'s store refuses as a closed one does.
async fn store_is_closed(client: &Client) -> bool {
    let store = client.store().expect("a store");
    matches!(
        store.load_recovery_state().await,
        Err(KupferdrahtError::SessionExpired)
    )
}

/// Without options `logout` is the native one: the session ends, the
/// account stays in this browser.
#[tokio::test]
async fn a_plain_logout_keeps_the_account_here() {
    let browser = Browser::new();
    let client = browser.first_setup().await;
    let revocation = browser.mount_revocation();

    browser
        .kupferdraht
        .logout_client(&client, PurgeOptions::default(), None)
        .await
        .expect("logged out");

    revocation.assert_calls(1);
    assert!(browser.seed().is_some(), "the seed stays");
    assert!(browser.account_dir().exists(), "the databases stay");
    assert!(browser.listed().await, "still listed");
}

#[tokio::test]
async fn purge_local_forgets_the_account_here() {
    let browser = Browser::new();
    let client = browser.first_setup().await;
    let revocation = browser.mount_revocation();

    browser
        .kupferdraht
        .logout_client(&client, PURGE_LOCAL, None)
        .await
        .expect("purged");

    revocation.assert_calls(1);
    assert!(browser.seed().is_none(), "the seed is gone");
    assert!(browser.database_key().is_none(), "the keys are gone");
    assert!(!browser.account_dir().exists(), "the databases are gone");
    assert!(!browser.listed().await, "no longer listed");
    assert_eq!(
        browser
            .kupferdraht
            .global
            .active_account()
            .await
            .expect("active account"),
        None
    );
}

/// After a purge the client is closed: its store refuses, and the manager
/// no longer hands it out.
#[tokio::test]
async fn a_purged_client_is_closed_and_forgotten() {
    let browser = Browser::new();
    let client = browser.first_setup().await;
    browser.mount_revocation();

    browser
        .kupferdraht
        .logout_client(&client, PURGE_LOCAL, None)
        .await
        .expect("purged");

    assert!(store_is_closed(&client).await, "the store refuses");
    assert!(!browser.caches_client().await, "the manager forgot it");
}

#[tokio::test]
async fn purge_device_needs_the_password() {
    let browser = Browser::new();
    let client = browser.first_setup().await;
    let deletion = browser.mount_device_deletion(200);

    let result = browser
        .kupferdraht
        .logout_client(&client, PURGE_BOTH, None)
        .await;

    assert!(
        matches!(result, Err(KupferdrahtError::InvalidInput(_))),
        "{result:?}"
    );
    deletion.assert_calls(0);
    assert!(browser.seed().is_some(), "nothing purged");
    assert!(browser.account_dir().exists(), "nothing deleted");
    assert!(!store_is_closed(&client).await, "the client still works");
}

/// A wrong password leaves everything as it was — the call can be
/// repeated.
#[tokio::test]
async fn a_refused_device_deletion_purges_nothing() {
    let browser = Browser::new();
    let client = browser.first_setup().await;
    let deletion = browser.mount_device_deletion(403);

    let result = browser
        .kupferdraht
        .logout_client(&client, PURGE_BOTH, Some("wrong"))
        .await;

    assert!(
        matches!(result, Err(KupferdrahtError::BackendRejected(_))),
        "{result:?}"
    );
    deletion.assert_calls(1);
    assert!(browser.seed().is_some(), "the seed stays");
    assert!(browser.account_dir().exists(), "the databases stay");
    assert!(browser.listed().await, "still listed");
    assert!(!store_is_closed(&client).await, "the client still works");
}

/// `purgeDevice` alone deletes the device; the local mode keeps the
/// databases here (they are the device's only on the storage server).
#[tokio::test]
async fn purge_device_deletes_the_device_and_closes_the_client() {
    let browser = Browser::new();
    let client = browser.first_setup().await;
    let deletion = browser.mount_device_deletion(200);
    browser.mount_revocation();

    browser
        .kupferdraht
        .logout_client(&client, PURGE_DEVICE, Some(PASSWORD))
        .await
        .expect("purged");

    deletion.assert_calls(1);
    assert!(browser.seed().is_some(), "the seed stays");
    assert!(browser.account_dir().exists(), "the local databases stay");
    assert!(browser.listed().await, "still listed");
    assert!(store_is_closed(&client).await, "the client is closed");
    assert!(!browser.caches_client().await, "the manager forgot it");
}

/// Both together leave a browser for good.
#[tokio::test]
async fn purging_both_leaves_nothing() {
    let browser = Browser::new();
    let client = browser.first_setup().await;
    let deletion = browser.mount_device_deletion(200);
    browser.mount_revocation();

    browser
        .kupferdraht
        .logout_client(&client, PURGE_BOTH, Some(PASSWORD))
        .await
        .expect("purged");

    deletion.assert_calls(1);
    assert!(browser.seed().is_none(), "the seed is gone");
    assert!(browser.database_key().is_none(), "the keys are gone");
    assert!(!browser.account_dir().exists(), "the databases are gone");
    assert!(!browser.listed().await, "no longer listed");
}
