// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `feature_configs` table CRUD. One row per Wire team-feature key
//! (`fileSharing`, `mls`, `mlsE2EId`, …); written by Step 10c's
//! login pipeline and Step 10d's `feature-config.update` event arm,
//! read by Step 10e's `Client::feature_configs` /
//! `Client::feature_config`. Step 10b.
//!
//! Per ADR-0003 + ADR-0012 the table receives no field encryption —
//! these rows hold server-administered policy, not user-secret
//! content.

use rusqlite::{OptionalExtension, params};

use crate::error::KupferdrahtError;
use crate::storage::records::FeatureConfigEntry;

use super::SqliteStore;

pub(super) fn replace_feature_configs(
    store: &SqliteStore,
    entries: &[FeatureConfigEntry],
) -> Result<(), KupferdrahtError> {
    let mut conn = store.lock()?;
    let tx = super::write_tx::begin(&mut conn)
        .map_err(|e| KupferdrahtError::Storage(format!("replace_feature_configs begin tx: {e}")))?;
    tx.execute("DELETE FROM feature_configs", [])
        .map_err(|e| KupferdrahtError::Storage(format!("replace_feature_configs clear: {e}")))?;
    for entry in entries {
        tx.execute(
            "INSERT INTO feature_configs (
                name, status, config_json, last_synced_at_ms
             ) VALUES (?1, ?2, ?3, ?4)",
            params![
                entry.name,
                entry.status,
                entry.config_json,
                entry.last_synced_at_ms,
            ],
        )
        .map_err(|e| {
            KupferdrahtError::Storage(format!(
                "replace_feature_configs insert {}: {e}",
                entry.name
            ))
        })?;
    }
    tx.commit()
        .map_err(|e| KupferdrahtError::Storage(format!("replace_feature_configs commit: {e}")))?;
    Ok(())
}

pub(super) fn upsert_feature_config(
    store: &SqliteStore,
    entry: &FeatureConfigEntry,
) -> Result<(), KupferdrahtError> {
    let conn = store.lock()?;
    conn.execute(
        "INSERT INTO feature_configs (
            name, status, config_json, last_synced_at_ms
         ) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(name) DO UPDATE SET
             status = excluded.status,
             config_json = excluded.config_json,
             last_synced_at_ms = excluded.last_synced_at_ms",
        params![
            entry.name,
            entry.status,
            entry.config_json,
            entry.last_synced_at_ms,
        ],
    )
    .map_err(|e| KupferdrahtError::Storage(format!("upsert_feature_config {}: {e}", entry.name)))?;
    Ok(())
}

pub(super) fn list_feature_configs(
    store: &SqliteStore,
) -> Result<Vec<FeatureConfigEntry>, KupferdrahtError> {
    let conn = store.lock()?;
    let mut stmt = conn
        .prepare("SELECT name, status, config_json, last_synced_at_ms FROM feature_configs")
        .map_err(|e| KupferdrahtError::Storage(format!("list_feature_configs prepare: {e}")))?;
    let rows: Vec<FeatureConfigEntry> = stmt
        .query_map([], |row| {
            Ok(FeatureConfigEntry {
                name: row.get(0)?,
                status: row.get(1)?,
                config_json: row.get(2)?,
                last_synced_at_ms: row.get(3)?,
            })
        })
        .map_err(|e| KupferdrahtError::Storage(format!("list_feature_configs query: {e}")))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| KupferdrahtError::Storage(format!("list_feature_configs row: {e}")))?;
    Ok(rows)
}

pub(super) fn get_feature_config(
    store: &SqliteStore,
    name: &str,
) -> Result<Option<FeatureConfigEntry>, KupferdrahtError> {
    let conn = store.lock()?;
    conn.query_row(
        "SELECT name, status, config_json, last_synced_at_ms
         FROM feature_configs WHERE name = ?1",
        params![name],
        |row| {
            Ok(FeatureConfigEntry {
                name: row.get(0)?,
                status: row.get(1)?,
                config_json: row.get(2)?,
                last_synced_at_ms: row.get(3)?,
            })
        },
    )
    .optional()
    .map_err(|e| KupferdrahtError::Storage(format!("get_feature_config {name}: {e}")))
}

#[cfg(test)]
mod tests {
    use crate::storage::records::{FeatureConfigEntry, Store};
    use crate::storage::sqlite::fresh_store;

    fn sample(name: &str, status: Option<&str>, config_json: &str, ts: i64) -> FeatureConfigEntry {
        FeatureConfigEntry {
            name: name.to_string(),
            status: status.map(str::to_string),
            config_json: config_json.to_string(),
            last_synced_at_ms: ts,
        }
    }

    #[tokio::test]
    async fn replace_feature_configs_round_trips_each_entry() {
        let (_dir, store) = fresh_store();
        let snapshot = vec![
            sample(
                "fileSharing",
                Some("enabled"),
                r#"{"opaque":true}"#,
                1_700_000_000_000,
            ),
            sample(
                "mls",
                Some("enabled"),
                r#"{"defaultProtocol":"mls"}"#,
                1_700_000_000_000,
            ),
            sample(
                "classifiedDomains",
                Some("disabled"),
                "{}",
                1_700_000_000_000,
            ),
        ];

        store
            .replace_feature_configs(&snapshot)
            .await
            .expect("replace");
        let loaded = store.list_feature_configs().await.expect("list");

        assert_eq!(loaded.len(), 3);
        let by_name: std::collections::HashMap<_, _> =
            loaded.into_iter().map(|e| (e.name.clone(), e)).collect();
        assert_eq!(by_name["fileSharing"].status.as_deref(), Some("enabled"));
        assert_eq!(by_name["fileSharing"].config_json, r#"{"opaque":true}"#);
        assert_eq!(by_name["fileSharing"].last_synced_at_ms, 1_700_000_000_000);
    }

    #[tokio::test]
    async fn replace_feature_configs_persists_null_status() {
        let (_dir, store) = fresh_store();
        // Some Wire feature configs ship config-only (no `status` field).
        let snapshot = vec![sample(
            "conferenceCalling",
            None,
            r#"{"useSFTForOneToOneCalls":true}"#,
            1_700_000_000_000,
        )];

        store
            .replace_feature_configs(&snapshot)
            .await
            .expect("replace");
        let loaded = store.list_feature_configs().await.expect("list");

        assert_eq!(loaded.len(), 1);
        assert!(loaded[0].status.is_none());
        assert_eq!(loaded[0].config_json, r#"{"useSFTForOneToOneCalls":true}"#);
    }

    #[tokio::test]
    async fn replace_feature_configs_is_full_replacement() {
        let (_dir, store) = fresh_store();
        store
            .replace_feature_configs(&[sample("fileSharing", Some("enabled"), "{}", 1_000)])
            .await
            .expect("first replace");
        assert_eq!(
            store.list_feature_configs().await.expect("list #1").len(),
            1
        );

        // Second replace replaces the snapshot wholesale: old key gone, new key in.
        store
            .replace_feature_configs(&[sample("mls", Some("enabled"), "{}", 2_000)])
            .await
            .expect("second replace");
        let loaded = store.list_feature_configs().await.expect("list #2");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].name, "mls");
    }

    #[tokio::test]
    async fn replace_feature_configs_with_empty_slice_clears_table() {
        let (_dir, store) = fresh_store();
        store
            .replace_feature_configs(&[sample("fileSharing", Some("enabled"), "{}", 1_000)])
            .await
            .expect("seed");
        store
            .replace_feature_configs(&[])
            .await
            .expect("replace empty");
        assert!(store.list_feature_configs().await.expect("list").is_empty());
    }

    #[tokio::test]
    async fn get_feature_config_returns_some_when_present() {
        let (_dir, store) = fresh_store();
        store
            .replace_feature_configs(&[sample("fileSharing", Some("enabled"), r#"{"x":1}"#, 1_000)])
            .await
            .expect("seed");
        let found = store.get_feature_config("fileSharing").await.expect("get");
        let entry = found.expect("present");
        assert_eq!(entry.name, "fileSharing");
        assert_eq!(entry.config_json, r#"{"x":1}"#);
    }

    #[tokio::test]
    async fn get_feature_config_returns_none_when_absent() {
        let (_dir, store) = fresh_store();
        let found = store.get_feature_config("nonexistent").await.expect("get");
        assert!(found.is_none());
    }

    #[tokio::test]
    async fn upsert_feature_config_inserts_new_row() {
        let (_dir, store) = fresh_store();
        store
            .upsert_feature_config(&sample("fileSharing", Some("enabled"), "{}", 1_000))
            .await
            .expect("upsert");
        let loaded = store.list_feature_configs().await.expect("list");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].name, "fileSharing");
    }

    #[tokio::test]
    async fn upsert_feature_config_updates_existing_row() {
        let (_dir, store) = fresh_store();
        store
            .upsert_feature_config(&sample("fileSharing", Some("enabled"), "{}", 1_000))
            .await
            .expect("first upsert");
        // Same name, new status + config + timestamp — should replace, not duplicate.
        store
            .upsert_feature_config(&sample(
                "fileSharing",
                Some("disabled"),
                r#"{"reason":"team-policy"}"#,
                2_000,
            ))
            .await
            .expect("second upsert");
        let loaded = store.list_feature_configs().await.expect("list");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].status.as_deref(), Some("disabled"));
        assert_eq!(loaded[0].config_json, r#"{"reason":"team-policy"}"#);
        assert_eq!(loaded[0].last_synced_at_ms, 2_000);
    }
}
