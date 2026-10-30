// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Whole-DB encryption at rest (engine-swap sub-step 2a; engine re-swapped
//! to SQLite3MC / ChaCha20-Poly1305 in Step 90, ADR-0021).
//!
//! The store encrypts the *entire* `kupferdraht.db` at rest via `PRAGMA key`,
//! applied at `open(path, key)`. The concrete store owns its encryption
//! and is opened **ready** — there is no separate unlock step (the trait
//! is pure CRUD). These tests pin: the page content is encrypted at rest
//! (while the SQLite header stays plaintext for iOS WAL identification),
//! the correct key round-trips the data across a reopen, and the wrong
//! key cannot read it. The salt-isolation pin of the SQLCipher era is
//! retired with the salt: under chacha20 with a raw key there is no salt
//! in the trust path (the 256-bit key is the whole security parameter).
//! Byte-level header/ciphertext pins: `tests/step_90_sqlite3mc_engine.rs`.

use kupferdraht::storage::db_key::DbKey;
use kupferdraht::storage::{SessionRecord, SqliteStore, Store};
use tempfile::TempDir;

/// Build a [`DbKey`] from a fill byte (raw-key mode — the high-entropy
/// 32-byte key goes straight into `PRAGMA key`, no KDF).
fn db_key(byte: u8) -> DbKey {
    DbKey::from_keystore_bytes(&[byte; 32]).expect("32-byte raw key")
}

/// Two distinct keys.
fn key_a() -> DbKey {
    db_key(0x11)
}
fn key_b() -> DbKey {
    db_key(0x22)
}

fn sample_session() -> SessionRecord {
    SessionRecord {
        user_id: "alice@example.test".into(),
        device_id: "dev-1".into(),
        access_token: "real.access.jwt.token".into(),
        refresh_token: "real-refresh-cookie-payload".into(),
        cookie_label: Some("kupferdraht-cli".into()),
        api_version: 14,
        access_token_issued_at_ms: 1_700_000_000_000,
    }
}

/// The SQLite header stays plaintext (so iOS can identify a WAL-mode DB in
/// a shared container) while the page content is encrypted at rest.
#[tokio::test]
async fn header_is_plaintext_but_content_is_encrypted() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("kupferdraht.db");

    let store = SqliteStore::open(&path, &key_a()).expect("open ready");
    store
        .save_session(&sample_session())
        .await
        .expect("save session");
    drop(store);

    let bytes = std::fs::read(&path).expect("read kupferdraht.db");
    assert!(
        bytes.starts_with(b"SQLite format 3\0"),
        "header must be plaintext so iOS can identify the WAL DB"
    );
    // The access token we wrote must not appear in the raw file — content
    // beyond the 32-byte plaintext header stays encrypted.
    let marker = b"real.access.jwt.token";
    assert!(
        !bytes.windows(marker.len()).any(|w| w == marker),
        "page content must be encrypted at rest — found a plaintext marker"
    );
}

/// The correct key round-trips the data across a reopen.
#[tokio::test]
async fn right_key_reads_data_back() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("kupferdraht.db");

    {
        let store = SqliteStore::open(&path, &key_a()).expect("open");
        store.save_session(&sample_session()).await.expect("save");
    }

    let store = SqliteStore::open(&path, &key_a()).expect("reopen with the same key");
    let loaded = store
        .load_session("alice@example.test")
        .await
        .expect("load")
        .expect("session row present");
    assert_eq!(loaded.user_id, "alice@example.test");
    assert_eq!(loaded.access_token, "real.access.jwt.token");
}

/// The wrong key cannot read the data — opening the encrypted file under
/// a different key must not surface the stored session.
#[tokio::test]
async fn wrong_key_cannot_read_data() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("kupferdraht.db");

    {
        let store = SqliteStore::open(&path, &key_a()).expect("open");
        store.save_session(&sample_session()).await.expect("save");
    }

    // Opening under the wrong key fails outright (the key is verified by
    // a first read at open, Step 90); should that ever regress to a
    // later failure, the session must still never be returned in clear.
    let leaked = match SqliteStore::open(&path, &key_b()) {
        Err(_) => false,
        Ok(store) => store
            .load_session("alice@example.test")
            .await
            .ok()
            .flatten()
            .is_some(),
    };
    assert!(
        !leaked,
        "the wrong key must never reveal the stored session"
    );
}
