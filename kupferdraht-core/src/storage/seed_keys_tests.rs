// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 110a (ADR-0035) — the one place a database key is made, in a home
//! whose account keys come from the account's seed. TDD red.
//!
//! Keys are stored and loaded as natively; only a *new* key of an account
//! differs: derived from the seed (`storage::account_keys`), refused with
//! `RecoveryDataRequired` without it. Device-level keys stay random; a
//! `Random` home is unchanged.

use std::sync::Arc;

use tempfile::TempDir;

use super::account_keys::{AccountKeyPurpose, AccountKeySource, SEED_LEN, derive_account_key};
use super::{KeyLock, KupferdrahtHome, load_or_create_key_locked};
use crate::error::KupferdrahtError;
use crate::keystore::{InMemoryKeyStore, SecureKeyStore, key_id};
use crate::storage::db_key::DbKey;

const ACCOUNT: &str = "user-uuid@example.test";
const SEED: [u8; SEED_LEN] = [0x5a; SEED_LEN];

fn keystore() -> Arc<dyn SecureKeyStore> {
    Arc::new(InMemoryKeyStore::new())
}

fn seed_home(dir: &TempDir) -> KupferdrahtHome {
    KupferdrahtHome::open(dir.path().to_path_buf())
        .expect("home")
        .with_account_keys(AccountKeySource::Seed)
}

fn store_seed(home: &KupferdrahtHome, keystore: &Arc<dyn SecureKeyStore>) {
    keystore
        .set_secret(key_id::account_seed(home.home_id(), ACCOUNT), SEED.to_vec())
        .expect("store the seed");
}

/// The `kupferdraht.db` key of `ACCOUNT` in `home`, made or loaded.
fn store_key(
    home: &KupferdrahtHome,
    keystore: &Arc<dyn SecureKeyStore>,
) -> Result<DbKey, KupferdrahtError> {
    load_or_create_key_locked(
        home,
        KeyLock::Account(ACCOUNT),
        keystore,
        &key_id::account_db(home.home_id(), ACCOUNT),
        &home.account_db(ACCOUNT),
    )
}

fn stored(keystore: &Arc<dyn SecureKeyStore>, id: &str) -> Option<Vec<u8>> {
    keystore.get_secret(id.to_owned()).expect("keystore read")
}

#[test]
fn a_seed_home_derives_a_new_account_key_and_stores_it_under_the_native_id() {
    let dir = TempDir::new().expect("tempdir");
    let home = seed_home(&dir);
    let keystore = keystore();
    store_seed(&home, &keystore);

    let key = store_key(&home, &keystore).expect("derived key");

    let expected = derive_account_key(&SEED, AccountKeyPurpose::Store);
    assert_eq!(key.key(), expected.key(), "the key comes from the seed");
    let id = key_id::account_db(home.home_id(), ACCOUNT);
    assert_eq!(
        stored(&keystore, id.namespaced()).as_deref(),
        Some(expected.key().as_slice()),
        "stored under the native id, like any key"
    );
}

#[test]
fn a_seed_home_derives_the_mls_key_with_its_own_label() {
    let dir = TempDir::new().expect("tempdir");
    let home = seed_home(&dir);
    let keystore = keystore();
    store_seed(&home, &keystore);

    let key = load_or_create_key_locked(
        &home,
        KeyLock::Account(ACCOUNT),
        &keystore,
        &key_id::account_mls(home.home_id(), ACCOUNT),
        &home.account_mls(ACCOUNT),
    )
    .expect("derived key");

    let expected = derive_account_key(&SEED, AccountKeyPurpose::Mls);
    assert_eq!(key.key(), expected.key());
}

/// `global.db` belongs to the device, not to an account: no seed is
/// needed, and its key is drawn at random as natively.
#[test]
fn the_global_key_stays_random_in_a_seed_home() {
    let dir = TempDir::new().expect("tempdir");
    let home = seed_home(&dir);
    let keystore = keystore();

    let key = load_or_create_key_locked(
        &home,
        KeyLock::Home,
        &keystore,
        &key_id::global_db(home.home_id()),
        &home.global_db(),
    )
    .expect("a device key needs no seed");

    assert_ne!(
        key.key(),
        derive_account_key(&[0u8; SEED_LEN], AccountKeyPurpose::Store).key()
    );
}

#[test]
fn without_the_seed_an_account_key_is_refused_and_nothing_is_written() {
    let dir = TempDir::new().expect("tempdir");
    let home = seed_home(&dir);
    let keystore = keystore();

    let result = store_key(&home, &keystore);

    assert!(
        matches!(result, Err(KupferdrahtError::RecoveryDataRequired(_))),
        "no seed, no key: {:?}",
        result.err()
    );
    let id = key_id::account_db(home.home_id(), ACCOUNT);
    assert_eq!(stored(&keystore, id.namespaced()), None, "nothing written");
}

/// ADR-0029 refuses a new key for an existing database. A derived key is
/// not new — the seed yields the same key again — so it is restored.
#[test]
fn an_existing_database_whose_key_is_missing_gets_the_derived_key_back() {
    let dir = TempDir::new().expect("tempdir");
    let home = seed_home(&dir);
    let keystore = keystore();
    store_seed(&home, &keystore);
    let database = home.account_db(ACCOUNT);
    std::fs::create_dir_all(database.parent().expect("account dir")).expect("mkdir");
    std::fs::write(&database, b"an existing database").expect("write");

    let key = store_key(&home, &keystore).expect("the same key again");

    assert_eq!(
        key.key(),
        derive_account_key(&SEED, AccountKeyPurpose::Store).key()
    );
}

/// Loading is native: a stored key wins, whatever the seed would yield.
#[test]
fn a_stored_key_is_loaded_as_is() {
    let dir = TempDir::new().expect("tempdir");
    let home = seed_home(&dir);
    let keystore = keystore();
    store_seed(&home, &keystore);
    let id = key_id::account_db(home.home_id(), ACCOUNT);
    keystore
        .set_secret(id.namespaced().to_owned(), vec![7u8; 32])
        .expect("store a key");

    let key = store_key(&home, &keystore).expect("loaded");

    assert_eq!(key.key(), &[7u8; 32]);
}

/// A `Random` home — native — needs no seed and draws the key.
#[test]
fn a_random_home_is_unchanged() {
    let dir = TempDir::new().expect("tempdir");
    let home = KupferdrahtHome::open(dir.path().to_path_buf()).expect("home");
    let keystore = keystore();

    assert_eq!(home.account_keys(), AccountKeySource::Random);
    let key = store_key(&home, &keystore).expect("a random key");

    let id = key_id::account_db(home.home_id(), ACCOUNT);
    assert_eq!(
        stored(&keystore, id.namespaced()).as_deref(),
        Some(key.key().as_slice())
    );
}
