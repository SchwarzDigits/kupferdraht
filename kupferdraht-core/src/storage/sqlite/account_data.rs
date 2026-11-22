// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! The persisted account data, CRUD. One row per account-data key
//! (e.g. `WIRE_RECEIPT_MODE`, `labels`); written by Step 10c's login
//! pipeline and Step 10d's `user.properties-set` /
//! `user.properties-delete` event arms, read by Step 10e's
//! `Client::account_data` / `Client::account_data_entry` / typed
//! getters. Step 10b.
//!
//! Per ADR-0003 + ADR-0012 the table receives no field encryption —
//! these rows hold server-administered policy, not user-secret
//! content.

use rusqlite::{OptionalExtension, params};

use crate::error::KupferdrahtError;
use crate::storage::records::AccountDataEntry;

use super::SqliteStore;

pub(super) fn replace_account_data(
    store: &SqliteStore,
    entries: &[AccountDataEntry],
) -> Result<(), KupferdrahtError> {
    let mut conn = store.lock()?;
    let tx = super::write_tx::begin(&mut conn)
        .map_err(|e| KupferdrahtError::Storage(format!("replace_account_data begin tx: {e}")))?;
    tx.execute("DELETE FROM user_properties", [])
        .map_err(|e| KupferdrahtError::Storage(format!("replace_account_data clear: {e}")))?;
    for entry in entries {
        tx.execute(
            "INSERT INTO user_properties (
                key, value, last_synced_at_ms
             ) VALUES (?1, ?2, ?3)",
            params![entry.key, entry.value, entry.last_synced_at_ms],
        )
        .map_err(|e| {
            KupferdrahtError::Storage(format!("replace_account_data insert {}: {e}", entry.key))
        })?;
    }
    tx.commit()
        .map_err(|e| KupferdrahtError::Storage(format!("replace_account_data commit: {e}")))?;
    Ok(())
}

pub(super) fn upsert_account_data_entry(
    store: &SqliteStore,
    entry: &AccountDataEntry,
) -> Result<(), KupferdrahtError> {
    let conn = store.lock()?;
    conn.execute(
        "INSERT INTO user_properties (
            key, value, last_synced_at_ms
         ) VALUES (?1, ?2, ?3)
         ON CONFLICT(key) DO UPDATE SET
             value = excluded.value,
             last_synced_at_ms = excluded.last_synced_at_ms",
        params![entry.key, entry.value, entry.last_synced_at_ms],
    )
    .map_err(|e| {
        KupferdrahtError::Storage(format!("upsert_account_data_entry {}: {e}", entry.key))
    })?;
    Ok(())
}

pub(super) fn delete_account_data_entry(
    store: &SqliteStore,
    key: &str,
) -> Result<(), KupferdrahtError> {
    let conn = store.lock()?;
    conn.execute("DELETE FROM user_properties WHERE key = ?1", params![key])
        .map_err(|e| KupferdrahtError::Storage(format!("delete_account_data_entry {key}: {e}")))?;
    Ok(())
}

pub(super) fn list_account_data(
    store: &SqliteStore,
) -> Result<Vec<AccountDataEntry>, KupferdrahtError> {
    let conn = store.lock()?;
    let mut stmt = conn
        .prepare("SELECT key, value, last_synced_at_ms FROM user_properties")
        .map_err(|e| KupferdrahtError::Storage(format!("list_account_data prepare: {e}")))?;
    let rows: Vec<AccountDataEntry> = stmt
        .query_map([], |row| {
            Ok(AccountDataEntry {
                key: row.get(0)?,
                value: row.get(1)?,
                last_synced_at_ms: row.get(2)?,
            })
        })
        .map_err(|e| KupferdrahtError::Storage(format!("list_account_data query: {e}")))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| KupferdrahtError::Storage(format!("list_account_data row: {e}")))?;
    Ok(rows)
}

pub(super) fn get_account_data_entry(
    store: &SqliteStore,
    key: &str,
) -> Result<Option<AccountDataEntry>, KupferdrahtError> {
    let conn = store.lock()?;
    conn.query_row(
        "SELECT key, value, last_synced_at_ms
         FROM user_properties WHERE key = ?1",
        params![key],
        |row| {
            Ok(AccountDataEntry {
                key: row.get(0)?,
                value: row.get(1)?,
                last_synced_at_ms: row.get(2)?,
            })
        },
    )
    .optional()
    .map_err(|e| KupferdrahtError::Storage(format!("get_account_data_entry {key}: {e}")))
}

#[cfg(test)]
mod tests {
    use crate::storage::records::{AccountDataEntry, Store};
    use crate::storage::sqlite::fresh_store;

    fn sample(key: &str, value: &str, ts: i64) -> AccountDataEntry {
        AccountDataEntry {
            key: key.to_string(),
            value: value.to_string(),
            last_synced_at_ms: ts,
        }
    }

    #[tokio::test]
    async fn replace_account_data_round_trips_each_entry() {
        let (_dir, store) = fresh_store();
        let snapshot = vec![
            sample("WIRE_RECEIPT_MODE", "1", 1_700_000_000_000),
            sample(
                "labels",
                r#"[{"id":"work","name":"Work"}]"#,
                1_700_000_000_000,
            ),
        ];

        store
            .replace_account_data(&snapshot)
            .await
            .expect("replace");
        let loaded = store.list_account_data().await.expect("list");

        assert_eq!(loaded.len(), 2);
        let by_key: std::collections::HashMap<_, _> =
            loaded.into_iter().map(|e| (e.key.clone(), e)).collect();
        assert_eq!(by_key["WIRE_RECEIPT_MODE"].value, "1");
        assert_eq!(
            by_key["WIRE_RECEIPT_MODE"].last_synced_at_ms,
            1_700_000_000_000
        );
    }

    #[tokio::test]
    async fn replace_account_data_is_full_replacement() {
        let (_dir, store) = fresh_store();
        store
            .replace_account_data(&[sample("WIRE_RECEIPT_MODE", "1", 1_000)])
            .await
            .expect("first replace");
        assert_eq!(store.list_account_data().await.expect("list #1").len(), 1);

        store
            .replace_account_data(&[sample("labels", "[]", 2_000)])
            .await
            .expect("second replace");
        let loaded = store.list_account_data().await.expect("list #2");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].key, "labels");
    }

    #[tokio::test]
    async fn replace_account_data_with_empty_slice_clears_table() {
        let (_dir, store) = fresh_store();
        store
            .replace_account_data(&[sample("WIRE_RECEIPT_MODE", "1", 1_000)])
            .await
            .expect("seed");
        store
            .replace_account_data(&[])
            .await
            .expect("replace empty");
        assert!(store.list_account_data().await.expect("list").is_empty());
    }

    #[tokio::test]
    async fn get_account_data_entry_returns_some_when_present() {
        let (_dir, store) = fresh_store();
        store
            .replace_account_data(&[sample("WIRE_RECEIPT_MODE", "1", 1_000)])
            .await
            .expect("seed");
        let found = store
            .get_account_data_entry("WIRE_RECEIPT_MODE")
            .await
            .expect("get");
        let entry = found.expect("present");
        assert_eq!(entry.key, "WIRE_RECEIPT_MODE");
        assert_eq!(entry.value, "1");
    }

    #[tokio::test]
    async fn get_account_data_entry_returns_none_when_absent() {
        let (_dir, store) = fresh_store();
        let found = store
            .get_account_data_entry("nonexistent")
            .await
            .expect("get");
        assert!(found.is_none());
    }

    #[tokio::test]
    async fn upsert_account_data_entry_inserts_new_row() {
        let (_dir, store) = fresh_store();
        store
            .upsert_account_data_entry(&sample("WIRE_RECEIPT_MODE", "1", 1_000))
            .await
            .expect("upsert");
        let loaded = store.list_account_data().await.expect("list");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].key, "WIRE_RECEIPT_MODE");
    }

    #[tokio::test]
    async fn upsert_account_data_entry_updates_existing_row() {
        let (_dir, store) = fresh_store();
        store
            .upsert_account_data_entry(&sample("WIRE_RECEIPT_MODE", "1", 1_000))
            .await
            .expect("first upsert");
        // Same key, new value + timestamp — should replace, not duplicate.
        store
            .upsert_account_data_entry(&sample("WIRE_RECEIPT_MODE", "0", 2_000))
            .await
            .expect("second upsert");
        let loaded = store.list_account_data().await.expect("list");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].value, "0");
        assert_eq!(loaded[0].last_synced_at_ms, 2_000);
    }

    #[tokio::test]
    async fn delete_account_data_entry_removes_existing_row() {
        let (_dir, store) = fresh_store();
        store
            .replace_account_data(&[
                sample("WIRE_RECEIPT_MODE", "1", 1_000),
                sample("labels", "[]", 1_000),
            ])
            .await
            .expect("seed");
        store
            .delete_account_data_entry("WIRE_RECEIPT_MODE")
            .await
            .expect("delete");
        let loaded = store.list_account_data().await.expect("list");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].key, "labels");
    }

    #[tokio::test]
    async fn delete_account_data_entry_is_idempotent_for_missing_key() {
        let (_dir, store) = fresh_store();
        // No seed — table is empty. Delete must still succeed.
        store
            .delete_account_data_entry("never-was-set")
            .await
            .expect("idempotent delete");
        assert!(store.list_account_data().await.expect("list").is_empty());
    }
}
