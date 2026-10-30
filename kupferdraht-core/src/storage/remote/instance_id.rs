// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! The VFS instance id of an account's databases (Step 111d): stable per
//! browser home and account, so a reload, a crash or a leader change
//! between tabs takes its own lease on the storage server back at once —
//! the server grants a lease without a takeover to the id that holds it.
//! Not secret: it opens nothing without the storage-login key. Derived,
//! never stored — the home id already is.

/// The frozen label of the derivation.
const STORAGE_INSTANCE_LABEL: &[u8] = b"kupferdraht/v1/storage-instance";

/// The instance id of `account_id`'s databases in the home `home_id`: the
/// first 16 bytes of SHA-256 over the label, the home id and the account
/// id, each prefixed with its length (32-bit big-endian).
pub(crate) fn storage_instance_id(home_id: &str, account_id: &str) -> [u8; 16] {
    use sha2::{Digest, Sha256};
    debug_assert!(!home_id.is_empty(), "an id needs its home id");
    let mut hasher = Sha256::new();
    for part in [
        STORAGE_INSTANCE_LABEL,
        home_id.as_bytes(),
        account_id.as_bytes(),
    ] {
        let len = u32::try_from(part.len()).unwrap_or(u32::MAX);
        hasher.update(len.to_be_bytes());
        hasher.update(part);
    }
    let digest = hasher.finalize();
    let mut id = [0u8; 16];
    id.copy_from_slice(&digest[..16]);
    id
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOME: &str = "0123456789abcdef0123456789abcdef";
    const ACCOUNT: &str = "user-uuid@example.test";

    /// Computed independently of this crate (Python `hashlib`).
    #[test]
    fn the_id_is_the_known_answer() {
        assert_eq!(
            storage_instance_id(HOME, ACCOUNT),
            [
                0xb7, 0x49, 0x2d, 0x79, 0xf7, 0x24, 0x6d, 0xaf, 0x01, 0x7c, 0x29, 0x7a, 0x61, 0xff,
                0xac, 0x37
            ]
        );
    }

    #[test]
    fn the_id_is_stable() {
        assert_eq!(
            storage_instance_id(HOME, ACCOUNT),
            storage_instance_id(HOME, ACCOUNT)
        );
    }

    /// Another browser (another home) or another account is another
    /// instance: it still needs the explicit takeover.
    #[test]
    fn another_home_or_account_is_another_instance() {
        let id = storage_instance_id(HOME, ACCOUNT);
        assert_ne!(
            id,
            storage_instance_id("fedcba9876543210fedcba9876543210", ACCOUNT)
        );
        assert_ne!(id, storage_instance_id(HOME, "other-uuid@example.test"));
    }

    /// The length prefixes keep the parts apart: moving a character from
    /// the home id to the account id is another id.
    #[test]
    fn the_parts_do_not_run_into_each_other() {
        assert_ne!(
            storage_instance_id("ab", "c"),
            storage_instance_id("a", "bc")
        );
    }
}
