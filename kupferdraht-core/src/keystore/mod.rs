// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Platform-keystore abstraction — the [`SecureKeyStore`] secret store.
//!
//! `SecureKeyStore` is the thin trait Kupferdraht reaches for whenever it
//! needs to persist or read a database key's material — a raw 32-byte
//! key (`storage::db_key::DbKey`, ADR-0021). The material is stored
//! **directly** under an id — no KEK, no on-disk wrapped envelope. The Kupferdraht
//! namespace for the id comes from the keystore *instance* (see
//! [`key_id`]), not the id string itself.
//!
//! Implementations:
//!
//! - [`KeyringStore`] — desktop default backed by the `keyring` crate
//!   (macOS Keychain, Windows Credential Manager; on Linux only the
//!   kernel keyring, lost on reboot — open.md §71 B4). Exercised at
//!   smoke-test time on the maintainer's machine.
//! - [`InMemoryKeyStore`] — process-scoped, used by `cargo test`. CI
//!   doesn't have a desktop session, so a keyring-rs-backed
//!   `cargo test` would be flaky / impossible.
//! - [`FileKeyStore`] — file-backed dev fallback, activated by the
//!   `KUPFERDRAHT_DEV_KEYSTORE` environment variable.
//! - Mobile (iOS Keychain, Android Keystore) and Web (`SubtleCrypto`)
//!   supply a `UniFFI` callback variant implementing the same trait.

mod file_store;
mod in_memory;
pub(crate) mod key_id;
// The desktop OS keyring has no wasm backend; native-only.
#[cfg(not(target_arch = "wasm32"))]
mod keyring_store;
// The browser WebCrypto+IndexedDB keystore (Step 45 / W2) — wasm-only.
#[cfg(target_arch = "wasm32")]
mod indexeddb_keystore;
#[cfg(any(target_arch = "wasm32", test))]
pub(crate) mod pending_writes;

pub use file_store::FileKeyStore;
pub use in_memory::InMemoryKeyStore;
#[cfg(target_arch = "wasm32")]
pub use indexeddb_keystore::IndexedDbKeyStore;
#[cfg(not(target_arch = "wasm32"))]
pub use keyring_store::KeyringStore;

use std::sync::Arc;

use zeroize::Zeroize as _;

use crate::error::KupferdrahtError;
use crate::storage::db_key::{DbKey, generate_db_key};

/// Platform-keystore-backed secret storage.
///
/// All methods key on an opaque `id` (see [`key_id`]): Kupferdraht stores
/// the device-level `global.db` key and one key per account, each
/// under its own id.
///
/// Implementations must be `Send + Sync` so a `Client` can hold one
/// behind an `Arc` and reach for it from any task.
///
/// **`UniFFI` shape note.** The trait is exported as a
/// `#[uniffi::export(with_foreign)]` callback interface so foreign
/// (Kotlin / Swift) code can supply the platform keystore. That shapes
/// two signature choices: `id` is an owned `String` (not `&str` —
/// `UniFFI` marshals callback-method arguments by value), and the
/// secret is a `Vec<u8>` / `Option<Vec<u8>>` rather than `[u8; 32]`
/// (`UniFFI` does not marshal fixed-size byte arrays as callback types).
/// Kupferdraht validates the 32-byte length at the call boundary
/// ([`DbKey::from_keystore_bytes`]); a foreign implementation returning
/// any other number of bytes — the pre-Step-90 48-byte blob included —
/// surfaces as `KupferdrahtError::Generic`, never a panic.
#[uniffi::export(with_foreign)]
pub trait SecureKeyStore: Send + Sync {
    /// The stored secret for `id`, or `None` if nothing is stored.
    ///
    /// **Contract (ADR-0029):** `None` means the id was never stored or
    /// was deleted. A store that is locked or not available yet (the iOS
    /// Keychain before the first unlock, a backing service that is not
    /// up) must return `Err` — Kupferdraht then fails and a later attempt
    /// succeeds. Answering `None` instead makes the open fail with
    /// `KeyUnavailable`; Kupferdraht never replaces a key of an existing
    /// database.
    ///
    /// # Errors
    /// `KupferdrahtError::Generic` for any keystore-side failure, a locked or
    /// unavailable store included.
    fn get_secret(&self, id: String) -> Result<Option<Vec<u8>>, KupferdrahtError>;

    /// Store (overwriting any prior value) the secret for `id`.
    ///
    /// # Errors
    /// `KupferdrahtError::Generic` for any keystore-side failure.
    fn set_secret(&self, id: String, secret: Vec<u8>) -> Result<(), KupferdrahtError>;

    /// Remove the secret for `id`. Idempotent: deleting a missing
    /// entry is `Ok(())`. This is the wipe primitive — dropping the
    /// key renders the matching database permanently unreadable
    /// (`kupferdraht-cli logout --purge-device`, backup-restore).
    ///
    /// # Errors
    /// `KupferdrahtError::Generic` for any keystore-side failure.
    fn delete_secret(&self, id: String) -> Result<(), KupferdrahtError>;
}

/// Await every keystore write still in flight — the browser keystore
/// persists in the background — and fail with the first error. Native
/// keystores write synchronously: nothing is pending there. Step 104.
///
/// # Errors
/// The first failed background write (`IndexedDB` / `WebCrypto`), or a
/// write that never reported.
pub(crate) async fn flush_pending_writes() -> Result<(), KupferdrahtError> {
    #[cfg(any(target_arch = "wasm32", test))]
    {
        pending_writes::flush().await
    }
    #[cfg(not(any(target_arch = "wasm32", test)))]
    {
        Ok(())
    }
}

/// Read the per-database key material stored under `id`, or — on first
/// call — generate it, store it, and return it. The material is a raw
/// 32-byte key; [`DbKey::from_keystore_bytes`] is the single place the
/// length invariant is enforced.
///
/// # Errors
/// `KupferdrahtError::Generic` if the keystore fails or returns a secret that
/// is not exactly 32 bytes.
pub(crate) fn load_or_create_key(
    store: &Arc<dyn SecureKeyStore>,
    id: &str,
) -> Result<DbKey, KupferdrahtError> {
    if let Some(key) = load_key(store, id)? {
        return Ok(key);
    }
    let key = generate_db_key();
    store.set_secret(id.to_owned(), key.to_keystore_bytes().to_vec())?;
    Ok(key)
}

/// Read the key material stored under `id`, or `None` when nothing is
/// stored — the read half of [`load_or_create_key`]. Step 85's locked
/// creation path (`storage::load_or_create_key_locked`) uses it for the
/// lock-free fast path: an existing key must never contend.
///
/// # Errors
/// `KupferdrahtError::Generic` if the keystore fails or returns a secret that
/// is not exactly 32 bytes.
pub(crate) fn load_key(
    store: &Arc<dyn SecureKeyStore>,
    id: &str,
) -> Result<Option<DbKey>, KupferdrahtError> {
    let Some(mut raw) = store.get_secret(id.to_owned())? else {
        return Ok(None);
    };
    // Parse copies the bytes into the (zeroizing) `DbKey`; wipe the heap
    // `raw` afterwards so a panic mid-flight leaves nothing recoverable.
    let key = DbKey::from_keystore_bytes(&raw);
    raw.zeroize();
    key.map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_or_create_generates_then_reads_back_the_same_key() {
        let store: Arc<dyn SecureKeyStore> = Arc::new(InMemoryKeyStore::new());
        let first = load_or_create_key(&store, "account-alice").expect("first");
        let second = load_or_create_key(&store, "account-alice").expect("second");
        assert_eq!(first.key(), second.key(), "second call returns same key");
    }

    #[test]
    fn load_or_create_rejects_a_wrong_length_secret() {
        // A buggy foreign keystore returning the wrong number of bytes
        // must surface as a KupferdrahtError, not a panic.
        let store: Arc<dyn SecureKeyStore> = Arc::new(InMemoryKeyStore::new());
        store
            .set_secret("account-alice".to_owned(), vec![0u8; 16])
            .expect("set");
        let result = load_or_create_key(&store, "account-alice");
        assert!(
            matches!(result, Err(KupferdrahtError::Generic(_))),
            "16-byte secret must reject"
        );
    }
}
