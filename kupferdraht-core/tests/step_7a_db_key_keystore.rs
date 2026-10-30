// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 7a: per-database key generation + the platform
//! `SecureKeyStore` secret interface (key material re-shaped by Step 90,
//! ADR-0021: a raw 32-byte key, no salt).
//!
//! 7a is local-only: no Wire round-trip, no openmls. The tests here
//! exercise two public surfaces:
//!
//! - `kupferdraht::storage::db_key::{DbKey, generate_db_key}` — random
//!   key-material generation (a 32-byte raw key; each database file gets
//!   its own) and the keystore-blob boundary: `from_keystore_bytes` is
//!   the single place the material-length invariant is enforced — exactly
//!   32 bytes; the pre-Step-90 48-byte `key ‖ salt` blob is rejected like
//!   any other length (plan input I3, decided 2026-09-24: hard 32).
//!
//! - `kupferdraht::keystore::{SecureKeyStore, InMemoryKeyStore}` — the
//!   platform keystore as a plain secret store. The 32-byte database
//!   key is stored **directly** under an id (`get_secret` /
//!   `set_secret` / `delete_secret`) — no KEK, no on-disk wrapped
//!   envelope. The desktop default (`keyring-rs`-backed `KeyringStore`)
//!   is exercised by the smoke run, not by `cargo test`: keyring-rs
//!   needs an active desktop session CI doesn't provide.

use kupferdraht::keystore::{InMemoryKeyStore, SecureKeyStore};
use kupferdraht::storage::db_key::{DbKey, KEY_LEN, MATERIAL_LEN, generate_db_key};

/// The keystore form of a `DbKey` (the raw 32-byte key) as a plain
/// `Vec<u8>`, which is what the `SecureKeyStore` interface round-trips.
fn material(key: &DbKey) -> Vec<u8> {
    key.to_keystore_bytes().to_vec()
}

#[test]
fn generate_db_key_returns_random_32_byte_material() {
    let a = generate_db_key();
    let b = generate_db_key();
    assert_eq!(KEY_LEN, 32, "the raw ChaCha20 key is 32 bytes");
    assert_eq!(
        MATERIAL_LEN, KEY_LEN,
        "the keystore blob is the key alone — no salt"
    );
    assert_eq!(a.key().len(), 32, "key must be 32 bytes");
    assert_eq!(material(&a).len(), 32, "keystore blob must be 32 bytes");
    assert_ne!(
        a.key(),
        b.key(),
        "two consecutive keys must differ — generation must use the OS CSPRNG, \
         not a fixed seed"
    );
}

#[test]
fn from_keystore_bytes_round_trips_32_bytes() {
    let raw: Vec<u8> = (100u8..132).collect();
    let key = DbKey::from_keystore_bytes(&raw).expect("32 bytes are the key");
    assert_eq!(&key.key()[..], &raw[..]);
    assert_eq!(
        material(&key),
        raw,
        "to_keystore_bytes emits the same 32 bytes"
    );
}

/// Every other length is a recoverable error — including the pre-Step-90
/// 48-byte `key ‖ salt` blob (I3: hard 32, no legacy acceptance).
#[test]
fn from_keystore_bytes_rejects_every_other_length() {
    for len in [0usize, 16, 31, 33, 47, 48, 49, 64] {
        let raw = vec![0x42u8; len];
        assert!(
            matches!(
                DbKey::from_keystore_bytes(&raw),
                Err(kupferdraht::KupferdrahtError::Generic(_))
            ),
            "{len} bytes must be rejected as a recoverable error"
        );
    }
}

#[test]
fn keystore_set_then_get_round_trips_the_secret() {
    let ks = InMemoryKeyStore::new();
    let key = generate_db_key();
    ks.set_secret("account-alice@example.com".to_owned(), material(&key))
        .expect("set_secret");

    let got = ks
        .get_secret("account-alice@example.com".to_owned())
        .expect("get_secret")
        .expect("secret is present after set");
    assert_eq!(
        got,
        material(&key),
        "get_secret must return exactly the stored bytes"
    );
}

#[test]
fn keystore_get_absent_id_returns_none() {
    let ks = InMemoryKeyStore::new();
    assert!(
        ks.get_secret("account-nobody@example.com".to_owned())
            .expect("get_secret")
            .is_none(),
        "an id that was never set must read back as None, not an error"
    );
}

#[test]
fn keystore_delete_removes_the_secret_and_is_idempotent() {
    let ks = InMemoryKeyStore::new();
    ks.set_secret(
        "account-alice@example.com".to_owned(),
        material(&generate_db_key()),
    )
    .expect("set_secret");

    ks.delete_secret("account-alice@example.com".to_owned())
        .expect("delete_secret");
    assert!(
        ks.get_secret("account-alice@example.com".to_owned())
            .expect("get_secret")
            .is_none(),
        "delete_secret must drop the secret — a later get reads None"
    );
    ks.delete_secret("account-alice@example.com".to_owned())
        .expect("deleting an absent id is Ok (idempotent)");
}

#[test]
fn keystore_isolates_secrets_per_id() {
    let ks = InMemoryKeyStore::new();
    let alice = generate_db_key();
    let bob = generate_db_key();
    ks.set_secret("account-alice@example.com".to_owned(), material(&alice))
        .expect("set alice");
    ks.set_secret("account-bob@example.com".to_owned(), material(&bob))
        .expect("set bob");

    assert_eq!(
        ks.get_secret("account-alice@example.com".to_owned())
            .unwrap()
            .unwrap(),
        material(&alice)
    );
    assert_eq!(
        ks.get_secret("account-bob@example.com".to_owned())
            .unwrap()
            .unwrap(),
        material(&bob)
    );
    assert_ne!(
        material(&alice),
        material(&bob),
        "distinct accounts must hold distinct keys"
    );
}

#[test]
fn a_second_keystore_instance_does_not_see_anothers_secret() {
    // Models "different machine / wiped keychain": the database key
    // lives only in the keystore that stored it. A fresh keystore
    // (another device, or the OS keychain wiped between sessions)
    // returns None — the encrypted DB is then unrecoverable, exactly
    // the destructive matrix the threat model expects ("On restore
    // from backup or disk image").
    let store_a = InMemoryKeyStore::new();
    store_a
        .set_secret(
            "account-alice@example.com".to_owned(),
            material(&generate_db_key()),
        )
        .expect("set");

    let store_b = InMemoryKeyStore::new();
    assert!(
        store_b
            .get_secret("account-alice@example.com".to_owned())
            .expect("get_secret")
            .is_none(),
        "a foreign keystore instance must not surface another's secret"
    );
}
