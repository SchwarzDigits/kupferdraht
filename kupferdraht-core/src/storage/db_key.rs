// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Per-database key-material type + generation.
//!
//! [`DbKey`] is the raw 32-byte key fed to `SQLite3MC`'s `chacha20` scheme
//! (ChaCha20-Poly1305, ADR-0021): the 256-bit key is the whole security
//! parameter — no KDF, no salt, no HMAC subkey in the trust path. **Each
//! database file has its own `DbKey`** (`kupferdraht.db`, `mls.db`, `global.db`
//! are independently keyed). At rest the 32 bytes live **directly** in the
//! platform keystore (`keystore::SecureKeyStore`); this module owns only
//! the in-memory type, its serialization, and random generation.
//!
//! The pre-Step-90 48-byte `key ‖ salt` blob of the `SQLCipher` era is
//! **not** accepted (hard 32: Kupferdraht was deployed nowhere when the
//! engine changed, so there was nothing to migrate); a consumer that
//! stored one re-provisions.

use rand::RngCore;
use rand::rngs::OsRng;
use zeroize::Zeroizing;

use crate::error::KupferdrahtError;

/// Length of the raw `ChaCha20` key — the whole of [`DbKey`].
pub const KEY_LEN: usize = 32;
/// Key-material length on the keystore wire: the 32-byte key alone.
pub const MATERIAL_LEN: usize = KEY_LEN;

/// Per-database key material — a raw 32-byte `ChaCha20` key (secret,
/// `Zeroizing`). Wiped on drop (ADR-0006 / `THREAT_MODEL.md` G4).
#[derive(Clone)]
pub struct DbKey {
    key: Zeroizing<[u8; KEY_LEN]>,
}

impl DbKey {
    /// The 32-byte raw key.
    #[must_use]
    pub fn key(&self) -> &[u8; KEY_LEN] {
        &self.key
    }

    /// Parse the keystore blob: exactly the 32-byte key. The single place
    /// the material-length invariant is enforced; the blob is runtime data
    /// (a foreign keystore may return anything), so a wrong length — the
    /// pre-Step-90 48-byte `key ‖ salt` blob included — is a recoverable
    /// error, never a panic.
    ///
    /// # Errors
    /// `KupferdrahtError::Generic` if `raw` is not exactly [`MATERIAL_LEN`] bytes.
    pub fn from_keystore_bytes(raw: &[u8]) -> Result<Self, KupferdrahtError> {
        if raw.len() != MATERIAL_LEN {
            return Err(KupferdrahtError::Generic(format!(
                "DB key material must be {MATERIAL_LEN} bytes (raw key), got {}",
                raw.len()
            )));
        }
        let mut key = Zeroizing::new([0u8; KEY_LEN]);
        key.copy_from_slice(&raw[..KEY_LEN]);
        Ok(Self { key })
    }

    /// Serialize to the 32-byte keystore blob, in a `Zeroizing` buffer so
    /// the copy is wiped after the keystore stores it.
    #[must_use]
    pub fn to_keystore_bytes(&self) -> Zeroizing<Vec<u8>> {
        let mut out = Zeroizing::new(Vec::with_capacity(MATERIAL_LEN));
        out.extend_from_slice(&self.key[..]);
        out
    }
}

impl From<Zeroizing<[u8; KEY_LEN]>> for DbKey {
    /// A key derived elsewhere (ADR-0035: from an account's seed).
    fn from(key: Zeroizing<[u8; KEY_LEN]>) -> Self {
        Self { key }
    }
}

/// Generate a fresh random 32-byte key from the OS CSPRNG.
#[must_use]
pub fn generate_db_key() -> DbKey {
    let mut key = Zeroizing::new([0u8; KEY_LEN]);
    OsRng.fill_bytes(&mut *key);
    DbKey { key }
}
