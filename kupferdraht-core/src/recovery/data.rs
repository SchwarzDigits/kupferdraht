// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! The recovery-data format (Step 112, ADR-0035; version 2 since Step 120,
//! ADR-0040; version 3 since Step 124): compact, versioned bytes the
//! embedder never parses. The header — magic `SDRD`, format version, the PIN
//! service's key version, the Argon2id parameters, salt, nonce, the SHA-256
//! of the account id (not the id: a found file names no owner, and another
//! account's file is refused before the PIN service is asked), the device's
//! Wire client id, when the data was made and the seed's receipt key (the
//! PIN service keys its evaluation by it) — is the associated data of the
//! sealed seed that follows it. Data of a device the account's storage slot
//! no longer names is refused before the PIN service is asked, too.
//!
//! ```text
//! magic 4 | version 1 | key version 4 | m (KiB) 4 | t 4 | p 4 | salt 16 |
//! nonce 12 | account hash 32 | client id 16 | created (ms) 8 |
//! refund key 33 | sealed seed 32 + tag 16                      (all BE)
//! ```

use super::receipt::REFUND_KEY_LEN;
use crate::error::KupferdrahtError;

/// The first bytes of recovery data.
pub(crate) const MAGIC: [u8; 4] = *b"SDRD";
/// The format this build writes and reads — versions 1 (no device) and 2
/// (no receipt key: the PIN service cannot evaluate for it any more) are not
/// read (no migration, ADR-0040, Step 124).
pub(crate) const FORMAT_VERSION: u8 = 3;
/// The length of the header — the associated data.
pub(crate) const HEADER_LEN: usize =
    4 + 1 + 4 + 12 + 16 + 12 + 32 + CLIENT_ID_LEN + 8 + REFUND_KEY_LEN;
/// The client id's field: its text, padded with zero bytes (Wire's ids are
/// at most 16 characters).
pub(crate) const CLIENT_ID_LEN: usize = 16;
/// The length of the sealed seed with its tag.
pub(crate) const SEALED_LEN: usize = 32 + 16;

/// The Argon2id parameters recovery data was sealed with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Argon2Params {
    /// Memory in KiB.
    pub(crate) m_kib: u32,
    pub(crate) t: u32,
    pub(crate) p: u32,
}

/// Recovery data, decoded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RecoveryData {
    pub(crate) key_version: u32,
    pub(crate) argon2: Argon2Params,
    pub(crate) salt: [u8; 16],
    pub(crate) nonce: [u8; 12],
    pub(crate) account_hash: [u8; 32],
    /// Step 120 (ADR-0040): the device — the Wire client — the data belongs
    /// to: its id's text, zero-padded.
    pub(crate) client_id: [u8; CLIENT_ID_LEN],
    /// When the data was made, ms since the Unix epoch.
    pub(crate) created_at_ms: u64,
    /// Step 124: the seed's receipt key (compressed P-256), which the PIN
    /// service derives its key for this data from.
    pub(crate) refund_key: [u8; REFUND_KEY_LEN],
    pub(crate) sealed: [u8; SEALED_LEN],
}

impl RecoveryData {
    /// The header: the associated data of the sealed seed.
    pub(crate) fn header(&self) -> [u8; HEADER_LEN] {
        let mut header = [0u8; HEADER_LEN];
        let parts: [&[u8]; 12] = [
            &MAGIC,
            &[FORMAT_VERSION],
            &self.key_version.to_be_bytes(),
            &self.argon2.m_kib.to_be_bytes(),
            &self.argon2.t.to_be_bytes(),
            &self.argon2.p.to_be_bytes(),
            &self.salt,
            &self.nonce,
            &self.account_hash,
            &self.client_id,
            &self.created_at_ms.to_be_bytes(),
            &self.refund_key,
        ];
        let mut at = 0;
        for part in parts {
            header[at..at + part.len()].copy_from_slice(part);
            at += part.len();
        }
        debug_assert_eq!(at, HEADER_LEN, "the header is filled");
        header
    }

    /// The bytes the embedder keeps.
    pub(crate) fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(HEADER_LEN + SEALED_LEN);
        bytes.extend_from_slice(&self.header());
        bytes.extend_from_slice(&self.sealed);
        bytes
    }

    /// Read recovery data.
    ///
    /// # Errors
    /// `InvalidRecoveryData`: `notRecoveryData` for bytes that are not
    /// recovery data (another magic, another length), `unsupported` for
    /// another format version.
    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, KupferdrahtError> {
        if bytes.len() < MAGIC.len() + 1 || bytes[..MAGIC.len()] != MAGIC {
            return Err(invalid("notRecoveryData"));
        }
        if bytes[MAGIC.len()] != FORMAT_VERSION {
            return Err(invalid("unsupported"));
        }
        if bytes.len() != HEADER_LEN + SEALED_LEN {
            return Err(invalid("notRecoveryData"));
        }
        let mut reader = Reader {
            bytes,
            at: MAGIC.len() + 1,
        };
        let data = Self {
            key_version: reader.u32(),
            argon2: Argon2Params {
                m_kib: reader.u32(),
                t: reader.u32(),
                p: reader.u32(),
            },
            salt: reader.array(),
            nonce: reader.array(),
            account_hash: reader.array(),
            client_id: reader.array(),
            created_at_ms: reader.u64(),
            refund_key: reader.array(),
            sealed: reader.array(),
        };
        debug_assert_eq!(reader.at, bytes.len(), "every byte is read");
        Ok(data)
    }
}

/// Reads the fixed fields of recovery data whose length was checked.
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn array<const N: usize>(&mut self) -> [u8; N] {
        let mut out = [0u8; N];
        out.copy_from_slice(&self.bytes[self.at..self.at + N]);
        self.at += N;
        out
    }

    fn u32(&mut self) -> u32 {
        u32::from_be_bytes(self.array())
    }

    fn u64(&mut self) -> u64 {
        u64::from_be_bytes(self.array())
    }
}

/// A Wire client id as the header carries it: its text, zero-padded.
///
/// # Errors
/// `InvalidInput` for an empty id, one longer than 16 bytes, or one with a
/// zero byte.
pub(crate) fn client_id_field(client_id: &str) -> Result<[u8; CLIENT_ID_LEN], KupferdrahtError> {
    let bytes = client_id.as_bytes();
    if bytes.is_empty() || bytes.len() > CLIENT_ID_LEN || bytes.contains(&0) {
        return Err(KupferdrahtError::InvalidInput(format!(
            "{client_id:?} is not a Wire client id (1–16 bytes)"
        )));
    }
    let mut field = [0u8; CLIENT_ID_LEN];
    field[..bytes.len()].copy_from_slice(bytes);
    Ok(field)
}

/// The client id a header carries, as text.
#[cfg(test)]
pub(crate) fn client_id_text(field: &[u8; CLIENT_ID_LEN]) -> String {
    let end = field.iter().position(|&b| b == 0).unwrap_or(CLIENT_ID_LEN);
    String::from_utf8_lossy(&field[..end]).into_owned()
}

/// SHA-256 of the account's qualified id — what the header carries.
pub(crate) fn account_hash(account_id: &str) -> [u8; 32] {
    use sha2::Digest as _;
    sha2::Sha256::digest(account_id.as_bytes()).into()
}

/// `InvalidRecoveryData` with `reason`.
pub(crate) fn invalid(reason: &str) -> KupferdrahtError {
    KupferdrahtError::InvalidRecoveryData {
        reason: reason.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> RecoveryData {
        RecoveryData {
            key_version: 3,
            argon2: Argon2Params {
                m_kib: 65_536,
                t: 3,
                p: 1,
            },
            salt: [0x11; 16],
            nonce: [0x22; 12],
            account_hash: account_hash("user-uuid@example.test"),
            client_id: client_id_field("db8ad281d4aa66").expect("a client id"),
            created_at_ms: 1_791_000_000_000,
            refund_key: [0x44; REFUND_KEY_LEN],
            sealed: [0x33; SEALED_LEN],
        }
    }

    fn reason(result: Result<RecoveryData, KupferdrahtError>) -> String {
        match result {
            Err(KupferdrahtError::InvalidRecoveryData { reason }) => reason,
            other => panic!("expected InvalidRecoveryData, got {other:?}"),
        }
    }

    /// Step 120 (ADR-0040): the header carries the device and when the data
    /// was made; Step 124: version 3 adds the receipt key — all of it the
    /// associated data.
    #[test]
    fn version_three_carries_the_device_its_creation_and_the_refund_key() {
        let data = sample();
        let bytes = data.encode();
        assert_eq!(FORMAT_VERSION, 3);
        assert_eq!(HEADER_LEN, 138);
        assert_eq!(bytes.len(), 186);
        assert_eq!(&bytes[81..97], b"db8ad281d4aa66\0\0");
        assert_eq!(&bytes[97..105], &1_791_000_000_000_u64.to_be_bytes());
        assert_eq!(&bytes[105..138], &[0x44; REFUND_KEY_LEN]);
        let decoded = RecoveryData::decode(&bytes).expect("decoded");
        assert_eq!(decoded.client_id, data.client_id);
        assert_eq!(decoded.created_at_ms, data.created_at_ms);
        assert_eq!(decoded.refund_key, data.refund_key);
    }

    /// No migration: data of versions 1 and 2 is refused — the PIN service
    /// keys its evaluation by the refund key since Step 124, so it could
    /// not be opened anyway.
    #[test]
    fn versions_one_and_two_are_unsupported() {
        for version in [1, 2] {
            let mut bytes = sample().encode();
            bytes[4] = version;
            assert_eq!(
                reason(RecoveryData::decode(&bytes)),
                "unsupported",
                "{version}"
            );
        }
    }

    /// The id is kept as text — whatever Wire's ids look like, up to 16
    /// bytes.
    #[test]
    fn client_ids_round_trip_as_text() {
        for id in ["db8ad281d4aa66", "fc100a0123ef263d", "1", "new-client-id"] {
            let field = client_id_field(id).expect("a client id");
            assert_eq!(client_id_text(&field), id);
        }
    }

    #[test]
    fn anything_else_is_not_a_client_id() {
        for id in ["", "fc100a0123ef263d0", "a\0b"] {
            assert!(
                matches!(client_id_field(id), Err(KupferdrahtError::InvalidInput(_))),
                "{id:?}"
            );
        }
    }

    #[test]
    fn recovery_data_round_trips() {
        let data = sample();
        let bytes = data.encode();
        assert_eq!(bytes.len(), HEADER_LEN + SEALED_LEN);
        assert_eq!(&bytes[..4], b"SDRD");
        assert_eq!(bytes[4], FORMAT_VERSION);
        assert_eq!(&bytes[..HEADER_LEN], &data.header());
        assert_eq!(RecoveryData::decode(&bytes).expect("decoded"), data);
    }

    #[test]
    fn foreign_bytes_are_not_recovery_data() {
        assert_eq!(
            reason(RecoveryData::decode(b"not recovery data")),
            "notRecoveryData"
        );
        let mut bytes = sample().encode();
        bytes[0] = b'X';
        assert_eq!(reason(RecoveryData::decode(&bytes)), "notRecoveryData");
    }

    #[test]
    fn a_truncated_or_padded_file_is_not_recovery_data() {
        let bytes = sample().encode();
        assert_eq!(
            reason(RecoveryData::decode(&bytes[..bytes.len() - 1])),
            "notRecoveryData"
        );
        let mut longer = bytes.clone();
        longer.push(0);
        assert_eq!(reason(RecoveryData::decode(&longer)), "notRecoveryData");
    }

    #[test]
    fn another_format_version_is_unsupported() {
        let mut bytes = sample().encode();
        bytes[4] = FORMAT_VERSION + 1;
        assert_eq!(reason(RecoveryData::decode(&bytes)), "unsupported");
    }

    /// SHA-256 of "user-uuid@example.test", computed independently.
    #[test]
    fn the_account_hash_is_sha256_of_the_account_id() {
        assert_eq!(
            account_hash("user-uuid@example.test"),
            [
                0xb7, 0x0b, 0xb4, 0xdb, 0x35, 0xab, 0x65, 0xbd, 0x17, 0xaa, 0x40, 0x26, 0xa3, 0x14,
                0xf5, 0x99, 0x94, 0x80, 0xd4, 0x67, 0x80, 0xbf, 0x55, 0x60, 0xd0, 0xd8, 0x0f, 0xd1,
                0xae, 0xca, 0xc0, 0xb1
            ]
        );
    }
}
