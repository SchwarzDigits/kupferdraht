// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! The client side of the OPRF (Step 112, ADR-0035): RFC 9497, mode OPRF,
//! suite `P256-SHA256` — the PIN service's. The PIN is blinded before it
//! leaves Kupferdraht; the service evaluates the blinded element with its key;
//! finalizing gives 32 bytes only the right PIN and the service's key
//! produce. Checked against the PIN service's own interop values.

use rand::{CryptoRng, RngCore};
use voprf::{EvaluationElement, OprfClient};
use zeroize::Zeroizing;

use crate::error::KupferdrahtError;

/// RFC 9497's `P256-SHA256`, the PIN service's suite.
type Suite = p256::NistP256;

/// Length of a serialized P-256 element (compressed SEC1).
pub(crate) const ELEMENT_LEN: usize = 33;

/// A blinded PIN: the element for the service, and what finalizing needs.
pub(crate) struct Blinded {
    client: OprfClient<Suite>,
    element: [u8; ELEMENT_LEN],
}

impl Blinded {
    /// The blinded element to send to the PIN service.
    pub(crate) fn element(&self) -> &[u8; ELEMENT_LEN] {
        &self.element
    }
}

/// Blind the NFC-normalized `pin` with a random scalar from `rng`.
///
/// # Errors
/// `Generic` when the input cannot be blinded (an empty PIN; no PIN the
/// rules admit is one).
pub(crate) fn blind(
    pin: &[u8],
    rng: &mut (impl RngCore + CryptoRng),
) -> Result<Blinded, KupferdrahtError> {
    let result = OprfClient::<Suite>::blind(pin, rng)
        .map_err(|e| KupferdrahtError::Generic(format!("OPRF blind: {e:?}")))?;
    Ok(Blinded {
        element: element_bytes(&result.message.serialize())?,
        client: result.state,
    })
}

/// Finalize the service's `evaluated` element for the `blinded` `pin`.
///
/// # Errors
/// `BackendRejected` when `evaluated` is not a P-256 element.
pub(crate) fn finalize(
    blinded: &Blinded,
    pin: &[u8],
    evaluated: &[u8],
) -> Result<Zeroizing<[u8; 32]>, KupferdrahtError> {
    let element = EvaluationElement::<Suite>::deserialize(evaluated).map_err(|e| {
        KupferdrahtError::BackendRejected(format!("the PIN service's evaluation: {e:?}"))
    })?;
    let output = blinded
        .client
        .finalize(pin, &element)
        .map_err(|e| KupferdrahtError::Generic(format!("OPRF finalize: {e:?}")))?;
    let mut bytes = Zeroizing::new([0u8; 32]);
    bytes.copy_from_slice(&output);
    Ok(bytes)
}

/// A serialized element as its fixed-size array.
fn element_bytes(bytes: &[u8]) -> Result<[u8; ELEMENT_LEN], KupferdrahtError> {
    bytes
        .try_into()
        .map_err(|_| KupferdrahtError::Generic(format!("an OPRF element of {} bytes", bytes.len())))
}

/// Blind with a given scalar — the interop values fix it.
#[cfg(test)]
pub(crate) fn blind_with_scalar(pin: &[u8], scalar: &[u8; 32]) -> Blinded {
    let scalar = <Suite as voprf::Group>::deserialize_scalar(scalar).expect("a scalar");
    let result = OprfClient::<Suite>::deterministic_blind_unchecked(pin, scalar).expect("blinded");
    Blinded {
        element: element_bytes(&result.message.serialize()).expect("33 bytes"),
        client: result.state,
    }
}

/// The PIN service's evaluation, for tests: a mock service that evaluates
/// with a known key.
#[cfg(test)]
pub(crate) fn evaluate_as_service(secret_key: &[u8; 32], blinded: &[u8]) -> [u8; ELEMENT_LEN] {
    let server = voprf::OprfServer::<Suite>::new_with_key(secret_key).expect("a key");
    let element = voprf::BlindedElement::<Suite>::deserialize(blinded).expect("an element");
    element_bytes(&server.blind_evaluate(&element).serialize()).expect("33 bytes")
}

#[cfg(test)]
mod tests {
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD;

    use super::*;
    use crate::recovery::pin::normalize_pin;

    struct Vector {
        name: &'static str,
        pin: &'static str,
        input: &'static str,
        blind: &'static str,
        blinded_element: &'static str,
        evaluated_element: &'static str,
        output: &'static str,
        secret_key: &'static str,
    }

    /// The PIN service's `internal/evaluator/testdata/interop.json`, both
    /// exchanges — since Step 124 with the service key derived per refund
    /// key (`natrium-recovery-v2|…|<refundKey>`). The second PIN is given
    /// decomposed; its input is NFC.
    const VECTORS: [Vector; 2] = [
        Vector {
            name: "digits",
            pin: "123456",
            input: "313233343536",
            blind: "3338fa65ec36e0290022b48eb562889d89dbfa691d1cde91517fa222ed7ad364",
            blinded_element: "A4aFtYKxKBlhHYd8F6OcKbRg+zqeaKZnmby5fWsF7vxM",
            evaluated_element: "AsBqurhoZJbu4AED2IdNksQCdxt5ueugFvrfVYYQXMNp",
            output: "cdb5a6c8ce1a0fa5ace124e069ab15e2f7802c768720108ca630db2f922344a9",
            secret_key: "d3e29efe18859211267ca8afc6540d9afc9e4ca32b364a121e56cfd8e135a5e6",
        },
        Vector {
            name: "unicode",
            pin: "Gru\u{308}\u{df}e 2026",
            input: "4772c3bcc39f652032303236",
            blind: "0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f",
            blinded_element: "A5RtTH3y8WNxMyyXkJU8mw1MCxVMFuQ8vkoefSyMY+xr",
            evaluated_element: "Au2FSKNvTTq69EC1ERtkpJGi8iUQEa0cRu3p1k/uzQmn",
            output: "d625938cc2b2d2137da10205e402b0987cc3848d469e74662e1278028b9fb11f",
            secret_key: "b3de13bf4e600a6d132e18b6f442c43e4af4ed2258ba8b359adbf24d5e6e1af7",
        },
    ];

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex"))
            .collect()
    }

    fn array32(s: &str) -> [u8; 32] {
        hex(s).try_into().expect("32 bytes")
    }

    #[test]
    fn the_pin_is_normalized_as_the_service_expects() {
        for v in &VECTORS {
            assert_eq!(normalize_pin(v.pin).as_bytes(), hex(v.input), "{}", v.name);
        }
    }

    #[test]
    fn blinding_reproduces_the_services_blinded_elements() {
        for v in &VECTORS {
            let blinded = blind_with_scalar(&hex(v.input), &array32(v.blind));
            assert_eq!(
                STANDARD.encode(blinded.element()),
                v.blinded_element,
                "{}",
                v.name
            );
        }
    }

    #[test]
    fn finalizing_reproduces_the_services_outputs() {
        for v in &VECTORS {
            let blinded = blind_with_scalar(&hex(v.input), &array32(v.blind));
            let evaluated = STANDARD.decode(v.evaluated_element).expect("base64");
            let output = finalize(&blinded, &hex(v.input), &evaluated).expect("finalize");
            assert_eq!(output.to_vec(), hex(v.output), "{}", v.name);
        }
    }

    /// The test service evaluates as the real one does.
    #[test]
    fn the_test_service_evaluates_like_the_real_one() {
        for v in &VECTORS {
            let blinded = STANDARD.decode(v.blinded_element).expect("base64");
            let evaluated = evaluate_as_service(&array32(v.secret_key), &blinded);
            assert_eq!(
                STANDARD.encode(evaluated),
                v.evaluated_element,
                "{}",
                v.name
            );
        }
    }

    #[test]
    fn a_random_blind_round_trips_through_the_service() {
        let key = array32(VECTORS[0].secret_key);
        let blinded = blind(b"482913", &mut rand::rngs::OsRng).expect("blind");
        let evaluated = evaluate_as_service(&key, blinded.element());
        let once = finalize(&blinded, b"482913", &evaluated).expect("finalize");
        let again = blind(b"482913", &mut rand::rngs::OsRng).expect("blind");
        let evaluated = evaluate_as_service(&key, again.element());
        let twice = finalize(&again, b"482913", &evaluated).expect("finalize");
        assert_eq!(
            *once, *twice,
            "the output depends on the PIN, not the blind"
        );
    }

    #[test]
    fn a_malformed_evaluation_is_refused() {
        let blinded = blind(b"482913", &mut rand::rngs::OsRng).expect("blind");
        assert!(finalize(&blinded, b"482913", &[0u8; ELEMENT_LEN]).is_err());
    }
}
