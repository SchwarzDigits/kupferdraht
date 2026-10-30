// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 95d — the MLS thumbprint of a leaf signature key.
//!
//! RFC 7638 JWK thumbprint over the P-256 public key: the canonical JWK
//! `{"crv":"P-256","kty":"EC","x":…,"y":…}` (members in lexicographic
//! order, no whitespace, coordinates base64url without padding), hashed
//! with SHA-256 (ciphersuite `0x0002`), base64url without padding —
//! the value official Wire apps show as "MLS Thumbprint". An encoding +
//! a standard hash, no new primitive.

use base64::Engine as _;
use sha2::{Digest as _, Sha256};

/// SEC1 uncompressed P-256 point: `0x04 || X(32) || Y(32)`.
const SEC1_P256_LEN: usize = 65;
const SEC1_UNCOMPRESSED: u8 = 0x04;

/// The MLS thumbprint of `signature_key`, a SEC1 uncompressed P-256
/// point (`0x04 || X || Y`, RFC 9420 §5.1.1). `None` for any other
/// encoding.
pub(crate) fn mls_thumbprint(signature_key: &[u8]) -> Option<String> {
    if signature_key.len() != SEC1_P256_LEN || signature_key[0] != SEC1_UNCOMPRESSED {
        return None;
    }
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let x = b64.encode(&signature_key[1..33]);
    let y = b64.encode(&signature_key[33..SEC1_P256_LEN]);
    // RFC 7638 §3.2: required members only, lexicographic order, no
    // whitespace — the coordinates are base64url, so no escaping.
    let canonical_jwk = format!(r#"{{"crv":"P-256","kty":"EC","x":"{x}","y":"{y}"}}"#);
    Some(b64.encode(Sha256::digest(canonical_jwk.as_bytes())))
}

#[cfg(test)]
mod tests {
    use super::mls_thumbprint;

    /// RFC 7515 Appendix A.3's P-256 key (`x` = `f83OJ3D2…`, `y` =
    /// `x_FEzRu9…`) as a SEC1 uncompressed point.
    const RFC7515_A3_SEC1_HEX: &str = "047fcdce2770f6c45d4183cbee6fdb4b7b580733357be9ef13bacf6e3c7bd15445c7f144cd1bbd9b7e872cdfedb9eeb9f4b3695d6ea90b24ad8a4623288588e5ad";

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex"))
            .collect()
    }

    /// Known answer computed independently (Python: `hashlib.sha256` over
    /// the canonical JWK, `base64.urlsafe_b64encode` without padding).
    #[test]
    fn rfc7515_a3_key_has_the_known_thumbprint() {
        assert_eq!(
            mls_thumbprint(&hex(RFC7515_A3_SEC1_HEX)).as_deref(),
            Some("oKIywvGUpTVTyxMQ3bwIIeQUudfr_CkLMjCE19ECD-U")
        );
    }

    #[test]
    fn a_non_sec1_key_has_no_thumbprint() {
        assert_eq!(mls_thumbprint(&[0x02; 33]), None, "compressed point");
        assert_eq!(mls_thumbprint(&[0x04; 64]), None, "one byte short");
        assert_eq!(mls_thumbprint(&[]), None, "empty");
    }
}
