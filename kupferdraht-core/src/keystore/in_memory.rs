// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `InMemoryKeyStore` — test-only `SecureKeyStore` implementation.
//!
//! Holds each secret in a process-local map. Used by integration tests
//! and by `kupferdraht-cli` smoke runs that don't want to touch the real
//! desktop keyring (CI, scratch installs, dual-instance smoke
//! sessions).

use std::collections::HashMap;
use std::sync::Mutex;

use crate::error::KupferdrahtError;
use crate::keystore::SecureKeyStore;

/// Process-local `SecureKeyStore`. Two distinct instances share
/// nothing — a secret stored in one is invisible to the other (the
/// test suite relies on that for
/// `a_second_keystore_instance_does_not_see_anothers_secret`).
pub struct InMemoryKeyStore {
    secrets: Mutex<HashMap<String, Vec<u8>>>,
}

impl InMemoryKeyStore {
    /// Construct an empty `InMemoryKeyStore`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            secrets: Mutex::new(HashMap::new()),
        }
    }
}

impl Default for InMemoryKeyStore {
    fn default() -> Self {
        Self::new()
    }
}

impl SecureKeyStore for InMemoryKeyStore {
    fn get_secret(&self, id: String) -> Result<Option<Vec<u8>>, KupferdrahtError> {
        let guard = self
            .secrets
            .lock()
            .map_err(|e| KupferdrahtError::Generic(format!("InMemoryKeyStore mutex: {e}")))?;
        Ok(guard.get(&id).cloned())
    }

    fn set_secret(&self, id: String, secret: Vec<u8>) -> Result<(), KupferdrahtError> {
        let mut guard = self
            .secrets
            .lock()
            .map_err(|e| KupferdrahtError::Generic(format!("InMemoryKeyStore mutex: {e}")))?;
        guard.insert(id, secret);
        Ok(())
    }

    fn delete_secret(&self, id: String) -> Result<(), KupferdrahtError> {
        let mut guard = self
            .secrets
            .lock()
            .map_err(|e| KupferdrahtError::Generic(format!("InMemoryKeyStore mutex: {e}")))?;
        guard.remove(&id);
        Ok(())
    }
}
