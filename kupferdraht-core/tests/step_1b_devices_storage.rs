// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 1b — `devices` table round-trip.
//!
//! Failing tests per PLAN.md Step 1 "TDD starter tests, 1b":
//! * `devices` table gains one row keyed by the new `client_id` after
//!   `save_device`.
//! * Lookup by `user_id` returns the stored device record.
//!
//! Expected to fail (red) until the 1b implementation phase lands.

use kupferdraht::storage::db_key::DbKey;
use kupferdraht::storage::{DeviceRecord, SqliteStore, Store};
use tempfile::TempDir;

/// Fixed key material — whole-DB encryption means a store opens **ready**
/// (keyed) at construction; there is no separate unlock step.
fn test_key() -> DbKey {
    DbKey::from_keystore_bytes(&[0x42; 32]).expect("32-byte test material")
}

fn fresh_store() -> (TempDir, SqliteStore) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("kupferdraht.db");
    let store = SqliteStore::open(&path, &test_key()).expect("open");
    (dir, store)
}

fn sample_device() -> DeviceRecord {
    DeviceRecord {
        user_id: "user-uuid".to_string(),
        client_id: "new-client-id".to_string(),
        mls_public_key: None,
    }
}

#[tokio::test]
async fn save_and_load_device_round_trip() {
    let (_dir, store) = fresh_store();
    let device = sample_device();

    store.save_device(&device).await.expect("save");

    let loaded = store
        .load_device(&device.user_id, &device.client_id)
        .await
        .expect("load");

    assert_eq!(loaded, Some(device));
}

#[tokio::test]
async fn load_device_returns_none_for_unknown_user() {
    let (_dir, store) = fresh_store();

    let loaded = store
        .load_device("not-in-db", "irrelevant-client")
        .await
        .expect("load");

    assert!(loaded.is_none());
}

#[tokio::test]
async fn save_device_is_upsert() {
    let (_dir, store) = fresh_store();
    let mut device = sample_device();
    store.save_device(&device).await.expect("first save");

    // Simulate Step 1c filling in the MLS public key post-registration.
    device.mls_public_key = Some("base64-encoded-mls-key".to_string());
    store.save_device(&device).await.expect("second save");

    let loaded = store
        .load_device(&device.user_id, &device.client_id)
        .await
        .expect("load");
    assert_eq!(loaded, Some(device));
}
