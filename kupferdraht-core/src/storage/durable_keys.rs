// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Create a database's key ahead of the database, durably (Step 104).
//!
//! The synchronous open paths (`open_global_store_in`, `open_store_in`,
//! the `mls.db` key in the client builder) generate a missing key and
//! create the database with it in one call. The browser keystore
//! persists that key in the background, so the async callers create the
//! keys first and await [`crate::keystore::flush_pending_writes`]: a key
//! that cannot be stored fails the call before any database exists, and
//! the open that follows finds the key. A key the keystore did not store
//! is forgotten again: the keystore still serves it from memory, and a
//! retry would otherwise find it, skip the write, and create the
//! database under a key that was never stored.

use std::path::Path;
use std::sync::Arc;

use crate::error::KupferdrahtError;
use crate::keystore::key_id::KeyId;
use crate::keystore::{SecureKeyStore, flush_pending_writes, key_id, load_key};

use super::{KeyLock, KupferdrahtHome, load_or_create_key_locked};

/// Make the home's `global.db` key exist durably — the browser's
/// `Kupferdraht.open` (native managers build synchronously on a synchronous
/// keystore).
///
/// # Errors
/// As `load_or_create_key_locked`, plus a failed keystore write.
#[cfg(any(target_arch = "wasm32", test))]
pub(crate) async fn ensure_global_key_durable(
    home: &KupferdrahtHome,
    keystore: &Arc<dyn SecureKeyStore>,
) -> Result<(), KupferdrahtError> {
    let mut created = Vec::new();
    create_key_ahead(
        home,
        KeyLock::Home,
        keystore,
        &key_id::global_db(home.home_id()),
        &home.global_db(),
        &mut created,
    )?;
    flush_or_forget(keystore, created).await
}

/// Make an account's database keys exist durably: `mls.db` always,
/// `kupferdraht.db` when the default storage provider opens it
/// (`with_store_key`; a custom provider keys its own store).
///
/// # Errors
/// As `load_or_create_key_locked`, plus a failed keystore write.
pub(crate) async fn ensure_account_keys_durable(
    home: &KupferdrahtHome,
    keystore: &Arc<dyn SecureKeyStore>,
    account_id: &str,
    with_store_key: bool,
) -> Result<(), KupferdrahtError> {
    // The ids name directories: refuse an unsafe one before any is made.
    super::refuse_path_unsafe_account_id(account_id)?;
    let lock = KeyLock::Account(account_id);
    let mut created = Vec::new();
    if with_store_key {
        create_key_ahead(
            home,
            lock,
            keystore,
            &key_id::account_db(home.home_id(), account_id),
            &home.account_db(account_id),
            &mut created,
        )?;
    }
    create_key_ahead(
        home,
        lock,
        keystore,
        &key_id::account_mls(home.home_id(), account_id),
        &home.account_mls(account_id),
        &mut created,
    )?;
    flush_or_forget(keystore, created).await
}

/// Store `seed` as the seed of `account_id` (ADR-0035) — created by a
/// first setup, or opened from recovery data (Step 112) — and make it
/// durable before anything of the account is created: the account's keys
/// derive from it, and a seed that was never stored would leave databases
/// no recovery data can open.
///
/// # Errors
/// Keystore errors, a failed keystore write.
#[cfg(any(target_arch = "wasm32", test))]
pub(crate) async fn store_seed_durably(
    keystore: &Arc<dyn SecureKeyStore>,
    home_id: &str,
    account_id: &str,
    seed: &[u8; super::account_keys::SEED_LEN],
) -> Result<(), KupferdrahtError> {
    let id = key_id::account_seed(home_id, account_id);
    debug_assert!(
        keystore.get_secret(id.clone())?.is_none(),
        "a first setup starts without a seed"
    );
    keystore.set_secret(id.clone(), seed.to_vec())?;
    flush_or_forget(keystore, vec![id]).await
}

/// Create `key` for `database` when the keystore has none; `created`
/// receives the id when this call wrote it (generated or adopted).
fn create_key_ahead(
    home: &KupferdrahtHome,
    lock: KeyLock<'_>,
    keystore: &Arc<dyn SecureKeyStore>,
    key: &KeyId,
    database: &Path,
    created: &mut Vec<String>,
) -> Result<(), KupferdrahtError> {
    let absent = load_key(keystore, key.namespaced())?.is_none();
    load_or_create_key_locked(home, lock, keystore, key, database)?;
    debug_assert!(
        load_key(keystore, key.namespaced())?.is_some(),
        "the key exists once load_or_create_key_locked returned"
    );
    if absent {
        created.push(key.namespaced().to_owned());
    }
    Ok(())
}

/// Await the keystore's background writes. When one failed, forget the
/// keys this call wrote: the keystore still serves them from memory, and
/// a retry would otherwise find them, skip the write, and create a
/// database under a key that was never stored.
async fn flush_or_forget(
    keystore: &Arc<dyn SecureKeyStore>,
    created: Vec<String>,
) -> Result<(), KupferdrahtError> {
    debug_assert!(
        created.iter().all(|id| id.starts_with("kupferdraht.")),
        "only home-scoped keys and seeds are created ahead"
    );
    let flushed = flush_pending_writes().await;
    if flushed.is_err() {
        for id in created {
            if let Err(e) = keystore.delete_secret(id.clone()) {
                tracing::warn!(%id, error = %e, "forgetting an unstored database key failed");
            }
        }
    }
    flushed
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use super::{ensure_account_keys_durable, ensure_global_key_durable};
    use crate::error::KupferdrahtError;
    use crate::keystore::pending_writes::track_write;
    use crate::keystore::{SecureKeyStore, key_id, load_key};
    use crate::storage::KupferdrahtHome;

    /// A keystore that, like the browser's, stores in memory at once and
    /// reports the durable write separately — failing the first
    /// `failures_left` writes, then succeeding.
    struct FailingWriteKeyStore {
        secrets: Mutex<HashMap<String, Vec<u8>>>,
        failures_left: AtomicUsize,
    }

    impl SecureKeyStore for FailingWriteKeyStore {
        fn get_secret(&self, id: String) -> Result<Option<Vec<u8>>, KupferdrahtError> {
            Ok(self.secrets.lock().expect("lock").get(&id).cloned())
        }

        fn set_secret(&self, id: String, secret: Vec<u8>) -> Result<(), KupferdrahtError> {
            self.secrets.lock().expect("lock").insert(id, secret);
            let fails = self
                .failures_left
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                .is_ok();
            let outcome = if fails {
                Err(KupferdrahtError::Generic("quota exceeded".into()))
            } else {
                Ok(())
            };
            let _ = track_write().send(outcome);
            Ok(())
        }

        fn delete_secret(&self, id: String) -> Result<(), KupferdrahtError> {
            self.secrets.lock().expect("lock").remove(&id);
            Ok(())
        }
    }

    fn keystore_failing(failures: usize) -> Arc<dyn SecureKeyStore> {
        Arc::new(FailingWriteKeyStore {
            secrets: Mutex::new(HashMap::new()),
            failures_left: AtomicUsize::new(failures),
        })
    }

    fn failing_keystore() -> Arc<dyn SecureKeyStore> {
        keystore_failing(usize::MAX)
    }

    #[tokio::test]
    async fn a_global_key_that_cannot_be_stored_fails_before_global_db_exists() {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = KupferdrahtHome::open(dir.path().to_path_buf()).expect("home");

        let result = ensure_global_key_durable(&home, &failing_keystore()).await;

        assert!(
            matches!(&result, Err(e) if e.to_string().contains("quota exceeded")),
            "the failed write surfaces: {result:?}"
        );
        assert!(
            !home.global_db().exists(),
            "no database without a stored key"
        );
    }

    #[tokio::test]
    async fn account_keys_that_cannot_be_stored_fail_before_the_databases_exist() {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = KupferdrahtHome::open(dir.path().to_path_buf()).expect("home");
        let account = "0f5f5a9e-1111-4222-8333-444455556666@example.com";

        let result = ensure_account_keys_durable(&home, &failing_keystore(), account, true).await;

        assert!(
            matches!(&result, Err(e) if e.to_string().contains("quota exceeded")),
            "the failed write surfaces: {result:?}"
        );
        assert!(
            !home.account_db(account).exists(),
            "no kupferdraht.db without its key"
        );
        assert!(
            !home.account_mls(account).exists(),
            "no mls.db without its key"
        );
    }

    /// The keystore serves a key from memory although its write failed:
    /// the key must be forgotten, or the retry finds it, writes nothing
    /// and creates the databases under a key that was never stored.
    #[tokio::test]
    async fn a_refused_write_leaves_no_key_behind_and_the_retry_stores_fresh_ones() {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = KupferdrahtHome::open(dir.path().to_path_buf()).expect("home");
        let account = "0f5f5a9e-1111-4222-8333-444455556666@example.com";
        let db_id = key_id::account_db(home.home_id(), account);
        let mls_id = key_id::account_mls(home.home_id(), account);
        // Both writes of the first attempt fail; every later one succeeds.
        let keystore = keystore_failing(2);

        let first = ensure_account_keys_durable(&home, &keystore, account, true).await;
        assert!(
            first.is_err(),
            "the refused write fails the call: {first:?}"
        );
        assert!(
            load_key(&keystore, db_id.namespaced())
                .expect("get")
                .is_none(),
            "a key the keystore never stored is forgotten"
        );
        assert!(
            load_key(&keystore, mls_id.namespaced())
                .expect("get")
                .is_none(),
            "the mls.db key as well"
        );

        ensure_account_keys_durable(&home, &keystore, account, true)
            .await
            .expect("the retry stores fresh keys");
        assert!(
            load_key(&keystore, db_id.namespaced())
                .expect("get")
                .is_some(),
            "the retry generated and stored the kupferdraht.db key"
        );
        assert!(
            load_key(&keystore, mls_id.namespaced())
                .expect("get")
                .is_some(),
            "and the mls.db key"
        );
    }
}
