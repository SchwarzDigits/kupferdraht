// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 110a (ADR-0035) — a login in a home whose account keys come from
//! the account's seed. TDD red.
//!
//! The login authenticates, then waits: without the account's seed it opens
//! and persists nothing, and only a first setup (or, from Step 112, a
//! restore) proceeds. With the seed it is the native path, device reuse
//! included. Driven natively with an in-memory keystore and `httpmock`;
//! the mocks are the integration tests' own (`tests/common/login_mocks.rs`).
//! Step 111c: the login opens nothing even with the seed; `registerDevice`
//! opens the account's storage.

use std::path::PathBuf;
use std::sync::Arc;

use httpmock::Method::{GET, POST};
use httpmock::{Mock, MockServer};
use tempfile::TempDir;

use super::{KeysState, Kupferdraht, PendingLogin, RegisterOptions};
use crate::client::ServerConfig;
use crate::error::KupferdrahtError;
use crate::keystore::{InMemoryKeyStore, SecureKeyStore, key_id};
use crate::storage::account_keys::{AccountKeyPurpose, AccountKeySource, derive_account_key};

use super::login_mocks::{mount_device_registration, mount_login};

/// The account the login mocks authenticate (`/self`).
const ACCOUNT: &str = "user-uuid@example.test";

const FIRST_SETUP: RegisterOptions = RegisterOptions { new_setup: true };

struct Fixture {
    dir: TempDir,
    keystore: Arc<dyn SecureKeyStore>,
    kupferdraht: Kupferdraht,
}

impl Fixture {
    fn new(source: AccountKeySource) -> Self {
        let dir = TempDir::new().expect("tempdir");
        let keystore: Arc<dyn SecureKeyStore> = Arc::new(InMemoryKeyStore::new());
        let kupferdraht = Kupferdraht::builder()
            .base_dir(dir.path().to_path_buf())
            .keystore(keystore.clone())
            .permit_http()
            .account_keys(source)
            .build()
            .expect("kupferdraht");
        Self {
            dir,
            keystore,
            kupferdraht,
        }
    }

    fn seed_home() -> Self {
        Self::new(AccountKeySource::Seed)
    }

    async fn login(&self, server: &MockServer) -> Arc<PendingLogin> {
        let config = ServerConfig {
            api_base_url: server.base_url(),
            websocket_url: server.base_url(),
            title: "mock".into(),
        };
        self.kupferdraht
            .login(config, "alice@example.test".into(), "pw".into(), None, None)
            .await
            .expect("login")
    }

    fn secret(&self, id: &str) -> Option<Vec<u8>> {
        self.keystore
            .get_secret(id.to_owned())
            .expect("keystore read")
    }

    fn seed(&self) -> Option<Vec<u8>> {
        self.secret(&key_id::account_seed(self.kupferdraht.home_id(), ACCOUNT))
    }

    fn forget_seed(&self) {
        self.keystore
            .delete_secret(key_id::account_seed(self.kupferdraht.home_id(), ACCOUNT))
            .expect("delete the seed");
    }

    fn store_key(&self) -> Option<Vec<u8>> {
        self.secret(key_id::account_db(self.kupferdraht.home_id(), ACCOUNT).namespaced())
    }

    fn mls_key(&self) -> Option<Vec<u8>> {
        self.secret(key_id::account_mls(self.kupferdraht.home_id(), ACCOUNT).namespaced())
    }

    /// Drop the account's database keys, keep its seed: opening the
    /// storage derives them again.
    fn forget_database_keys(&self) {
        for id in [
            key_id::account_db(self.kupferdraht.home_id(), ACCOUNT),
            key_id::account_mls(self.kupferdraht.home_id(), ACCOUNT),
        ] {
            self.keystore
                .delete_secret(id.namespaced().to_owned())
                .expect("delete a database key");
        }
    }

    fn account_dir(&self) -> PathBuf {
        self.dir.path().join("accounts").join(ACCOUNT)
    }

    async fn account_ids(&self) -> Vec<String> {
        self.kupferdraht
            .accounts()
            .await
            .expect("accounts")
            .into_iter()
            .map(|a| a.account_id)
            .collect()
    }
}

/// `GET /clients` answering one registered device per model.
fn mount_device_list(server: &MockServer, models: &[&str]) {
    let devices: Vec<String> = models
        .iter()
        .enumerate()
        .map(|(i, model)| {
            format!(
                r#"{{"id":"device{i}","type":"permanent","class":"desktop","model":"{model}"}}"#
            )
        })
        .collect();
    let body = format!("[{}]", devices.join(","));
    server.mock(|when, then| {
        when.method(GET).path("/v14/clients");
        then.status(200)
            .header("content-type", "application/json")
            .body(body);
    });
}

/// `POST /clients` refused: the account's device cap is reached.
fn mount_device_cap(server: &MockServer) -> Mock<'_> {
    server.mock(|when, then| {
        when.method(POST).path("/v14/clients");
        then.status(400)
            .header("content-type", "application/json")
            .body(r#"{"code":400,"label":"too-many-clients","message":"too many clients"}"#);
    })
}

/// `GET /clients/new-client-id`: whether the backend still has the device
/// the first setup registered.
fn mount_recorded_device(server: &MockServer, registered: bool) {
    server.mock(|when, then| {
        when.method(GET).path("/v14/clients/new-client-id");
        if registered {
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"new-client-id","type":"permanent"}"#);
        } else {
            then.status(404)
                .header("content-type", "application/json")
                .body(r#"{"code":404,"label":"client-not-found","message":"client not found"}"#);
        }
    });
}

/// A seed home with the account set up once: login, first setup, the
/// client dropped again. Returns the registration mock.
async fn set_up<'s>(fx: &Fixture, server: &'s MockServer) -> Mock<'s> {
    mount_login(server);
    mount_device_list(server, &[]);
    let register = mount_device_registration(server);
    let pending = fx.login(server).await;
    let client = pending
        .register_device_with(FIRST_SETUP)
        .await
        .expect("first setup");
    drop(client);
    register
}

#[tokio::test]
async fn a_login_without_the_accounts_seed_opens_and_persists_nothing() {
    let server = MockServer::start();
    mount_login(&server);
    mount_device_list(&server, &[]);
    let fx = Fixture::seed_home();

    let pending = fx.login(&server).await;

    assert_eq!(pending.keys(), KeysState::Missing);
    assert!(!fx.account_dir().exists(), "no database of the account");
    assert_eq!(fx.seed(), None);
    assert_eq!(fx.store_key(), None, "no key of the account");
    assert_eq!(fx.mls_key(), None);
    assert!(fx.account_ids().await.is_empty(), "no registry entry");
}

#[tokio::test]
async fn register_device_without_the_seed_is_refused_and_the_login_stays_usable() {
    let server = MockServer::start();
    mount_login(&server);
    mount_device_list(&server, &[]);
    mount_device_registration(&server);
    let fx = Fixture::seed_home();
    let pending = fx.login(&server).await;

    let refused = pending.register_device().await;
    assert!(
        matches!(refused, Err(KupferdrahtError::RecoveryDataRequired(_))),
        "{refused:?}"
    );
    assert!(!fx.account_dir().exists(), "the refusal created nothing");

    pending
        .register_device_with(FIRST_SETUP)
        .await
        .expect("a first setup on the same pending login");
}

#[tokio::test]
async fn a_first_setup_creates_the_seed_derives_the_keys_and_registers() {
    let server = MockServer::start();
    let fx = Fixture::seed_home();

    set_up(&fx, &server).await;

    let seed: [u8; 32] = fx
        .seed()
        .expect("the seed is stored")
        .try_into()
        .expect("32 bytes");
    assert_eq!(
        fx.store_key().as_deref(),
        Some(
            derive_account_key(&seed, AccountKeyPurpose::Store)
                .key()
                .as_slice()
        ),
        "the kupferdraht.db key comes from the seed"
    );
    assert_eq!(
        fx.mls_key().as_deref(),
        Some(
            derive_account_key(&seed, AccountKeyPurpose::Mls)
                .key()
                .as_slice()
        ),
        "the mls.db key comes from the seed"
    );
    assert_eq!(fx.account_ids().await, vec![ACCOUNT.to_owned()]);
}

/// Keys and databases an earlier build left without a seed cannot be
/// backed by recovery data: a first setup removes them.
#[tokio::test]
async fn a_first_setup_removes_keys_and_databases_left_without_a_seed() {
    let server = MockServer::start();
    let fx = Fixture::seed_home();
    let home_id = fx.kupferdraht.home_id().to_owned();
    fx.keystore
        .set_secret(
            key_id::account_db(&home_id, ACCOUNT)
                .namespaced()
                .to_owned(),
            vec![1u8; 32],
        )
        .expect("legacy key");
    std::fs::create_dir_all(fx.account_dir()).expect("mkdir");
    std::fs::write(
        fx.account_dir().join("kupferdraht.db"),
        b"a legacy database",
    )
    .expect("write");

    set_up(&fx, &server).await;

    let seed: [u8; 32] = fx.seed().expect("seed").try_into().expect("32 bytes");
    assert_eq!(
        fx.store_key().as_deref(),
        Some(
            derive_account_key(&seed, AccountKeyPurpose::Store)
                .key()
                .as_slice()
        ),
        "the legacy key was replaced by the derived one"
    );
}

#[tokio::test]
async fn a_later_login_with_the_seed_takes_the_native_path_and_reuses_the_device() {
    let server = MockServer::start();
    let fx = Fixture::seed_home();
    let register = set_up(&fx, &server).await;
    mount_recorded_device(&server, true);

    let pending = fx.login(&server).await;
    assert_eq!(pending.keys(), KeysState::Present);
    let client = pending.register_device().await.expect("native path");

    assert_eq!(register.calls(), 1, "the recorded device was reused");
    assert!(!client.device_replaced());
}

/// Step 111c (ADR-0035): a login opens no database of the account —
/// `registerDevice` does — so taking over databases another browser holds
/// never needs a second login. Observable through the keys the seed
/// derives: only an open derives them.
#[tokio::test]
async fn a_login_with_the_seed_opens_nothing_until_register_device() {
    let server = MockServer::start();
    let fx = Fixture::seed_home();
    let register = set_up(&fx, &server).await;
    mount_recorded_device(&server, true);
    fx.forget_database_keys();

    let pending = fx.login(&server).await;

    assert_eq!(pending.keys(), KeysState::Present, "the seed is here");
    assert!(
        fx.store_key().is_none() && fx.mls_key().is_none(),
        "the login opened no database of the account"
    );
    let client = pending.register_device().await.expect("registered");
    assert!(
        fx.store_key().is_some() && fx.mls_key().is_some(),
        "registerDevice opened the account's storage"
    );
    assert_eq!(register.calls(), 1, "the recorded device was reused");
    assert!(!client.device_replaced());
}

#[tokio::test]
async fn a_device_that_replaced_the_recorded_one_is_reported() {
    let server = MockServer::start();
    let fx = Fixture::seed_home();
    let register = set_up(&fx, &server).await;
    mount_recorded_device(&server, false);

    let client = fx
        .login(&server)
        .await
        .register_device()
        .await
        .expect("native path");

    assert_eq!(register.calls(), 2, "a new device was registered");
    assert!(client.device_replaced());
}

#[tokio::test]
async fn a_retry_after_too_many_devices_keeps_the_seed() {
    let server = MockServer::start();
    mount_login(&server);
    mount_device_list(&server, &[]);
    let mut cap = mount_device_cap(&server);
    let fx = Fixture::seed_home();
    let pending = fx.login(&server).await;

    let refused = pending.register_device_with(FIRST_SETUP).await;
    assert!(
        matches!(refused, Err(KupferdrahtError::TooManyDevices)),
        "{refused:?}"
    );
    let seed = fx.seed().expect("the first setup stored the seed");

    cap.delete();
    mount_device_registration(&server);
    pending
        .register_device_with(FIRST_SETUP)
        .await
        .expect("the same call again");
    assert_eq!(fx.seed(), Some(seed), "the seed was kept, not made again");
}

#[tokio::test]
async fn cancel_after_a_failed_first_setup_leaves_nothing() {
    let server = MockServer::start();
    mount_login(&server);
    mount_device_list(&server, &[]);
    let mut cap = mount_device_cap(&server);
    let fx = Fixture::seed_home();
    let pending = fx.login(&server).await;
    let refused = pending.register_device_with(FIRST_SETUP).await;
    assert!(
        matches!(refused, Err(KupferdrahtError::TooManyDevices)),
        "{refused:?}"
    );
    assert!(
        fx.account_dir().join("kupferdraht.db").exists(),
        "the first setup got as far as the database"
    );

    pending.cancel().expect("cancel");

    assert_eq!(fx.seed(), None);
    assert_eq!(fx.store_key(), None);
    assert_eq!(fx.mls_key(), None);
    assert!(!fx.account_dir().join("kupferdraht.db").exists());
    assert!(!fx.account_dir().join("mls.db").exists());
    assert!(fx.account_ids().await.is_empty());

    cap.delete();
    mount_device_registration(&server);
    fx.login(&server)
        .await
        .register_device_with(FIRST_SETUP)
        .await
        .expect("a new first setup works");
}

#[tokio::test]
async fn accounts_lists_only_accounts_with_a_seed() {
    let server = MockServer::start();
    let fx = Fixture::seed_home();
    set_up(&fx, &server).await;
    assert_eq!(fx.account_ids().await, vec![ACCOUNT.to_owned()]);

    fx.forget_seed();

    assert!(fx.account_ids().await.is_empty());
}

#[tokio::test]
async fn resume_without_the_seed_is_refused() {
    let server = MockServer::start();
    let fx = Fixture::seed_home();
    set_up(&fx, &server).await;
    fx.forget_seed();

    let resumed = fx.kupferdraht.resume(ACCOUNT).await;

    assert!(
        matches!(resumed, Err(KupferdrahtError::RecoveryDataRequired(_))),
        "{:?}",
        resumed.err()
    );
}

/// Step 120 (ADR-0040): `recoveryExpected` comes from the storage slot —
/// without a PIN service (the local mode) there is no recovery data, and
/// the device list no longer decides.
#[tokio::test]
async fn without_a_pin_service_no_recovery_is_expected() {
    let server = MockServer::start();
    mount_login(&server);
    mount_device_list(&server, &["Kupferdraht (Web)"]);
    let fx = Fixture::seed_home();

    let pending = fx.login(&server).await;

    assert!(!pending.recovery_expected());
}

/// Native — a `Random` home — is unchanged: the login assembles at once.
#[tokio::test]
async fn a_random_home_opens_the_account_at_login_as_before() {
    let server = MockServer::start();
    mount_login(&server);
    let fx = Fixture::new(AccountKeySource::Random);

    let pending = fx.login(&server).await;

    assert_eq!(pending.keys(), KeysState::Present);
    assert!(
        fx.account_dir().join("kupferdraht.db").exists(),
        "the store opens at login"
    );
    assert!(!pending.recovery_expected());
}

/// Step 121 (ADR-0040): an open the account's storage slot refused means this
/// browser holds a replaced device's keys. Forgetting them is the whole fix —
/// ADR-0040 does it on `resume` and promises a replaced browser "resets
/// itself", and a login is the other way in.
#[tokio::test]
async fn forgetting_a_retired_device_leaves_no_keys_of_the_account() {
    let server = MockServer::start();
    let fx = Fixture::seed_home();
    let _register = set_up(&fx, &server).await;
    assert!(fx.seed().is_some(), "the setup left a seed to forget");
    let pending = fx.login(&server).await;
    assert_eq!(pending.keys(), KeysState::Present);

    pending
        .forget_retired_device()
        .await
        .expect("forget the keys");

    assert_eq!(fx.seed(), None, "the seed is gone");
    assert_eq!(fx.store_key(), None, "and the keys derived from it");
    assert_eq!(fx.mls_key(), None);
    assert!(
        fx.account_ids().await.is_empty(),
        "accounts() lists only accounts with a seed, so the entry goes with it"
    );
}

/// And the next login is what the user actually needs: the keyfile path,
/// not the same failure again. Without the forgetting above, `keys` stays
/// `Present` and every attempt ends in `DeviceRetired`.
#[tokio::test]
async fn after_forgetting_a_retired_device_the_next_login_asks_for_the_keyfile() {
    let server = MockServer::start();
    let fx = Fixture::seed_home();
    let _register = set_up(&fx, &server).await;
    fx.login(&server)
        .await
        .forget_retired_device()
        .await
        .expect("forget the keys");

    let next = fx.login(&server).await;

    assert_eq!(next.keys(), KeysState::Missing, "recovery, or a new setup");
}
