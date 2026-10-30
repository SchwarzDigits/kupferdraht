// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Device-level account registry + app settings — the `GlobalStore`.
//!
//! The per-account [`Store`](super::Store) instances are each encrypted
//! behind their own key, so there is nowhere to record *which* accounts
//! exist or which is active. `GlobalStore` fills that gap: one device-wide
//! store, openable at app start **without** opening any account, backing
//! an account picker.
//!
//! The shipped default is a whole-DB-encrypted `global.db` (`SQLite3MC`,
//! ADR-0021) under a device-level key, stored in the platform keystore
//! under the reserved id [`key_id::GLOBAL_DB`] — distinct from any account
//! key, so it is openable at app start without opening an account.

use std::path::Path;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rusqlite::{Connection, OptionalExtension, params};

use crate::error::KupferdrahtError;
use crate::keystore::{SecureKeyStore, key_id};
use crate::storage::db_key::DbKey;
use crate::storage::{KeyLock, load_or_create_key_locked};

/// One row of the device account registry — enough to render an account
/// picker without opening the account.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AccountEntry {
    /// Qualified user id (`<uuid>@<domain>`) — the registry primary key.
    pub account_id: String,
    /// Wire API base URL the account is homed on.
    pub server: String,
    /// `@handle`, if known.
    pub handle: Option<String>,
    /// Human-facing display name, if known.
    pub display_name: Option<String>,
    /// Last time this account was active (epoch ms).
    pub last_active_ms: i64,
}

/// Device-wide registry + settings. One instance per device; openable
/// without opening any per-account [`Store`](super::Store).
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
pub trait GlobalStore: super::MaybeSendSync {
    /// Every registered account.
    async fn list_accounts(&self) -> Result<Vec<AccountEntry>, KupferdrahtError>;
    /// Insert or update an account by `account_id`.
    async fn upsert_account(&self, entry: &AccountEntry) -> Result<(), KupferdrahtError>;
    /// Forget an account (does not touch its per-account database).
    async fn remove_account(&self, account_id: &str) -> Result<(), KupferdrahtError>;
    /// The account id the device last marked active, if any.
    async fn active_account(&self) -> Result<Option<String>, KupferdrahtError>;
    /// Set (or clear, with `None`) the active account.
    async fn set_active_account(&self, account_id: Option<&str>) -> Result<(), KupferdrahtError>;
    /// Read an app-wide setting.
    async fn get_setting(&self, key: &str) -> Result<Option<String>, KupferdrahtError>;
    /// Write an app-wide setting.
    async fn set_setting(&self, key: &str, value: &str) -> Result<(), KupferdrahtError>;
}

/// Settings key under which the active account id is stored.
const ACTIVE_ACCOUNT_KEY: &str = "active_account";

const GLOBAL_SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS accounts (
        account_id     TEXT PRIMARY KEY,
        server         TEXT NOT NULL,
        handle         TEXT,
        display_name   TEXT,
        last_active_ms INTEGER NOT NULL
    );
    CREATE TABLE IF NOT EXISTS settings (
        key   TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );";

/// The shipped SQLite-backed (whole-DB-encrypted) [`GlobalStore`]. The connection stays
/// open for the device session (no per-account open/close lifecycle). On every
/// target since Step 91 (ADR-0022): a file under `base_dir` on native, a file
/// in the OPFS pool in the browser.
pub struct SqliteGlobalStore {
    connection: Mutex<Connection>,
}

impl SqliteGlobalStore {
    fn open(path: &Path, key: &DbKey) -> Result<Self, KupferdrahtError> {
        let conn = crate::storage::open_connection(path)?;
        crate::storage::apply_whole_db_key(&conn, key)?;
        // WAL is native-only: the browser's VFS provides no shared memory
        // (ADR-0022 decision 5), so it keeps the rollback journal.
        #[cfg(not(target_arch = "wasm32"))]
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| KupferdrahtError::Storage(format!("global set WAL: {e}")))?;
        #[cfg(not(target_arch = "wasm32"))]
        crate::storage::set_multi_process_pragmas(&conn)?;
        conn.execute_batch(GLOBAL_SCHEMA)
            .map_err(|e| KupferdrahtError::Storage(format!("global schema: {e}")))?;
        Ok(Self {
            connection: Mutex::new(conn),
        })
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Connection>, KupferdrahtError> {
        // Step 92 (ADR-0023): see `SqliteStore::lock`.
        #[cfg(target_arch = "wasm32")]
        if crate::storage::browser_storage_evicted() {
            return Err(KupferdrahtError::Evicted);
        }
        self.connection
            .lock()
            .map_err(|e| KupferdrahtError::Storage(format!("global mutex poisoned: {e}")))
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl GlobalStore for SqliteGlobalStore {
    async fn list_accounts(&self) -> Result<Vec<AccountEntry>, KupferdrahtError> {
        let conn = self.lock()?;
        let mut stmt = conn
            .prepare(
                "SELECT account_id, server, handle, display_name, last_active_ms
                 FROM accounts ORDER BY last_active_ms DESC",
            )
            .map_err(|e| KupferdrahtError::Storage(format!("list_accounts prepare: {e}")))?;
        let rows = stmt
            .query_map([], |row| {
                Ok(AccountEntry {
                    account_id: row.get(0)?,
                    server: row.get(1)?,
                    handle: row.get(2)?,
                    display_name: row.get(3)?,
                    last_active_ms: row.get(4)?,
                })
            })
            .map_err(|e| KupferdrahtError::Storage(format!("list_accounts query: {e}")))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| KupferdrahtError::Storage(format!("list_accounts row: {e}")))
    }

    async fn upsert_account(&self, entry: &AccountEntry) -> Result<(), KupferdrahtError> {
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO accounts (account_id, server, handle, display_name, last_active_ms)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(account_id) DO UPDATE SET
                server = excluded.server,
                handle = excluded.handle,
                display_name = excluded.display_name,
                last_active_ms = excluded.last_active_ms",
            params![
                entry.account_id,
                entry.server,
                entry.handle,
                entry.display_name,
                entry.last_active_ms,
            ],
        )
        .map_err(|e| KupferdrahtError::Storage(format!("upsert_account: {e}")))?;
        Ok(())
    }

    async fn remove_account(&self, account_id: &str) -> Result<(), KupferdrahtError> {
        let conn = self.lock()?;
        conn.execute(
            "DELETE FROM accounts WHERE account_id = ?1",
            params![account_id],
        )
        .map_err(|e| KupferdrahtError::Storage(format!("remove_account: {e}")))?;
        Ok(())
    }

    async fn active_account(&self) -> Result<Option<String>, KupferdrahtError> {
        self.get_setting(ACTIVE_ACCOUNT_KEY).await
    }

    async fn set_active_account(&self, account_id: Option<&str>) -> Result<(), KupferdrahtError> {
        if let Some(id) = account_id {
            return self.set_setting(ACTIVE_ACCOUNT_KEY, id).await;
        }
        let conn = self.lock()?;
        conn.execute(
            "DELETE FROM settings WHERE key = ?1",
            params![ACTIVE_ACCOUNT_KEY],
        )
        .map_err(|e| KupferdrahtError::Storage(format!("clear active_account: {e}")))?;
        Ok(())
    }

    async fn get_setting(&self, key: &str) -> Result<Option<String>, KupferdrahtError> {
        let conn = self.lock()?;
        conn.query_row(
            "SELECT value FROM settings WHERE key = ?1",
            params![key],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|e| KupferdrahtError::Storage(format!("get_setting: {e}")))
    }

    async fn set_setting(&self, key: &str, value: &str) -> Result<(), KupferdrahtError> {
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )
        .map_err(|e| KupferdrahtError::Storage(format!("set_setting: {e}")))?;
        Ok(())
    }
}

/// Open the shipped default device registry at `base_dir/global.db`,
/// encrypted under the device-level key stored in the platform keystore
/// under the home-scoped id `kupferdraht.<home-id>.global-db`
/// (`key_id::global_db`, Step 93; generated on first launch). Returned
/// behind the [`GlobalStore`] trait so consumers never name the
/// concrete backend. In the browser `base_dir/global.db` names a file in
/// the OPFS pool (Step 91, ADR-0022); no directory is created.
///
/// # Errors
/// `KupferdrahtError::Storage` if the directory or database cannot be opened,
/// `KupferdrahtError::KeyUnavailable` if the database exists but the keystore
/// returns no key for it (ADR-0029), or `KupferdrahtError::Generic` if the
/// keystore fails.
pub fn open_default_global_store(
    base_dir: &Path,
    keystore: &Arc<dyn SecureKeyStore>,
) -> Result<Arc<dyn GlobalStore>, KupferdrahtError> {
    // Step 93: resolving the home creates the directory and reads (on a
    // first open: creates, under the home lock) its id.
    let home = super::KupferdrahtHome::open(base_dir.to_path_buf())?;
    open_global_store_in(&home, keystore)
}

/// [`open_default_global_store`] on an already-resolved home — the default
/// [`StorageProvider`](super::StorageProvider) comes through here.
///
/// # Errors
/// As [`open_default_global_store`].
pub(crate) fn open_global_store_in(
    home: &super::KupferdrahtHome,
    keystore: &Arc<dyn SecureKeyStore>,
) -> Result<Arc<dyn GlobalStore>, KupferdrahtError> {
    let database = home.global_db();
    // Step 85: the first-ever creation of the registry key runs under the
    // home's lock (the directory exists by now).
    let key: DbKey = load_or_create_key_locked(
        home,
        KeyLock::Home,
        keystore,
        &key_id::global_db(home.home_id()),
        &database,
    )?;
    let store = SqliteGlobalStore::open(&database, &key)?;
    Ok(Arc::new(store))
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod pragma_tests {
    use super::SqliteGlobalStore;
    use crate::storage::db_key::generate_db_key;

    /// Step 85 (open.md §69 residual): the registry connection waits for a
    /// concurrent writer (`busy_timeout`) and keeps full durability
    /// (`synchronous = FULL`, read back as 2).
    #[test]
    fn global_store_connection_carries_busy_timeout_and_full_synchronous() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = SqliteGlobalStore::open(&dir.path().join("global.db"), &generate_db_key())
            .expect("open global store");
        let conn = store.lock().expect("lock");
        let busy: i64 = conn
            .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
            .expect("busy_timeout");
        assert_eq!(busy, 5000, "busy_timeout in effect on global.db");
        let synchronous: i64 = conn
            .query_row("PRAGMA synchronous", [], |row| row.get(0))
            .expect("synchronous");
        assert_eq!(synchronous, 2, "synchronous = FULL on global.db");
    }
}
