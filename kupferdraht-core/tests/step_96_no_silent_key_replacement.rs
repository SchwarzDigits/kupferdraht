// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 96 — a missing key for an existing database is an error, never a
//! new key (ADR-0029). TDD red.
//!
//! What they pin, for each of the three databases (`global.db`,
//! `kupferdraht.db`, `mls.db`):
//!
//! - the database exists, the keystore has no key → `KeyUnavailable`, and
//!   nothing is written to the keystore (the real key, should it come
//!   back, is not overwritten);
//! - the iOS-before-first-unlock case end to end: a keystore that answers
//!   "not stored" while locked leaves the account intact, and after the
//!   unlock the same account resumes;
//! - a keystore that follows the contract (locked → `Err`) surfaces its
//!   error and writes nothing (a pin: this already holds).

use std::fs;
use std::path::Path;
use std::sync::Arc;

use httpmock::MockServer;
use kupferdraht::keystore::{InMemoryKeyStore, SecureKeyStore};
use kupferdraht::storage::open_default_store;
use kupferdraht::{Kupferdraht, KupferdrahtError};
use tempfile::TempDir;

mod common;

use common::keystore::{GatedKeyStore, LockableKeyStore, LockedAnswer};

/// The account the shared login fixture logs in as.
const ACCOUNT: &str = "user-uuid@example.test";

fn home_id(home: &Path) -> String {
    fs::read_to_string(home.join("home.id"))
        .expect("home.id is readable")
        .trim()
        .to_owned()
}

fn mls_key_id(home: &Path) -> String {
    format!("kupferdraht.{}.account-{ACCOUNT}-mls", home_id(home))
}

fn account_key_id(home: &Path) -> String {
    format!("kupferdraht.{}.account-{ACCOUNT}", home_id(home))
}

fn global_key_id(home: &Path) -> String {
    format!("kupferdraht.{}.global-db", home_id(home))
}

/// A keystore that counts writes and never blocks.
fn counting_keystore() -> Arc<GatedKeyStore<InMemoryKeyStore>> {
    Arc::new(GatedKeyStore::new(InMemoryKeyStore::new(), Box::new(|| {})))
}

/// Pins the variant and hands back its payload (the database, the
/// keystore id and the way out) for the tests that check the text.
fn assert_key_unavailable<T>(result: Result<T, KupferdrahtError>, what: &str) -> String {
    match result {
        Err(KupferdrahtError::KeyUnavailable(payload)) => payload,
        Err(other) => panic!("{what}: expected KeyUnavailable, got {other:?}"),
        Ok(_) => panic!("{what}: expected KeyUnavailable, got Ok"),
    }
}

/// Log in through the manager on `home` with `keystore`: all three
/// databases and their keys exist afterwards.
async fn log_in(home: &Path, keystore: Arc<dyn SecureKeyStore>, server: &MockServer) {
    let app = Kupferdraht::open(home.to_path_buf(), keystore)
        .expect("open the home")
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
    drop(client);
}

async fn resume(home: &Path, keystore: Arc<dyn SecureKeyStore>) -> Result<(), KupferdrahtError> {
    let app = Kupferdraht::open(home.to_path_buf(), keystore)?.permit_http();
    let client = app.resume(ACCOUNT).await?;
    assert_eq!(client.user_id().expect("user id").as_str(), ACCOUNT);
    Ok(())
}

// ── one database at a time ──────────────────────────────────────────────

/// `kupferdraht.db` exists, the keystore has no key for it: used to generate
/// and store a fresh key (which cannot open the database).
#[test]
fn an_account_database_without_its_key_is_key_unavailable_and_nothing_is_written() {
    let home = TempDir::new().expect("home");
    let original: Arc<dyn SecureKeyStore> = Arc::new(InMemoryKeyStore::new());
    drop(open_default_store(home.path(), &original, ACCOUNT).expect("create kupferdraht.db"));

    let empty = counting_keystore();
    let as_dyn: Arc<dyn SecureKeyStore> = empty.clone();
    let payload = assert_key_unavailable(
        open_default_store(home.path(), &as_dyn, ACCOUNT),
        "kupferdraht.db without its key",
    );
    assert!(
        payload.contains("kupferdraht.db") && payload.contains("remove the account directory"),
        "the way out for an account database is its directory: {payload}"
    );
    assert_eq!(empty.sets(), 0, "nothing was written to the keystore");
    assert!(
        empty
            .get_secret(account_key_id(home.path()))
            .expect("read")
            .is_none()
    );
}

/// `global.db` exists, the keystore has no key for it.
#[test]
fn the_account_registry_without_its_key_is_key_unavailable_and_nothing_is_written() {
    let home = TempDir::new().expect("home");
    let original: Arc<dyn SecureKeyStore> = Arc::new(InMemoryKeyStore::new());
    drop(Kupferdraht::open(home.path().to_path_buf(), original).expect("create global.db"));

    let empty = counting_keystore();
    let payload = assert_key_unavailable(
        Kupferdraht::open(home.path().to_path_buf(), empty.clone()),
        "global.db without its key",
    );
    assert!(
        payload.contains("global.db") && !payload.contains("account directory"),
        "the registry is not in an account directory — its way out names global.db: {payload}"
    );
    assert_eq!(empty.sets(), 0, "nothing was written to the keystore");
    assert!(
        empty
            .get_secret(global_key_id(home.path()))
            .expect("read")
            .is_none()
    );
}

/// `mls.db` exists, only its key is missing (registry and account keys
/// are there): the resume fails before a new MLS key is written.
#[tokio::test]
async fn an_mls_database_without_its_key_is_key_unavailable_and_the_key_is_kept() {
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    let home = TempDir::new().expect("home");
    let keystore = Arc::new(LockableKeyStore::new(
        InMemoryKeyStore::new(),
        Box::new(|id| id.ends_with("-mls")),
    ));
    log_in(home.path(), keystore.clone(), &server).await;
    let mls_key = keystore
        .stored(&mls_key_id(home.path()))
        .expect("login stored the mls.db key");
    let sets_after_login = keystore.sets();

    keystore.lock(LockedAnswer::Missing);
    assert_key_unavailable(
        resume(home.path(), keystore.clone()).await,
        "mls.db without its key",
    );

    assert_eq!(keystore.sets(), sets_after_login, "nothing was written");
    assert_eq!(
        keystore.stored(&mls_key_id(home.path())),
        Some(mls_key),
        "the real key is not overwritten"
    );
}

// ── the locked-keystore story ───────────────────────────────────────────

/// iOS before the first unlock after a reboot, with a host keystore that
/// answers "not stored" while locked: the resume fails with
/// `KeyUnavailable`, every key survives, and after the unlock the same
/// account resumes. Before ADR-0029 the locked resume replaced the keys —
/// the account was gone for good.
#[tokio::test]
async fn a_locked_keystore_answering_missing_does_not_destroy_the_account() {
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    let home = TempDir::new().expect("home");
    let keystore = Arc::new(LockableKeyStore::new(
        InMemoryKeyStore::new(),
        Box::new(|_| true),
    ));
    log_in(home.path(), keystore.clone(), &server).await;
    let ids = [
        global_key_id(home.path()),
        account_key_id(home.path()),
        mls_key_id(home.path()),
    ];
    let keys: Vec<Option<Vec<u8>>> = ids.iter().map(|id| keystore.stored(id)).collect();
    assert!(
        keys.iter().all(Option::is_some),
        "login stored all three keys"
    );

    keystore.lock(LockedAnswer::Missing);
    assert_key_unavailable(
        resume(home.path(), keystore.clone()).await,
        "resume while the keystore is locked",
    );
    let after: Vec<Option<Vec<u8>>> = ids.iter().map(|id| keystore.stored(id)).collect();
    assert_eq!(after, keys, "no key was replaced while locked");

    keystore.unlock();
    resume(home.path(), keystore.clone())
        .await
        .expect("after the unlock the same account resumes");
}

/// A keystore that follows the ADR-0029 contract (locked → `Err`): the
/// error surfaces and nothing is written. Already true before Step 96 —
/// pinned so the new check cannot swallow it.
#[tokio::test]
async fn a_locked_keystore_answering_an_error_surfaces_it_and_writes_nothing() {
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    let home = TempDir::new().expect("home");
    let keystore = Arc::new(LockableKeyStore::new(
        InMemoryKeyStore::new(),
        Box::new(|_| true),
    ));
    log_in(home.path(), keystore.clone(), &server).await;
    let sets_after_login = keystore.sets();

    keystore.lock(LockedAnswer::Error);
    let result = resume(home.path(), keystore.clone()).await;

    assert!(
        matches!(&result, Err(KupferdrahtError::Generic(msg)) if msg.contains("locked")),
        "the keystore's own error surfaces: {result:?}"
    );
    assert_eq!(keystore.sets(), sets_after_login, "nothing was written");
}
