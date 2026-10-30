// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 1a — Storage foundations.
//!
//! Failing tests per PLAN.md Step 1 "TDD starter tests, 1a":
//! * `Store::save_session` + `Store::load_session` round-trip.
//! * `storage::schema::migrations` run idempotently on an existing DB.
//!
//! Expected to fail (red) until the 1a implementation phase lands.

use kupferdraht::storage::db_key::DbKey;
use kupferdraht::storage::{SessionRecord, SqliteStore, Store};
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

fn sample_session() -> SessionRecord {
    SessionRecord {
        user_id: "user-uuid".into(),
        device_id: "client-id-7e".into(),
        access_token: "access.jwt.value".into(),
        refresh_token: "refresh-cookie-value".into(),
        cookie_label: Some("zuid".into()),
        api_version: 14,
        access_token_issued_at_ms: 1_700_000_000_000,
    }
}

#[tokio::test]
async fn save_and_load_session_round_trip() {
    let (_dir, store) = fresh_store();
    let session = sample_session();

    store.save_session(&session).await.expect("save");

    let loaded = store.load_session(&session.user_id).await.expect("load");

    assert_eq!(loaded, Some(session));
}

#[tokio::test]
async fn load_session_returns_none_for_unknown_user() {
    let (_dir, store) = fresh_store();

    let loaded = store.load_session("not-in-db").await.expect("load");

    assert!(loaded.is_none());
}

#[tokio::test]
async fn migrations_are_idempotent_across_reopens() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("kupferdraht.db");

    // First open: creates the schema from V1 under the key.
    SqliteStore::open(&path, &test_key()).expect("first open");
    // Second open: migrations already applied — must be a no-op, not error.
    SqliteStore::open(&path, &test_key()).expect("second open");

    // Third open + a real round-trip proves the schema is healthy.
    let store = SqliteStore::open(&path, &test_key()).expect("third open");
    let session = sample_session();
    store
        .save_session(&session)
        .await
        .expect("save after reopen");
    let loaded = store
        .load_session(&session.user_id)
        .await
        .expect("load after reopen");
    assert_eq!(loaded, Some(session));
}
