// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `KeyringStore` — desktop default `SecureKeyStore` backed by the
//! `keyring` crate (macOS Keychain, Windows Credential Manager; on
//! Linux only the kernel keyring, which does not survive a reboot —
//! open.md §71 B4).
//!
//! The database key material (a raw 32-byte key, ADR-0021) is stored
//! **directly** under a Kupferdraht-namespaced service id — no KEK, no
//! wrapping. Mobile and Web
//! go through a `UniFFI` callback variant. The keyring crate's API is
//! desktop-only — calling into it on a headless / no-keyring
//! environment surfaces the error, it does not panic.

use base64::Engine as _;

use crate::error::KupferdrahtError;
use crate::keystore::SecureKeyStore;

/// `SecureKeyStore` backed by the desktop OS keyring. The database key
/// lives in the keyring under a Kupferdraht-namespaced service id.
pub struct KeyringStore {
    /// Service-name prefix written to the keyring; the secret id is the
    /// keyring "account" field. Kupferdraht-CLI uses `"kupferdraht"`; tests can
    /// construct with a unique prefix to avoid colliding with a running
    /// install.
    service_prefix: String,
}

impl KeyringStore {
    /// Construct a `KeyringStore` with the given keyring service prefix
    /// (typically `"kupferdraht"`).
    #[must_use]
    pub fn new(service_prefix: impl Into<String>) -> Self {
        Self {
            service_prefix: service_prefix.into(),
        }
    }
}

impl SecureKeyStore for KeyringStore {
    fn get_secret(&self, id: String) -> Result<Option<Vec<u8>>, KupferdrahtError> {
        let entry = keyring::Entry::new(&self.service_prefix, &id)
            .map_err(|e| KupferdrahtError::Generic(format!("keyring open: {e}")))?;
        match entry.get_password() {
            Ok(b64) => Ok(Some(decode_secret(&b64)?)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(KupferdrahtError::Generic(format!(
                "keyring get_password: {e}"
            ))),
        }
    }

    fn set_secret(&self, id: String, secret: Vec<u8>) -> Result<(), KupferdrahtError> {
        let entry = keyring::Entry::new(&self.service_prefix, &id)
            .map_err(|e| KupferdrahtError::Generic(format!("keyring open: {e}")))?;
        let encoded = base64::engine::general_purpose::STANDARD.encode(&secret);
        entry
            .set_password(&encoded)
            .map_err(|e| KupferdrahtError::Generic(format!("keyring set_password: {e}")))
    }

    fn delete_secret(&self, id: String) -> Result<(), KupferdrahtError> {
        let entry = keyring::Entry::new(&self.service_prefix, &id)
            .map_err(|e| KupferdrahtError::Generic(format!("keyring open: {e}")))?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(KupferdrahtError::Generic(format!("keyring delete: {e}"))),
        }
    }
}

fn decode_secret(b64: &str) -> Result<Vec<u8>, KupferdrahtError> {
    base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_err(|e| KupferdrahtError::Generic(format!("keyring secret base64: {e}")))
}
