// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! The storage-login key (Step 111b, ADR-0035): an account key like the
//! database keys — stored under `kupferdraht.<home-id>.account-<user>-storage-login`,
//! in the browser derived from the account's seed
//! (`kupferdraht/v1/storage-login`). An Ed25519 signing key: the storage server
//! derives the subject that owns the account's databases from its public
//! key, so the same seed finds the same databases in any browser. The VFS
//! only ever gets a signer over it.

use std::sync::Arc;

use zeroize::Zeroize;

use crate::error::KupferdrahtError;
use crate::keystore::{SecureKeyStore, key_id};
use crate::storage::KupferdrahtHome;
use crate::storage::account_keys::{SEED_LEN, derive_from_seed, load_seed};

/// The frozen HKDF label of the storage-login key (ADR-0035).
const STORAGE_LOGIN_LABEL: &[u8] = b"kupferdraht/v1/storage-login";

/// An account's storage-login key.
pub(crate) struct StorageLoginKey {
    signing: ed25519_dalek::SigningKey,
}

impl StorageLoginKey {
    /// The key a seed yields.
    #[cfg(test)]
    fn from_seed(seed: &[u8; SEED_LEN]) -> Self {
        Self::from_secret(&login_secret(seed))
    }

    fn from_secret(secret: &[u8; 32]) -> Self {
        Self {
            signing: ed25519_dalek::SigningKey::from_bytes(secret),
        }
    }

    /// The Ed25519 public key (32 bytes).
    pub(crate) fn public_key(&self) -> [u8; 32] {
        self.signing.verifying_key().to_bytes()
    }

    /// Sign `message` (a login transcript of the storage server).
    pub(crate) fn sign(&self, message: &[u8]) -> [u8; 64] {
        use ed25519_dalek::Signer as _;
        self.signing.sign(message).to_bytes()
    }
}

/// The Ed25519 secret a seed yields: HKDF-SHA-256 with
/// `kupferdraht/v1/storage-login`.
fn login_secret(seed: &[u8; SEED_LEN]) -> zeroize::Zeroizing<[u8; 32]> {
    derive_from_seed(seed, STORAGE_LOGIN_LABEL)
}

/// The storage-login key of `account_id` in `home`: loaded from the
/// keystore, or derived from the account's seed and stored — it can always
/// be derived again, so a lost write strands nothing.
///
/// # Errors
/// `RecoveryDataRequired` without the key and without the seed;
/// `KupferdrahtError::Storage` for a stored key of the wrong length; keystore
/// errors.
pub(crate) fn account_storage_login_key(
    home: &KupferdrahtHome,
    keystore: &Arc<dyn SecureKeyStore>,
    account_id: &str,
) -> Result<StorageLoginKey, KupferdrahtError> {
    let id = key_id::account_storage_login(home.home_id(), account_id);
    if let Some(mut stored) = keystore.get_secret(id.clone())? {
        let secret: Result<[u8; 32], _> = stored.as_slice().try_into();
        stored.zeroize();
        let mut secret = secret.map_err(|_| {
            KupferdrahtError::Storage(format!(
                "the storage-login key of {account_id} is not 32 bytes"
            ))
        })?;
        let key = StorageLoginKey::from_secret(&secret);
        secret.zeroize();
        return Ok(key);
    }
    let Some(seed) = load_seed(keystore, home.home_id(), account_id)? else {
        return Err(KupferdrahtError::RecoveryDataRequired(
            account_id.to_owned(),
        ));
    };
    let secret = login_secret(&seed);
    keystore.set_secret(id, secret.to_vec())?;
    Ok(StorageLoginKey::from_secret(&secret))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keystore::{InMemoryKeyStore, key_id};
    use crate::storage::account_keys::AccountKeySource;

    const ACCOUNT: &str = "user-uuid@example.test";

    fn test_seed() -> [u8; SEED_LEN] {
        let mut seed = [0u8; SEED_LEN];
        for (i, byte) in seed.iter_mut().enumerate() {
            *byte = u8::try_from(i).expect("index fits a byte");
        }
        seed
    }

    fn seed_home(dir: &tempfile::TempDir) -> KupferdrahtHome {
        KupferdrahtHome::open(dir.path().to_path_buf())
            .expect("home")
            .with_account_keys(AccountKeySource::Seed)
    }

    /// HKDF-SHA-256(seed `00 … 1f`, info `kupferdraht/v1/storage-login`) is the
    /// Ed25519 secret — computed independently of this crate.
    #[test]
    fn the_storage_login_key_derives_from_the_seed() {
        let key = StorageLoginKey::from_seed(&test_seed());
        let secret: [u8; 32] = [
            0xee, 0x47, 0x7f, 0x58, 0x6e, 0xd4, 0x14, 0xe9, 0x44, 0x92, 0x3c, 0xb7, 0xde, 0xc3,
            0xd5, 0x1d, 0x1e, 0x34, 0x0b, 0xef, 0x26, 0x30, 0x2f, 0xcd, 0xed, 0x8a, 0x92, 0xc3,
            0xca, 0x80, 0xd4, 0xc9,
        ];
        let expected = ed25519_dalek::SigningKey::from_bytes(&secret)
            .verifying_key()
            .to_bytes();
        assert_eq!(key.public_key(), expected);
    }

    /// The storage server verifies the login signature with the public key.
    #[test]
    fn its_signature_verifies_with_its_public_key() {
        let key = StorageLoginKey::from_seed(&test_seed());
        let signature = key.sign(b"login transcript");
        let verifying =
            ed25519_dalek::VerifyingKey::from_bytes(&key.public_key()).expect("a valid public key");
        verifying
            .verify_strict(
                b"login transcript",
                &ed25519_dalek::Signature::from_bytes(&signature),
            )
            .expect("the signature verifies");
    }

    #[test]
    fn the_key_id_sits_beside_the_accounts_other_keys() {
        assert_eq!(
            key_id::account_storage_login("0123456789abcdef0123456789abcdef", ACCOUNT),
            format!("kupferdraht.0123456789abcdef0123456789abcdef.account-{ACCOUNT}-storage-login")
        );
    }

    /// Missing in the keystore: derived from the seed and stored under its
    /// id, as a database key would be.
    #[test]
    fn a_missing_key_is_derived_and_stored() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let home = seed_home(&dir);
        let keystore: Arc<dyn SecureKeyStore> = Arc::new(InMemoryKeyStore::new());
        keystore
            .set_secret(
                key_id::account_seed(home.home_id(), ACCOUNT),
                test_seed().to_vec(),
            )
            .expect("seed");

        let key = account_storage_login_key(&home, &keystore, ACCOUNT).expect("derived");

        assert_eq!(
            key.public_key(),
            StorageLoginKey::from_seed(&test_seed()).public_key()
        );
        assert!(
            keystore
                .get_secret(key_id::account_storage_login(home.home_id(), ACCOUNT))
                .expect("read")
                .is_some(),
            "stored under its id"
        );
    }

    /// A first setup forgets the account's secrets before its new seed:
    /// the old login key must go with them, or the new seed's databases
    /// would land under the old key's subject.
    #[test]
    fn forgetting_the_account_forgets_its_storage_login_key() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let home = seed_home(&dir);
        let keystore: Arc<dyn SecureKeyStore> = Arc::new(InMemoryKeyStore::new());
        keystore
            .set_secret(
                key_id::account_seed(home.home_id(), ACCOUNT),
                test_seed().to_vec(),
            )
            .expect("seed");
        account_storage_login_key(&home, &keystore, ACCOUNT).expect("derived");

        crate::storage::account_keys::forget_account_secrets(&keystore, home.home_id(), ACCOUNT)
            .expect("forgotten");

        assert!(
            keystore
                .get_secret(key_id::account_storage_login(home.home_id(), ACCOUNT))
                .expect("read")
                .is_none(),
            "the storage-login key is gone"
        );
    }

    #[test]
    fn without_key_and_seed_it_is_refused() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let home = seed_home(&dir);
        let keystore: Arc<dyn SecureKeyStore> = Arc::new(InMemoryKeyStore::new());

        let result = account_storage_login_key(&home, &keystore, ACCOUNT);

        assert!(
            matches!(result, Err(KupferdrahtError::RecoveryDataRequired(_))),
            "{:?}",
            result.err()
        );
    }
}
