// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 9 — GPL-free Wire-compatible prekey generation (ADR-0010).
//!
//! Replaces the GPL-3.0 `proteus-wasm` call site with a
//! permissive-licensed implementation built on `ed25519-dalek`
//! (BSD-3-Clause), so Kupferdraht's dependency graph contains no GPL
//! code (ADR-0010); Kupferdraht's own license is GPL-3.0-only
//! (ADR-0016).
//!
//! Wire's `POST /v14/clients` schema requires every registered
//! client — even MLS-only ones — to upload a prekey bundle. The
//! bundle is generated once at registration time, uploaded, and
//! never touched again. **Kupferdraht never executes the Double
//! Ratchet protocol** (CLAUDE.md "What NOT to do" + ADR-0001) — the
//! Ed25519 keys we put into the bundle exist solely to satisfy
//! Wire's registration contract.
//!
//! The format Wire's backend accepts is the legacy `proteus`
//! `PreKeyBundle` CBOR shape: a 5-field map with `u8` integer
//! keys (`0..=4`). Each `PreKey` is wrapped in a `PreKeyBundle`
//! envelope (with `signature` = CBOR `null`, since Kupferdraht emits
//! unsigned bundles) and serialised to bytes the registration
//! call base64-encodes onto the wire. The encoder writes those
//! bytes directly without pulling in a CBOR codec crate — the
//! schema is small and stable enough that hand-emission is the
//! simplest correct path. Where the format was observed — proteus
//! v2.1.1 (GPL-3.0), with permalinks — is recorded in
//! `docs/wire/api-notes.md` under `POST /v14/clients`.
//!
//! The hermetic structural decoder in `tests` is the per-commit
//! regression net; live validation against a Wire backend (Step 9d)
//! confirmed end-to-end interop on a fresh device.

use ed25519_dalek::SigningKey;
use rand::rngs::OsRng;

// CBOR (RFC 8949) byte constants for the fixed-schema encoder
// below. Each is the initial byte of one data item as RFC 8949 §3
// defines it: major type in the high three bits, argument in the
// low five. The schema is fixed, so the encoder emits the bytes
// directly instead of pulling in a CBOR codec crate.

/// CBOR major type 5 (map) with 5 entries — the `PreKeyBundle`
/// envelope's outer marker.
const CBOR_MAP_5: u8 = 0xA5;
/// CBOR major type 5 (map) with 1 entry — used by the inner
/// `PublicKey` and `IdentityKey` wrappers.
const CBOR_MAP_1: u8 = 0xA1;
/// CBOR major type 2 (byte string) with the next byte holding a
/// `u8` length. For the 32-byte Ed25519 verifying keys Kupferdraht
/// emits, the full prefix is `[0x58, 0x20]` (length = 0x20 = 32).
const CBOR_BYTES_U8_LEN: u8 = 0x58;
/// CBOR `null` — the `signature` field of an unsigned prekey
/// bundle, the only kind Kupferdraht produces.
const CBOR_NULL: u8 = 0xF6;
/// CBOR major type 0 (unsigned int) with 1-byte argument. Used
/// for `prekey_id` values 24..=255.
const CBOR_U8_HEADER: u8 = 0x18;
/// CBOR major type 0 (unsigned int) with 2-byte big-endian
/// argument. Used for `prekey_id` values ≥ 256 (in particular
/// `LAST_RESORT_PREKEY_ID = u16::MAX`).
const CBOR_U16_HEADER: u8 = 0x19;
/// Length of an Ed25519 verifying key — the payload of the
/// `PublicKey` envelope, for the prekey and for the identity key.
const ED25519_PUBLIC_KEY_BYTES: usize = 32;

/// Number of regular prekeys generated at registration. Wire's
/// schema cap is much higher; 100 gives a fresh device enough
/// one-time keys to start enough Proteus sessions before the
/// backend hands out the last-resort prekey, while keeping
/// registration latency bounded.
pub const INITIAL_PREKEY_COUNT: u16 = 100;

/// Sentinel id Wire reserves for the "last-resort" prekey: a
/// single prekey that the backend hands out after every regular
/// id has been consumed: 65535, the maximum `u16`.
pub const LAST_RESORT_PREKEY_ID: u16 = u16::MAX;

/// One pre-generated prekey, serialised as a `PreKeyBundle` (CBOR)
/// ready for base64-encoding into Wire's JSON payload — one entry
/// of the `prekeys` array in the registration body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreKey {
    pub id: u16,
    pub key: Vec<u8>,
}

/// Bundle of N regular prekeys plus one last-resort prekey,
/// shaped exactly the way Wire's `POST /v14/clients` request
/// expects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrekeyBundle {
    pub prekeys: Vec<PreKey>,
    pub lastkey: PreKey,
}

/// Generate the registration prekey bundle: `INITIAL_PREKEY_COUNT`
/// regular prekeys (ids `0..INITIAL_PREKEY_COUNT`) plus one
/// last-resort prekey (id `LAST_RESORT_PREKEY_ID`). All prekeys
/// share the same freshly-generated identity key and are wrapped
/// in `PreKeyBundle` envelopes serialised to CBOR. The encoder
/// writes into an in-memory `Vec<u8>` and cannot fail.
pub fn build_prekey_bundle() -> PrekeyBundle {
    let mut rng = OsRng;
    let identity = SigningKey::generate(&mut rng);
    let identity_public = identity.verifying_key().to_bytes();

    let mut prekeys = Vec::with_capacity(usize::from(INITIAL_PREKEY_COUNT));
    for id in 0..INITIAL_PREKEY_COUNT {
        let prekey_signing = SigningKey::generate(&mut rng);
        let prekey_public = prekey_signing.verifying_key().to_bytes();
        prekeys.push(PreKey {
            id,
            key: encode_prekey_bundle(id, &prekey_public, &identity_public),
        });
    }

    let last_signing = SigningKey::generate(&mut rng);
    let last_public = last_signing.verifying_key().to_bytes();
    let lastkey = PreKey {
        id: LAST_RESORT_PREKEY_ID,
        key: encode_prekey_bundle(LAST_RESORT_PREKEY_ID, &last_public, &identity_public),
    };

    PrekeyBundle { prekeys, lastkey }
}

/// Encode one `PreKeyBundle` in the CBOR layout Wire's backend
/// parses (`docs/wire/api-notes.md`, `POST /v14/clients`):
///
/// ```text
/// 0xA5                                map(5)
///   0x00 0x01                         field 0 (version) = 1
///   0x01 <prekey_id u16>              field 1 (prekey_id)
///   0x02 <PublicKey envelope>         field 2 (public_key)
///   0x03 <IdentityKey envelope>       field 3 (identity_key)
///   0x04 0xF6                         field 4 (signature) = null
/// ```
///
/// `<PublicKey envelope>` and `<IdentityKey envelope>` are
/// 1-entry maps that wrap the 32-byte raw Ed25519 verifying key
/// (`{0: bytes(32) <pk>}` and `{0: <PublicKey envelope>}`
/// respectively).
fn encode_prekey_bundle(
    prekey_id: u16,
    prekey_pk: &[u8; ED25519_PUBLIC_KEY_BYTES],
    identity_pk: &[u8; ED25519_PUBLIC_KEY_BYTES],
) -> Vec<u8> {
    // Pre-sized for the worst case (lastkey id = u16::MAX) so the
    // Vec never re-allocates: 1 (map5) + 2 (version) + 3 (u16 id)
    // + 1 (key2) + 36 (PublicKey) + 1 (key3) + 38 (IdentityKey) +
    // 2 (signature key + null) = 84 bytes.
    let mut out = Vec::with_capacity(84);
    out.push(CBOR_MAP_5);

    // Field 0 — version = 1.
    encode_field_key(&mut out, 0);
    encode_u8_value(&mut out, 1);

    // Field 1 — prekey_id (u16).
    encode_field_key(&mut out, 1);
    encode_u16_value(&mut out, prekey_id);

    // Field 2 — public_key envelope.
    encode_field_key(&mut out, 2);
    encode_public_key_envelope(&mut out, prekey_pk);

    // Field 3 — identity_key envelope.
    encode_field_key(&mut out, 3);
    encode_identity_key_envelope(&mut out, identity_pk);

    // Field 4 — signature = null. Kupferdraht never signs a bundle.
    encode_field_key(&mut out, 4);
    out.push(CBOR_NULL);

    out
}

/// Push a CBOR field-key byte. The bundle schema uses keys 0..=4,
/// all of which fit CBOR's direct encoding of small unsigned
/// integers — a single byte equal to the value.
fn encode_field_key(out: &mut Vec<u8>, k: u8) {
    debug_assert!(k < 24, "field keys 0..=4 fit in CBOR tiny-int form");
    out.push(k);
}

/// Push a CBOR-encoded `u8` *value* (RFC 8949 §3.1): one byte in
/// the direct form for 0..=23, otherwise a one-byte argument.
fn encode_u8_value(out: &mut Vec<u8>, v: u8) {
    if v < 24 {
        out.push(v);
    } else {
        out.push(CBOR_U8_HEADER);
        out.push(v);
    }
}

/// Push a CBOR-encoded `u16` *value* (RFC 8949 §3.1): direct form
/// for 0..=23, 1-byte argument for 24..=255, 2-byte big-endian
/// argument otherwise.
fn encode_u16_value(out: &mut Vec<u8>, v: u16) {
    if v < 24 {
        out.push(u8::try_from(v).expect("v<24 fits in u8"));
    } else if v < 256 {
        out.push(CBOR_U8_HEADER);
        out.push(u8::try_from(v).expect("v<256 fits in u8"));
    } else {
        out.push(CBOR_U16_HEADER);
        out.extend_from_slice(&v.to_be_bytes());
    }
}

/// Push a `PublicKey` envelope. Layout: `0xA1 0x00 0x58 0x20 <32
/// bytes raw Ed25519 verifying key>` (4 + 32 = 36 bytes).
fn encode_public_key_envelope(out: &mut Vec<u8>, pk: &[u8; ED25519_PUBLIC_KEY_BYTES]) {
    out.push(CBOR_MAP_1);
    encode_field_key(out, 0);
    out.push(CBOR_BYTES_U8_LEN);
    out.push(u8::try_from(ED25519_PUBLIC_KEY_BYTES).expect("32 fits in u8"));
    out.extend_from_slice(pk);
}

/// Push an `IdentityKey` envelope. Layout: `0xA1 0x00 <PublicKey
/// envelope>` (38 bytes total).
fn encode_identity_key_envelope(out: &mut Vec<u8>, identity_pk: &[u8; ED25519_PUBLIC_KEY_BYTES]) {
    out.push(CBOR_MAP_1);
    encode_field_key(out, 0);
    encode_public_key_envelope(out, identity_pk);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Step 70 (gap-B.21): the transient Ed25519 signing keys —
    /// the only secret material in this module; `PreKey.key` holds
    /// public verifying keys — are wiped on drop by dalek's
    /// `zeroize` feature. This pin fails to compile if the feature
    /// is ever dropped (e.g. a future `default-features = false`).
    #[test]
    fn signing_key_wipe_is_compiler_enforced() {
        fn requires_zeroize_on_drop<T: zeroize::ZeroizeOnDrop>(_: &T) {}
        let mut rng = OsRng;
        requires_zeroize_on_drop(&SigningKey::generate(&mut rng));
    }

    /// Decoded view of a Kupferdraht-emitted `PreKeyBundle` CBOR blob:
    /// the five fields of the bundle schema. Until Step 9e the
    /// round-trip went through the then-linked proteus crate's
    /// deserialiser as the interop oracle; since then this
    /// hand-rolled byte-level decoder walks the fixed schema. Step
    /// 9d's live validation confirmed the bytes are wire-compatible;
    /// this decoder is the hermetic regression net for any future
    /// encoder edit.
    struct DecodedBundle {
        version: u8,
        prekey_id: u16,
        public_key: [u8; ED25519_PUBLIC_KEY_BYTES],
        identity_key: [u8; ED25519_PUBLIC_KEY_BYTES],
        signature_is_null: bool,
    }

    /// Hand-rolled `PreKeyBundle` CBOR decoder. Asserts each byte
    /// against the fixed schema described in `encode_prekey_bundle`'s
    /// docstring; on any structural mismatch panics with the
    /// expected/actual byte pair so encoder regressions surface
    /// pinpoint-localised in test output.
    fn decode_prekey_bundle(bytes: &[u8]) -> DecodedBundle {
        let mut c = bytes.iter().copied();
        // map(5) outer marker
        assert_eq!(c.next(), Some(CBOR_MAP_5), "expected outer 0xA5 map(5)");
        // field 0 — version
        assert_eq!(c.next(), Some(0u8), "expected field key 0");
        let version = c.next().expect("version byte");
        // field 1 — prekey_id (u16, three possible CBOR encodings)
        assert_eq!(c.next(), Some(1u8), "expected field key 1");
        let prekey_id = decode_u16_value(&mut c);
        // field 2 — public_key envelope
        assert_eq!(c.next(), Some(2u8), "expected field key 2");
        let public_key = decode_public_key_envelope(&mut c);
        // field 3 — identity_key envelope (1-entry map wrapping a PublicKey envelope)
        assert_eq!(c.next(), Some(3u8), "expected field key 3");
        assert_eq!(c.next(), Some(CBOR_MAP_1), "expected IdentityKey 0xA1");
        assert_eq!(c.next(), Some(0u8), "expected IdentityKey field key 0");
        let identity_key = decode_public_key_envelope(&mut c);
        // field 4 — signature (Kupferdraht emits null)
        assert_eq!(c.next(), Some(4u8), "expected field key 4");
        let sig_byte = c.next().expect("signature byte");
        let signature_is_null = sig_byte == CBOR_NULL;
        assert!(c.next().is_none(), "trailing bytes after PreKeyBundle");
        DecodedBundle {
            version,
            prekey_id,
            public_key,
            identity_key,
            signature_is_null,
        }
    }

    fn decode_u16_value(c: &mut impl Iterator<Item = u8>) -> u16 {
        let head = c.next().expect("u16 head byte");
        match head {
            v if v < 24 => u16::from(v),
            CBOR_U8_HEADER => u16::from(c.next().expect("u8 payload")),
            CBOR_U16_HEADER => {
                let hi = c.next().expect("u16 hi byte");
                let lo = c.next().expect("u16 lo byte");
                u16::from_be_bytes([hi, lo])
            }
            other => panic!("unexpected CBOR uint head byte: {other:#04X}"),
        }
    }

    fn decode_public_key_envelope(
        c: &mut impl Iterator<Item = u8>,
    ) -> [u8; ED25519_PUBLIC_KEY_BYTES] {
        assert_eq!(c.next(), Some(CBOR_MAP_1), "expected PublicKey 0xA1");
        assert_eq!(c.next(), Some(0u8), "expected PublicKey field key 0");
        assert_eq!(
            c.next(),
            Some(CBOR_BYTES_U8_LEN),
            "expected bytes(u8-len) header 0x58",
        );
        assert_eq!(c.next(), Some(32u8), "expected length 32 for Ed25519 pk");
        let mut pk = [0u8; ED25519_PUBLIC_KEY_BYTES];
        for (i, byte) in pk.iter_mut().enumerate() {
            *byte = c.next().unwrap_or_else(|| {
                panic!("public key truncated at byte {i}");
            });
        }
        pk
    }

    #[test]
    fn bundle_has_initial_prekey_count_plus_lastkey() {
        let bundle = build_prekey_bundle();
        assert_eq!(
            u16::try_from(bundle.prekeys.len()).expect("fits"),
            INITIAL_PREKEY_COUNT,
        );
        assert!(!bundle.lastkey.key.is_empty());
    }

    #[test]
    fn prekey_ids_are_sequential_starting_at_zero() {
        let bundle = build_prekey_bundle();
        for (i, pk) in bundle.prekeys.iter().enumerate() {
            let expected = u16::try_from(i).expect("fits in u16");
            assert_eq!(pk.id, expected, "prekey at index {i} has wrong id");
        }
    }

    #[test]
    fn lastkey_id_is_wires_last_resort_sentinel() {
        let bundle = build_prekey_bundle();
        assert_eq!(bundle.lastkey.id, LAST_RESORT_PREKEY_ID);
    }

    #[test]
    fn each_prekey_cbor_starts_with_5_entry_map_marker() {
        // The bundle opens with a CBOR map of five entries — major
        // type 5, argument 5 → leading byte 0xA5.
        // The full structural decoder below will catch any deeper
        // drift; this test pins the leading-byte invariant for fast
        // feedback.
        let bundle = build_prekey_bundle();
        for (i, pk) in bundle.prekeys.iter().enumerate() {
            let leading = pk.key.first().copied().expect("non-empty");
            assert_eq!(
                leading, 0xA5,
                "prekey {i} CBOR must start with `A5` (5-entry map), got {leading:#04X}",
            );
        }
        assert_eq!(bundle.lastkey.key.first().copied(), Some(0xA5));
    }

    #[test]
    fn each_prekey_decodes_with_expected_field_layout() {
        let bundle = build_prekey_bundle();
        for (i, pk) in bundle.prekeys.iter().enumerate() {
            let decoded = decode_prekey_bundle(&pk.key);
            assert_eq!(decoded.version, 1, "prekey {i} version != 1");
            assert_eq!(
                decoded.prekey_id, pk.id,
                "prekey {i} CBOR prekey_id != envelope id",
            );
            assert!(
                decoded.signature_is_null,
                "prekey {i} signature must be CBOR null — \
                 Kupferdraht emits unsigned bundles, never signed ones",
            );
        }
    }

    #[test]
    fn lastkey_decodes_with_expected_field_layout() {
        let bundle = build_prekey_bundle();
        let decoded = decode_prekey_bundle(&bundle.lastkey.key);
        assert_eq!(decoded.version, 1);
        assert_eq!(decoded.prekey_id, LAST_RESORT_PREKEY_ID);
        assert!(decoded.signature_is_null);
    }

    #[test]
    fn all_prekeys_share_one_identity_key() {
        // Wire's registration model: one identity key per
        // device; every prekey in the bundle wraps that same
        // identity. Decoding all prekeys must yield identical
        // identity_key bytes.
        let bundle = build_prekey_bundle();
        let canonical = decode_prekey_bundle(&bundle.prekeys[0].key).identity_key;
        for (i, pk) in bundle.prekeys.iter().enumerate().skip(1) {
            let id_bytes = decode_prekey_bundle(&pk.key).identity_key;
            assert_eq!(
                canonical, id_bytes,
                "prekey {i} carries a different identity_key than its siblings",
            );
        }
        // lastkey shares the same identity too.
        let last_id = decode_prekey_bundle(&bundle.lastkey.key).identity_key;
        assert_eq!(
            canonical, last_id,
            "lastkey identity_key drifts from regular prekeys",
        );
    }

    #[test]
    fn distinct_invocations_produce_distinct_identity_keys() {
        // build_prekey_bundle is non-deterministic by design — each
        // call sources fresh CSPRNG bytes. Two calls must therefore
        // produce two different identities.
        let a = build_prekey_bundle();
        let b = build_prekey_bundle();
        let id_a = decode_prekey_bundle(&a.lastkey.key).identity_key;
        let id_b = decode_prekey_bundle(&b.lastkey.key).identity_key;
        assert_ne!(
            id_a, id_b,
            "identity bytes should differ across invocations — \
             OS CSPRNG must source fresh entropy each call",
        );
    }

    #[test]
    fn prekey_public_keys_are_unique_within_bundle() {
        // Each prekey id maps to a fresh keypair — no two regular
        // prekeys may share a public key (a CSPRNG collision on
        // 32 random bytes is astronomically unlikely; this test
        // catches bugs like "encoder reused identity_pk for prekey
        // public_key field" that would otherwise sail past the
        // structural decoder above).
        let bundle = build_prekey_bundle();
        let mut seen: Vec<[u8; ED25519_PUBLIC_KEY_BYTES]> =
            Vec::with_capacity(bundle.prekeys.len());
        for (i, pk) in bundle.prekeys.iter().enumerate() {
            let pub_key = decode_prekey_bundle(&pk.key).public_key;
            assert!(
                !seen.contains(&pub_key),
                "prekey {i}'s public_key collides with an earlier prekey",
            );
            seen.push(pub_key);
        }
    }
}
