// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Sealing the seed (Step 112, ADR-0035): the key is
//! `HKDF-SHA256(ikm = o ‖ Argon2id(pin, salt), info = "kupferdraht/v1/recovery-data")`
//! — `o` the OPRF output only the PIN service's key and the right PIN
//! give; AES-256-GCM seals the 32-byte seed with the recovery data's header
//! as associated data. A failed open is a wrong PIN: the scheme cannot
//! tell it from a changed file, and neither opens.

use aes_gcm::Aes256Gcm;
use aes_gcm::aead::{Aead, KeyInit, Payload};
use hkdf::Hkdf;
use sha2::Sha256;
use zeroize::Zeroizing;

use super::data::{Argon2Params, SEALED_LEN};
use crate::error::KupferdrahtError;

/// The frozen HKDF label of the recovery key (ADR-0035).
pub(crate) const RECOVERY_DATA_LABEL: &[u8] = b"kupferdraht/v1/recovery-data";

/// The Argon2id parameters new recovery data is sealed with: 64 MiB, three
/// passes, one lane.
pub(crate) const ARGON2_PARAMS: Argon2Params = Argon2Params {
    m_kib: 65_536,
    t: 3,
    p: 1,
};

/// The key that seals the seed.
///
/// # Errors
/// `Generic` for Argon2id parameters it refuses.
pub(crate) fn recovery_key(
    oprf_output: &[u8; 32],
    pin: &[u8],
    salt: &[u8; 16],
    params: Argon2Params,
) -> Result<Zeroizing<[u8; 32]>, KupferdrahtError> {
    let argon2_params = argon2::Params::new(params.m_kib, params.t, params.p, Some(32))
        .map_err(|e| KupferdrahtError::Generic(format!("Argon2id parameters: {e}")))?;
    let argon2 = argon2::Argon2::new(
        argon2::Algorithm::Argon2id,
        argon2::Version::V0x13,
        argon2_params,
    );
    let mut ikm = Zeroizing::new([0u8; 64]);
    ikm[..32].copy_from_slice(oprf_output);
    argon2
        .hash_password_into(pin, salt, &mut ikm[32..])
        .map_err(|e| KupferdrahtError::Generic(format!("Argon2id: {e}")))?;
    let mut key = Zeroizing::new([0u8; 32]);
    Hkdf::<Sha256>::new(None, ikm.as_slice())
        .expand(RECOVERY_DATA_LABEL, key.as_mut_slice())
        .map_err(|e| KupferdrahtError::Generic(format!("HKDF: {e}")))?;
    Ok(key)
}

/// Seal `seed` with `key`, the header as associated data.
///
/// # Errors
/// `Generic` when the cipher refuses (it does not for these lengths).
pub(crate) fn seal_seed(
    seed: &[u8; 32],
    key: &[u8; 32],
    header: &[u8],
    nonce: &[u8; 12],
) -> Result<[u8; SEALED_LEN], KupferdrahtError> {
    let sealed = Aes256Gcm::new(key.into())
        .encrypt(
            nonce.into(),
            Payload {
                msg: seed,
                aad: header,
            },
        )
        .map_err(|e| KupferdrahtError::Generic(format!("seal: {e}")))?;
    sealed
        .as_slice()
        .try_into()
        .map_err(|_| KupferdrahtError::Generic(format!("sealed seed of {} bytes", sealed.len())))
}

/// Open a sealed seed.
///
/// # Errors
/// `WrongPin` (without attempts: the caller knows them) when `key` or the
/// header is not the one it was sealed with.
pub(crate) fn open_seed(
    sealed: &[u8; SEALED_LEN],
    key: &[u8; 32],
    header: &[u8],
    nonce: &[u8; 12],
) -> Result<Zeroizing<[u8; 32]>, KupferdrahtError> {
    let wrong_pin = KupferdrahtError::WrongPin {
        attempts_remaining: None,
    };
    let opened = Zeroizing::new(
        Aes256Gcm::new(key.into())
            .decrypt(
                nonce.into(),
                Payload {
                    msg: sealed,
                    aad: header,
                },
            )
            .map_err(|_| wrong_pin)?,
    );
    let mut seed = Zeroizing::new([0u8; 32]);
    if opened.len() != seed.len() {
        return Err(KupferdrahtError::InvalidRecoveryData {
            reason: "notRecoveryData".into(),
        });
    }
    seed.copy_from_slice(&opened);
    Ok(seed)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Small parameters for the tests that do not test the parameters.
    const QUICK: Argon2Params = Argon2Params {
        m_kib: 64,
        t: 1,
        p: 1,
    };
    const SEED: [u8; 32] = [7; 32];
    const HEADER: &[u8] = b"the header";
    const NONCE: [u8; 12] = [9; 12];

    fn key(oprf_output: u8) -> Zeroizing<[u8; 32]> {
        recovery_key(&[oprf_output; 32], b"482913", &[5; 16], QUICK).expect("key")
    }

    #[test]
    fn a_sealed_seed_opens_with_the_same_key() {
        let key = key(1);
        let sealed = seal_seed(&SEED, &key, HEADER, &NONCE).expect("sealed");
        assert_ne!(&sealed[..32], &SEED, "the seed is not in the clear");
        let opened = open_seed(&sealed, &key, HEADER, &NONCE).expect("opened");
        assert_eq!(*opened, SEED);
    }

    /// Another PIN gives another OPRF output — another key.
    #[test]
    fn another_key_is_a_wrong_pin() {
        let sealed = seal_seed(&SEED, &key(1), HEADER, &NONCE).expect("sealed");
        assert!(matches!(
            open_seed(&sealed, &key(2), HEADER, &NONCE),
            Err(KupferdrahtError::WrongPin {
                attempts_remaining: None
            })
        ));
    }

    #[test]
    fn a_changed_header_does_not_open() {
        let key = key(1);
        let sealed = seal_seed(&SEED, &key, HEADER, &NONCE).expect("sealed");
        assert!(open_seed(&sealed, &key, b"another header", &NONCE).is_err());
    }

    #[test]
    fn the_key_depends_on_pin_salt_and_oprf_output() {
        let base = recovery_key(&[1; 32], b"482913", &[5; 16], QUICK).expect("key");
        let pin = recovery_key(&[1; 32], b"482914", &[5; 16], QUICK).expect("key");
        let salt = recovery_key(&[1; 32], b"482913", &[6; 16], QUICK).expect("key");
        let output = recovery_key(&[2; 32], b"482913", &[5; 16], QUICK).expect("key");
        assert_ne!(*base, *pin);
        assert_ne!(*base, *salt);
        assert_ne!(*base, *output);
        let again = recovery_key(&[1; 32], b"482913", &[5; 16], QUICK).expect("key");
        assert_eq!(*base, *again, "deterministic");
    }

    /// New recovery data costs 64 MiB and three passes per guess.
    #[test]
    fn new_recovery_data_uses_the_production_parameters() {
        assert_eq!(
            ARGON2_PARAMS,
            Argon2Params {
                m_kib: 65_536,
                t: 3,
                p: 1
            }
        );
        let key = recovery_key(&[1; 32], b"482913", &[5; 16], ARGON2_PARAMS).expect("key");
        assert_ne!(
            *key,
            *recovery_key(&[1; 32], b"482913", &[5; 16], QUICK).expect("key")
        );
    }
}
