// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 124 — receipts for the PIN service: a successful export or
//! restore gives its attempt back, so only wrong PINs stay counted.
//!
//! Each seed has a receipt key pair, ECDSA over P-256 (the PIN service's
//! `docs/protocol.md`, "Receipts"):
//!
//! ```text
//! refundSeed = HKDF-SHA256(ikm = seed, salt = empty,
//!                          info = "natrium-pin-refund-v1|<domain>|<userId>")
//! d          = DeriveKeyPair(refundSeed, "natrium-pin-refund-v1")
//!              (RFC 9497 §3.2.1, P256-SHA256, mode OPRF)
//! refundKey  = d·G, compressed SEC1 (33 bytes)
//! signature  = ECDSA-P256-SHA256(d, "natrium-pin-refund-v1|<domain>|<userId>|<attemptId>"),
//!              r ‖ s (64 bytes)
//! ```
//!
//! The public key travels in the recovery data's header and with every
//! evaluation (the service derives its OPRF key per refund key); only the
//! seed — so only the right PIN — yields the private key.

use p256::ecdsa::signature::Signer as _;
use p256::ecdsa::{Signature, SigningKey};
use zeroize::{Zeroize as _, Zeroizing};

use crate::error::KupferdrahtError;

/// A compressed P-256 public key.
pub(crate) const REFUND_KEY_LEN: usize = 33;
/// An ECDSA signature, `r ‖ s`.
pub(crate) const SIGNATURE_LEN: usize = 64;
/// The receipts' label — HKDF's info prefix, `DeriveKeyPair`'s info, the
/// message's prefix.
const LABEL: &str = "natrium-pin-refund-v1";

/// The receipt key pair of a seed for an account.
pub(crate) struct ReceiptKey {
    signing: SigningKey,
    domain: String,
    user_id: String,
}

impl ReceiptKey {
    /// The receipt key pair of `seed` for `account_id` (`<userId>@<domain>`).
    ///
    /// # Errors
    /// `InvalidInput` for an account id without a domain.
    pub(crate) fn derive(seed: &[u8; 32], account_id: &str) -> Result<Self, KupferdrahtError> {
        let (user_id, domain) = split_account(account_id)?;
        let refund_seed = refund_seed(seed, account_id)?;
        let failed =
            |e: &dyn std::fmt::Display| KupferdrahtError::Generic(format!("receipt key: {e}"));
        let server = voprf::OprfServer::<p256::NistP256>::new_from_seed(
            refund_seed.as_slice(),
            LABEL.as_bytes(),
        )
        .map_err(|e| failed(&e))?;
        let mut scalar = server.serialize();
        let signing = SigningKey::from_bytes(&scalar).map_err(|e| failed(&e));
        scalar.as_mut_slice().zeroize();
        Ok(Self {
            signing: signing?,
            domain: domain.to_owned(),
            user_id: user_id.to_owned(),
        })
    }

    /// The public key, compressed — `refundKey`.
    pub(crate) fn public_key(&self) -> [u8; REFUND_KEY_LEN] {
        let point = self.signing.verifying_key().to_encoded_point(true);
        let mut key = [0u8; REFUND_KEY_LEN];
        key.copy_from_slice(point.as_bytes());
        key
    }

    /// The receipt for `attempt_id`, exactly as the service returned it:
    /// ECDSA P-256/SHA-256 (RFC 6979), `r ‖ s`.
    pub(crate) fn sign(&self, attempt_id: &str) -> [u8; SIGNATURE_LEN] {
        let message = message(&self.domain, &self.user_id, attempt_id);
        let signature: Signature = self.signing.sign(message.as_bytes());
        signature.to_bytes().into()
    }
}

/// The receipt key's seed: HKDF-SHA256 of the account's seed, no salt,
/// `natrium-pin-refund-v1|<domain>|<userId>` as info.
///
/// # Errors
/// `InvalidInput` for an account id without a domain.
pub(crate) fn refund_seed(
    seed: &[u8; 32],
    account_id: &str,
) -> Result<Zeroizing<[u8; 32]>, KupferdrahtError> {
    let (user_id, domain) = split_account(account_id)?;
    let info = format!("{LABEL}|{domain}|{user_id}");
    Ok(crate::storage::account_keys::derive_from_seed(
        seed,
        info.as_bytes(),
    ))
}

/// What a receipt signs — the known-answer tests' view of [`ReceiptKey::sign`].
///
/// # Errors
/// `InvalidInput` for an account id without a domain.
#[cfg(test)]
pub(crate) fn refund_message(
    account_id: &str,
    attempt_id: &str,
) -> Result<String, KupferdrahtError> {
    let (user_id, domain) = split_account(account_id)?;
    Ok(message(domain, user_id, attempt_id))
}

fn message(domain: &str, user_id: &str, attempt_id: &str) -> String {
    format!("{LABEL}|{domain}|{user_id}|{attempt_id}")
}

/// `<userId>@<domain>` → (user id, domain).
fn split_account(account_id: &str) -> Result<(&str, &str), KupferdrahtError> {
    account_id
        .split_once('@')
        .filter(|(user_id, domain)| !user_id.is_empty() && !domain.is_empty())
        .ok_or_else(|| {
            KupferdrahtError::InvalidInput(format!(
                "{account_id:?} is not a qualified account id (<userId>@<domain>)"
            ))
        })
}

#[cfg(test)]
mod tests {
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD;
    use p256::ecdsa::signature::Verifier as _;
    use p256::ecdsa::{Signature, VerifyingKey};

    use super::*;

    struct Vector {
        name: &'static str,
        account: &'static str,
        secret: &'static str,
        refund_seed: &'static str,
        refund_private_key: &'static str,
        refund_key: &'static str,
        attempt_id: &'static str,
        refund_message: &'static str,
        signature: &'static str,
    }

    /// The PIN service's `interop.json`, first vector; `secret` is the
    /// seed. The service's second vector names a development instance
    /// in the signed message and is left out until the service issues
    /// one with a neutral domain (open.md §77).
    const VECTORS: [Vector; 1] = [Vector {
        name: "digits",
        account: "39b7f597-dfd1-4dff-86f5-fe1b79cb70a0@wire.example",
        secret: "a0a1a2a3a4a5a6a7a8a9aaabacadaeafb0b1b2b3b4b5b6b7b8b9babbbcbdbebf",
        refund_seed: "53a4fe673605e1a2405120a82330d6d3c09e0fa611968b43d64fca46068ee1c5",
        refund_private_key: "7bbd2a4fefe9afaa3301f84d9d679a5778076e9ab408287f1c2988e194356e63",
        refund_key: "A7RAoFrwwW6LPWoswV51l5MYJ3IkumhNz2Wk3KgW8xku",
        attempt_id: "AAECAwQFBgcICQoLDA0ODw==",
        refund_message: "natrium-pin-refund-v1|wire.example|39b7f597-dfd1-4dff-86f5-fe1b79cb70a0|AAECAwQFBgcICQoLDA0ODw==",
        signature: "LY93yq2zoZXkJnUC/JFzXhTgi4C66EKWHtigtYTqiNXVcdvwe058YgitONuIWVXa4eQ3cJNZOyCyTvIP+UWAKA==",
    }];

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex"))
            .collect()
    }

    fn seed(v: &Vector) -> [u8; 32] {
        hex(v.secret).try_into().expect("32 bytes")
    }

    fn verifying_key(v: &Vector) -> VerifyingKey {
        VerifyingKey::from_sec1_bytes(&STANDARD.decode(v.refund_key).expect("base64"))
            .expect("a P-256 point")
    }

    #[test]
    fn the_refund_seed_is_hkdf_of_the_seed() {
        for v in &VECTORS {
            let derived = refund_seed(&seed(v), v.account).expect("refund seed");
            assert_eq!(derived.to_vec(), hex(v.refund_seed), "{}", v.name);
        }
    }

    #[test]
    fn the_private_key_is_derive_key_pair_of_the_refund_seed() {
        for v in &VECTORS {
            let key = ReceiptKey::derive(&seed(v), v.account).expect("receipt key");
            assert_eq!(
                key.signing.to_bytes().to_vec(),
                hex(v.refund_private_key),
                "{}",
                v.name
            );
        }
    }

    #[test]
    fn the_refund_key_is_the_compressed_public_key() {
        for v in &VECTORS {
            let key = ReceiptKey::derive(&seed(v), v.account).expect("receipt key");
            assert_eq!(
                STANDARD.encode(key.public_key()),
                v.refund_key,
                "{}",
                v.name
            );
        }
    }

    #[test]
    fn the_message_names_domain_user_and_attempt() {
        for v in &VECTORS {
            assert_eq!(
                refund_message(v.account, v.attempt_id).expect("message"),
                v.refund_message,
                "{}",
                v.name
            );
        }
    }

    /// The service's signatures are random (and not low-S): checked, not
    /// compared — which also pins that `r ‖ s` and the message line up.
    #[test]
    fn the_services_signatures_verify_with_the_refund_key() {
        for v in &VECTORS {
            let signature = Signature::from_slice(&STANDARD.decode(v.signature).expect("base64"))
                .expect("r ‖ s");
            assert!(
                verifying_key(v)
                    .verify(v.refund_message.as_bytes(), &signature)
                    .is_ok(),
                "{}",
                v.name
            );
        }
    }

    #[test]
    fn our_receipts_verify_with_the_refund_key() {
        for v in &VECTORS {
            let key = ReceiptKey::derive(&seed(v), v.account).expect("receipt key");
            let signature = Signature::from_slice(&key.sign(v.attempt_id)).expect("r ‖ s");
            assert!(
                verifying_key(v)
                    .verify(v.refund_message.as_bytes(), &signature)
                    .is_ok(),
                "{}",
                v.name
            );
        }
    }

    #[test]
    fn an_account_id_without_a_domain_is_refused() {
        assert!(matches!(
            ReceiptKey::derive(&[7; 32], "no-domain"),
            Err(KupferdrahtError::InvalidInput(_))
        ));
        assert!(matches!(
            refund_message("no-domain", "AAECAwQFBgcICQoLDA0ODw=="),
            Err(KupferdrahtError::InvalidInput(_))
        ));
    }
}
