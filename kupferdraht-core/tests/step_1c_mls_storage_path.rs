// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 1c.6 — `ClientBuilder::base_dir` MLS wiring.
//!
//! When the caller configures `.base_dir(dir)`, `login` must create (as
//! needed) `dir/accounts/<id>/mls.db` and back the internal `MlsClient`
//! with a file-backed openmls storage so the signature keypair and
//! `KeyPackage` private halves survive the process.

use httpmock::MockServer;
use kupferdraht::ClientBuilder;

mod common;

#[tokio::test]
async fn login_with_storage_path_creates_mls_db() {
    let dir = tempfile::tempdir().expect("tempdir");
    let server = MockServer::start();
    common::mount_full_login_flow(&server);

    let client = ClientBuilder::new()
        .server(common::server_config(&server.base_url()))
        .permit_http()
        .base_dir(dir.path().to_path_buf())
        .login("alice@example.test".into(), "pw".into(), None)
        .await
        .expect("login with base_dir")
        .register_device()
        .await
        .expect("register device");

    let mls_db = dir
        .path()
        .join("accounts")
        .join(client.user_id().expect("user_id").as_str())
        .join("mls.db");
    assert!(mls_db.exists(), "login must create {}", mls_db.display());
}

#[tokio::test]
async fn login_with_storage_path_exposes_signature_public_key() {
    let dir = tempfile::tempdir().expect("tempdir");
    let server = MockServer::start();
    common::mount_full_login_flow(&server);

    let client = ClientBuilder::new()
        .server(common::server_config(&server.base_url()))
        .permit_http()
        .base_dir(dir.path().to_path_buf())
        .login("alice@example.test".into(), "pw".into(), None)
        .await
        .expect("login")
        .register_device()
        .await
        .expect("register device");

    let pk = client.mls_signature_public_key();
    // ECDSA-P256 uncompressed: 0x04 || X(32) || Y(32) = 65 bytes.
    assert_eq!(pk.len(), 65, "P-256 uncompressed public key is 65 bytes");
    assert_eq!(
        pk[0], 0x04,
        "first byte of uncompressed P-256 point is 0x04"
    );
}

#[tokio::test]
async fn login_persists_signature_keypair_to_encrypted_mls_db() {
    use kupferdraht::keystore::{InMemoryKeyStore, SecureKeyStore};
    use rusqlite::Connection;
    use std::sync::Arc;

    let dir = tempfile::tempdir().expect("tempdir");
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    let keystore: Arc<dyn SecureKeyStore> = Arc::new(InMemoryKeyStore::new());

    let client = ClientBuilder::new()
        .server(common::server_config(&server.base_url()))
        .permit_http()
        .base_dir(dir.path().to_path_buf())
        .keystore(keystore.clone())
        .login("alice@example.test".into(), "pw".into(), None)
        .await
        .expect("login")
        .register_device()
        .await
        .expect("register device");

    // mls.db is encrypted at rest under its **own** key, stored directly
    // under `kupferdraht.<home-id>.account-<id>-mls` (separate from the
    // kupferdraht.db key, ADR-0021; home-scoped since Step 93, ADR-0025).
    // Recover that key from the keystore, reopen the file with it, and
    // assert openmls persisted the signature keypair — proving durability,
    // the keying, and that mls.db is keyed independently of kupferdraht.db.
    let account_id = client.user_id().expect("user_id").as_str().to_string();
    let home_id = std::fs::read_to_string(dir.path().join("home.id"))
        .expect("home.id")
        .trim()
        .to_owned();
    let key = keystore
        .get_secret(format!("kupferdraht.{home_id}.account-{account_id}-mls"))
        .expect("get_secret")
        .expect("mls key present in keystore");
    assert_eq!(
        key.len(),
        32,
        "mls.db key material is a raw 32-byte key (ADR-0021)"
    );
    let key_hex: String = key.iter().map(|b| format!("{b:02x}")).collect();

    let mls_db = dir.path().join("accounts").join(&account_id).join("mls.db");
    let raw = Connection::open(&mls_db).expect("reopen mls.db");
    // SQLite3MC pragma order (ADR-0021): cipher scheme and header size
    // BEFORE the key — the production batch's mirror (`apply_whole_db_key`).
    raw.execute_batch(&format!(
        "PRAGMA cipher = 'chacha20'; \
         PRAGMA plaintext_header_size = 32; \
         PRAGMA key = \"x'{key_hex}'\";"
    ))
    .expect("apply mls.db cipher config");
    let count: i64 = raw
        .query_row("SELECT COUNT(*) FROM openmls_signature_keys", [], |row| {
            row.get(0)
        })
        .expect("count signature keys");
    assert!(
        count >= 1,
        "expected at least one persisted signature keypair, got {count}"
    );
}
