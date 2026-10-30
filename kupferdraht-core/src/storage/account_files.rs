// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Removing an account's databases (Step 110a, ADR-0035): what a first
//! setup finds left without a seed by an earlier build, and what a
//! cancelled first setup created. Only ever before the account's stores
//! are open, or after they were dropped. On the storage server (Step 111b)
//! a new seed is a new storage-login key, whose subject holds nothing:
//! there is nothing left over to remove.

use crate::error::KupferdrahtError;
use crate::storage::KupferdrahtHome;

/// Remove the databases of `account_id` in `home` when they exist. Native:
/// the account directory as a whole (databases, locks, asset cache); the
/// browser: `kupferdraht.db` and `mls.db` through their prepared VFS
/// (`storage::prepare_account_databases` first).
///
/// # Errors
/// `KupferdrahtError::InvalidInput` for a path-unsafe account id;
/// `KupferdrahtError::Storage` when a file cannot be removed.
pub(crate) fn delete_account_databases(
    home: &KupferdrahtHome,
    account_id: &str,
) -> Result<(), KupferdrahtError> {
    super::refuse_path_unsafe_account_id(account_id)?;
    #[cfg(not(target_arch = "wasm32"))]
    {
        let dir = home.account_dir(account_id);
        match std::fs::remove_dir_all(&dir) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(KupferdrahtError::Storage(format!(
                    "remove {}: {e}",
                    dir.display()
                )));
            }
        }
        debug_assert!(!dir.exists(), "the account directory is gone");
    }
    #[cfg(target_arch = "wasm32")]
    for database in [home.account_db(account_id), home.account_mls(account_id)] {
        if super::remote::server::storage_server_configured()
            && super::browser::browser_database_vfs(&database).is_none()
        {
            // Never registered in this worker: it created nothing there.
            continue;
        }
        super::browser::delete_browser_database(&database)?;
    }
    Ok(())
}

/// Remove what the account has left without a seed before a first setup
/// creates one: its databases on this device (the browser's local mode
/// prepares them first); on the storage server nothing — the new seed's
/// subject is empty.
///
/// # Errors
/// As [`delete_account_databases`].
pub(crate) fn delete_leftover_account_databases(
    home: &KupferdrahtHome,
    account_id: &str,
) -> Result<(), KupferdrahtError> {
    #[cfg(target_arch = "wasm32")]
    if super::remote::server::storage_server_configured() {
        return Ok(());
    }
    delete_account_databases(home, account_id)
}
