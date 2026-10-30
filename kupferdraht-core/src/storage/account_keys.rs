// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Where a new key of an account comes from (ADR-0035).
//!
//! Keys are stored and loaded exactly as natively; only the creation of a
//! missing key of an account differs by home. A `Random` home (native, the
//! default) draws it at random. A `Seed` home (the browser) derives it from
//! the account's seed — a keystore entry of its own — with HKDF-SHA-256 and a
//! frozen label, so the same seed always yields the same keys. Device-level
//! keys (`global.db`) are random in every home.

use std::sync::Arc;

use hkdf::Hkdf;
#[cfg(any(target_arch = "wasm32", test))]
use rand::RngCore;
#[cfg(any(target_arch = "wasm32", test))]
use rand::rngs::OsRng;
use sha2::Sha256;
use zeroize::{Zeroize, Zeroizing};

use crate::error::KupferdrahtError;
use crate::keystore::SecureKeyStore;
use crate::keystore::key_id::{self, KeyId, KeyOwner};
use crate::storage::KupferdrahtHome;
use crate::storage::db_key::{DbKey, KEY_LEN};

/// The length of an account's seed in bytes.
pub(crate) const SEED_LEN: usize = 32;

/// Where a new key of an account comes from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum AccountKeySource {
    /// Drawn at random — native.
    #[default]
    Random,
    /// Derived from the account's seed — the browser (ADR-0035).
    Seed,
}

/// What an account key opens; each purpose has its own frozen label.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AccountKeyPurpose {
    /// `kupferdraht.db` — `kupferdraht/v1/db-key/kupferdraht`.
    Store,
    /// `mls.db` — `kupferdraht/v1/db-key/mls`.
    Mls,
}

impl AccountKeyPurpose {
    /// The HKDF `info` of this purpose. Frozen once shipped: a changed
    /// label orphans every database keyed with the old one.
    fn label(self) -> &'static [u8] {
        match self {
            Self::Store => b"kupferdraht/v1/db-key/kupferdraht",
            Self::Mls => b"kupferdraht/v1/db-key/mls",
        }
    }
}

/// Derive the key of `purpose` from an account's seed: HKDF-SHA-256, no
/// salt (the seed is uniformly random), the purpose's label as `info`.
pub(crate) fn derive_account_key(seed: &[u8; SEED_LEN], purpose: AccountKeyPurpose) -> DbKey {
    DbKey::from(derive_from_seed(seed, purpose.label()))
}

/// 32 bytes HKDF-SHA-256 derives from an account's seed for `label`: no
/// salt (the seed is uniformly random), the label as `info`.
pub(crate) fn derive_from_seed(seed: &[u8; SEED_LEN], label: &[u8]) -> Zeroizing<[u8; KEY_LEN]> {
    let mut key = Zeroizing::new([0u8; KEY_LEN]);
    let expanded = Hkdf::<Sha256>::new(None, seed).expand(label, key.as_mut_slice());
    assert!(expanded.is_ok(), "HKDF-SHA-256 yields 32 bytes");
    key
}

/// A fresh random seed from the OS CSPRNG.
#[cfg(any(target_arch = "wasm32", test))]
pub(crate) fn generate_seed() -> Zeroizing<[u8; SEED_LEN]> {
    let mut seed = Zeroizing::new([0u8; SEED_LEN]);
    OsRng.fill_bytes(seed.as_mut_slice());
    seed
}

/// The seed of `account_id` in the home `home_id`, if the keystore holds it.
///
/// # Errors
/// Keystore errors; `KupferdrahtError::Storage` for a stored seed of the wrong
/// length (runtime data, never a panic).
pub(crate) fn load_seed(
    keystore: &Arc<dyn SecureKeyStore>,
    home_id: &str,
    account_id: &str,
) -> Result<Option<Zeroizing<[u8; SEED_LEN]>>, KupferdrahtError> {
    let Some(mut raw) = keystore.get_secret(key_id::account_seed(home_id, account_id))? else {
        return Ok(None);
    };
    if raw.len() != SEED_LEN {
        let len = raw.len();
        raw.zeroize();
        return Err(KupferdrahtError::Storage(format!(
            "the seed of {account_id} has {len} bytes, not {SEED_LEN}"
        )));
    }
    let mut seed = Zeroizing::new([0u8; SEED_LEN]);
    seed.copy_from_slice(&raw);
    raw.zeroize();
    Ok(Some(seed))
}

/// Whether the keystore holds the seed of `account_id` in the home `home_id`.
///
/// # Errors
/// As [`load_seed`].
pub(crate) fn has_seed(
    keystore: &Arc<dyn SecureKeyStore>,
    home_id: &str,
    account_id: &str,
) -> Result<bool, KupferdrahtError> {
    Ok(load_seed(keystore, home_id, account_id)?.is_some())
}

/// A missing key of an account in a `Seed` home: derived from the
/// account's seed and stored under its native id. `None` for a device key
/// or a `Random` home — the native path makes those.
///
/// # Errors
/// `KupferdrahtError::RecoveryDataRequired` when the home holds no seed for the
/// account (nothing is written); keystore errors.
pub(crate) fn derive_missing_account_key(
    home: &KupferdrahtHome,
    keystore: &Arc<dyn SecureKeyStore>,
    key: &KeyId,
) -> Result<Option<DbKey>, KupferdrahtError> {
    let KeyOwner::Account {
        account_id,
        purpose,
    } = key.owner()
    else {
        return Ok(None);
    };
    if home.account_keys() != AccountKeySource::Seed {
        return Ok(None);
    }
    let Some(seed) = load_seed(keystore, home.home_id(), account_id)? else {
        return Err(KupferdrahtError::RecoveryDataRequired(account_id.clone()));
    };
    let derived = derive_account_key(&seed, *purpose);
    keystore.set_secret(
        key.namespaced().to_owned(),
        derived.to_keystore_bytes().to_vec(),
    )?;
    debug_assert!(
        keystore.get_secret(key.namespaced().to_owned())?.is_some(),
        "a derived key is stored under its native id"
    );
    Ok(Some(derived))
}

/// Forget the seed, the database keys and the storage-login key of
/// `account_id` in the home `home_id` — a new seed must not meet a login
/// key of the old one. Deleting a missing entry is not an error.
///
/// # Errors
/// Keystore errors.
#[cfg(any(target_arch = "wasm32", test))]
pub(crate) fn forget_account_secrets(
    keystore: &Arc<dyn SecureKeyStore>,
    home_id: &str,
    account_id: &str,
) -> Result<(), KupferdrahtError> {
    keystore.delete_secret(key_id::account_seed(home_id, account_id))?;
    keystore.delete_secret(
        key_id::account_db(home_id, account_id)
            .namespaced()
            .to_owned(),
    )?;
    keystore.delete_secret(
        key_id::account_mls(home_id, account_id)
            .namespaced()
            .to_owned(),
    )?;
    keystore.delete_secret(key_id::account_storage_login(home_id, account_id))?;
    debug_assert!(
        !has_seed(keystore, home_id, account_id)?,
        "the seed is gone once forgotten"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Seed `00 01 … 1f`.
    fn test_seed() -> [u8; SEED_LEN] {
        let mut seed = [0u8; SEED_LEN];
        for (i, byte) in seed.iter_mut().enumerate() {
            *byte = u8::try_from(i).expect("index fits a byte");
        }
        seed
    }

    /// HKDF-SHA-256(salt = none, ikm = seed, info = `kupferdraht/v1/db-key/kupferdraht`),
    /// 32 bytes — computed independently of this crate.
    #[test]
    fn derives_the_kupferdraht_db_key_from_the_seed() {
        let key = derive_account_key(&test_seed(), AccountKeyPurpose::Store);
        assert_eq!(
            key.key(),
            &[
                0x10, 0x66, 0xa8, 0xcc, 0x30, 0xb1, 0x55, 0xdf, 0x7e, 0x01, 0xda, 0x5e, 0x9d, 0xef,
                0x3b, 0xa8, 0xa0, 0x0d, 0x2f, 0xe0, 0xbb, 0xc2, 0x24, 0x7d, 0x6a, 0x27, 0x03, 0x00,
                0x68, 0x61, 0xa6, 0x09,
            ]
        );
    }

    /// Same, info `kupferdraht/v1/db-key/mls`.
    #[test]
    fn derives_the_mls_db_key_from_the_seed() {
        let key = derive_account_key(&test_seed(), AccountKeyPurpose::Mls);
        assert_eq!(
            key.key(),
            &[
                0xa9, 0xef, 0x47, 0xab, 0x47, 0x65, 0xb9, 0xd9, 0x39, 0x35, 0x29, 0x95, 0x16, 0x3b,
                0x02, 0x8e, 0xe6, 0xc1, 0x55, 0x03, 0x15, 0x7e, 0x98, 0xfb, 0x50, 0xf4, 0x14, 0x66,
                0x18, 0x47, 0xc0, 0x70,
            ]
        );
    }
}
