// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! The local PIN check (Step 112, ADR-0035): `changePin` and `verifyPin`
//! check the current PIN without the PIN service — a check there would
//! spend one of the user's attempts. The verifier is
//! `HMAC-SHA256(HKDF(seed, "kupferdraht/v1/pin-verifier"), NFC(pin))`, kept with
//! the recovery state: breaking it needs the seed, and whoever holds the
//! seed needs no PIN.

use hmac::{Hmac, Mac};
use sha2::Sha256;

use super::pin::normalize_pin;
use crate::error::KupferdrahtError;
use crate::storage::account_keys::derive_from_seed;

/// The frozen HKDF label of the verifier's key.
pub(crate) const PIN_VERIFIER_LABEL: &[u8] = b"kupferdraht/v1/pin-verifier";

/// The verifier of `pin` for the account whose seed is `seed`.
///
/// # Errors
/// `Generic` if HMAC refused the key (it takes any length).
pub(crate) fn pin_verifier(seed: &[u8; 32], pin: &str) -> Result<[u8; 32], KupferdrahtError> {
    Ok(verifier_mac(seed, pin)?.finalize().into_bytes().into())
}

/// Whether `pin` is the PIN `verifier` was made for (compared in constant
/// time).
///
/// # Errors
/// As [`pin_verifier`].
pub(crate) fn pin_matches(
    seed: &[u8; 32],
    pin: &str,
    verifier: &[u8; 32],
) -> Result<bool, KupferdrahtError> {
    Ok(verifier_mac(seed, pin)?.verify_slice(verifier).is_ok())
}

/// The HMAC over the normalized `pin`, keyed from `seed`.
fn verifier_mac(seed: &[u8; 32], pin: &str) -> Result<Hmac<Sha256>, KupferdrahtError> {
    let key = derive_from_seed(seed, PIN_VERIFIER_LABEL);
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key.as_slice())
        .map_err(|e| KupferdrahtError::Generic(format!("PIN verifier: {e}")))?;
    mac.update(normalize_pin(pin).as_bytes());
    Ok(mac)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Seed `00 01 … 1f`.
    fn seed() -> [u8; 32] {
        let mut seed = [0u8; 32];
        for (i, byte) in seed.iter_mut().enumerate() {
            *byte = u8::try_from(i).expect("fits");
        }
        seed
    }

    /// Computed independently (Python `hmac`, HKDF by RFC 5869).
    #[test]
    fn the_verifier_is_the_known_answer() {
        assert_eq!(
            pin_verifier(&seed(), "482913").expect("verifier"),
            [
                0x3b, 0x3b, 0xc8, 0x98, 0xdd, 0x53, 0x57, 0xe2, 0xf6, 0x24, 0xa5, 0xc0, 0x87, 0x46,
                0xd3, 0x7c, 0x09, 0x98, 0x68, 0x54, 0x16, 0x22, 0xa4, 0x51, 0xea, 0xcf, 0x8c, 0x52,
                0x27, 0x42, 0x0e, 0x6e
            ]
        );
    }

    #[test]
    fn the_right_pin_matches_and_another_does_not() {
        let verifier = pin_verifier(&seed(), "482913").expect("verifier");
        assert!(pin_matches(&seed(), "482913", &verifier).expect("checked"));
        assert!(!pin_matches(&seed(), "482914", &verifier).expect("checked"));
    }

    /// The same PIN typed composed or decomposed is the same PIN.
    #[test]
    fn the_pin_is_normalized_first() {
        let verifier = pin_verifier(&seed(), "Gr\u{fc}\u{df}e 2026").expect("verifier");
        assert!(pin_matches(&seed(), "Gru\u{308}\u{df}e 2026", &verifier).expect("checked"));
    }

    #[test]
    fn another_seed_gives_another_verifier() {
        assert_ne!(
            pin_verifier(&seed(), "482913").expect("verifier"),
            pin_verifier(&[9; 32], "482913").expect("verifier")
        );
    }
}
