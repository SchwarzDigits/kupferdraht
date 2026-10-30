// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Engine identity of the linked `SQLite`: [`SqliteStore::storage_engine`]
//! reads engine, cipher scheme and header mode off the live connection.

use super::SqliteStore;
use crate::error::KupferdrahtError;
use crate::storage::StorageEngineInfo;

/// What the engine calls itself in `sqlite3mc_version()`, followed by a
/// space and the release.
const ENGINE_NAME: &str = "SQLite3 Multiple Ciphers";

/// What the open connection reports: `sqlite3mc_version()` split into
/// engine name + release, `PRAGMA cipher`, `PRAGMA plaintext_header_size`.
///
/// # Errors
/// `KupferdrahtError::Storage` if the connection cannot be locked, the engine
/// does not answer (a mis-linked `SQLite` has no `sqlite3mc_version()`), or
/// its version string has an unexpected shape.
pub(super) fn storage_engine(store: &SqliteStore) -> Result<StorageEngineInfo, KupferdrahtError> {
    let conn = store.lock()?;
    let version_string: String = conn
        .query_row("SELECT sqlite3mc_version()", [], |row| row.get(0))
        .map_err(|e| KupferdrahtError::Storage(format!("storage engine identity: {e}")))?;
    let Some(version) = version_string.strip_prefix(ENGINE_NAME).map(str::trim) else {
        return Err(KupferdrahtError::Storage(format!(
            "unexpected storage engine identity {version_string:?}"
        )));
    };
    let cipher: String = conn
        .query_row("PRAGMA cipher", [], |row| row.get(0))
        .map_err(|e| KupferdrahtError::Storage(format!("storage engine cipher: {e}")))?;
    // SQLite3MC answers its cipher-parameter pragmas as TEXT ("32"), so
    // read the column as a string and parse — runtime data, never asserted.
    let header_text: String = conn
        .query_row("PRAGMA plaintext_header_size", [], |row| row.get(0))
        .map_err(|e| KupferdrahtError::Storage(format!("storage engine header mode: {e}")))?;
    let plaintext_header_size: u32 = header_text.trim().parse().map_err(|e| {
        KupferdrahtError::Storage(format!(
            "storage engine header mode {header_text:?} is not a number: {e}"
        ))
    })?;
    Ok(StorageEngineInfo {
        engine: ENGINE_NAME.to_owned(),
        version: version.to_owned(),
        cipher,
        plaintext_header_size,
    })
}

/// Engine identity (ADR-0021): the `SQLite` linked into the workspace
/// must be `SQLite3MC`, not vanilla `SQLite` (or `SQLCipher`). Only
/// `SQLite3MC` registers the `sqlite3mc_version()` SQL function; this
/// proves the *engine* is in place independent of any `PRAGMA key`.
#[cfg(test)]
mod tests {
    use super::super::fresh_store;

    #[test]
    fn linked_engine_is_sqlite3mc() {
        let (_dir, store) = fresh_store();
        let conn = store.lock().expect("lock connection");
        let version: String = conn
            .query_row("SELECT sqlite3mc_version()", [], |row| row.get(0))
            .expect("sqlite3mc_version(): linked engine must be SQLite3MC");
        assert!(
            version.starts_with("SQLite3 Multiple Ciphers"),
            "expected an SQLite3MC version string, got: {version:?}"
        );
    }

    #[test]
    fn storage_engine_reports_chacha20_with_a_32_byte_plaintext_header() {
        let (_dir, store) = fresh_store();
        let info = super::storage_engine(&store).expect("storage engine");
        assert_eq!(info.engine, "SQLite3 Multiple Ciphers");
        assert!(!info.version.is_empty(), "engine release must be reported");
        assert_eq!(info.cipher, "chacha20");
        assert_eq!(info.plaintext_header_size, 32);
    }
}
