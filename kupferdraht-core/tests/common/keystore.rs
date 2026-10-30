// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Keystore test doubles shared by the cross-process (Step 85) and
//! cross-home (Step 93) tests.
#![allow(dead_code)]

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use kupferdraht::KupferdrahtError;
use kupferdraht::keystore::SecureKeyStore;

/// A keystore that runs a gate closure once — by default before its FIRST
/// `set_secret`, the way a test parks a caller inside the key-creation
/// critical section (the `set` only ever happens under the lock). With
/// [`Self::on_second_read`] the gate runs before the SECOND `get_secret`
/// instead: the re-read `load_or_create_key_locked` does under the lock,
/// which every open without a key reaches — also one ADR-0029 then
/// refuses without writing (Step 96).
pub struct GatedKeyStore<K: SecureKeyStore> {
    inner: K,
    gate: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    sets: AtomicUsize,
    gets: AtomicUsize,
    on_read: bool,
}

impl<K: SecureKeyStore> GatedKeyStore<K> {
    pub fn new(inner: K, gate: Box<dyn FnOnce() + Send>) -> Self {
        Self {
            inner,
            gate: Mutex::new(Some(gate)),
            sets: AtomicUsize::new(0),
            gets: AtomicUsize::new(0),
            on_read: false,
        }
    }

    /// Gate the second `get_secret` instead of the first `set_secret`.
    pub fn on_second_read(inner: K, gate: Box<dyn FnOnce() + Send>) -> Self {
        Self {
            on_read: true,
            ..Self::new(inner, gate)
        }
    }

    fn run_gate(&self) {
        let gate = self.gate.lock().expect("gate mutex").take();
        if let Some(gate) = gate {
            gate();
        }
    }

    /// How many `set_secret` calls the store has seen.
    pub fn sets(&self) -> usize {
        self.sets.load(Ordering::SeqCst)
    }
}

impl<K: SecureKeyStore> SecureKeyStore for GatedKeyStore<K> {
    fn get_secret(&self, id: String) -> Result<Option<Vec<u8>>, KupferdrahtError> {
        if self.on_read && self.gets.fetch_add(1, Ordering::SeqCst) == 1 {
            self.run_gate();
        }
        self.inner.get_secret(id)
    }

    fn set_secret(&self, id: String, secret: Vec<u8>) -> Result<(), KupferdrahtError> {
        self.sets.fetch_add(1, Ordering::SeqCst);
        if !self.on_read {
            self.run_gate();
        }
        self.inner.set_secret(id, secret)
    }

    fn delete_secret(&self, id: String) -> Result<(), KupferdrahtError> {
        self.inner.delete_secret(id)
    }
}

/// How a [`LockableKeyStore`] answers a read while it is locked.
#[derive(Clone, Copy)]
pub enum LockedAnswer {
    /// `Ok(None)` — the misbehaving host: "locked" looks like "never stored".
    Missing,
    /// `Err(..)` — the ADR-0029 contract: a locked store is an error.
    Error,
}

/// A keystore that can be locked the way the iOS Keychain is before the
/// first unlock after a reboot (Step 96): while locked, reads of the ids
/// `hides` selects answer [`LockedAnswer`]; everything else passes through.
/// Counts writes so a test can pin "nothing was written".
pub struct LockableKeyStore<K: SecureKeyStore> {
    inner: K,
    locked: Mutex<Option<LockedAnswer>>,
    hides: Box<dyn Fn(&str) -> bool + Send + Sync>,
    sets: AtomicUsize,
}

impl<K: SecureKeyStore> LockableKeyStore<K> {
    pub fn new(inner: K, hides: Box<dyn Fn(&str) -> bool + Send + Sync>) -> Self {
        Self {
            inner,
            locked: Mutex::new(None),
            hides,
            sets: AtomicUsize::new(0),
        }
    }

    pub fn lock(&self, answer: LockedAnswer) {
        *self.locked.lock().expect("lock state") = Some(answer);
    }

    pub fn unlock(&self) {
        *self.locked.lock().expect("lock state") = None;
    }

    /// How many `set_secret` calls the store has seen.
    pub fn sets(&self) -> usize {
        self.sets.load(Ordering::SeqCst)
    }

    /// The stored secret, bypassing the lock (what the device really holds).
    pub fn stored(&self, id: &str) -> Option<Vec<u8>> {
        self.inner.get_secret(id.to_owned()).expect("inner read")
    }
}

impl<K: SecureKeyStore> SecureKeyStore for LockableKeyStore<K> {
    fn get_secret(&self, id: String) -> Result<Option<Vec<u8>>, KupferdrahtError> {
        let locked = *self.locked.lock().expect("lock state");
        match locked {
            Some(LockedAnswer::Missing) if (self.hides)(&id) => Ok(None),
            Some(LockedAnswer::Error) if (self.hides)(&id) => Err(KupferdrahtError::Generic(
                "keystore locked (test double)".into(),
            )),
            _ => self.inner.get_secret(id),
        }
    }

    fn set_secret(&self, id: String, secret: Vec<u8>) -> Result<(), KupferdrahtError> {
        self.sets.fetch_add(1, Ordering::SeqCst);
        self.inner.set_secret(id, secret)
    }

    fn delete_secret(&self, id: String) -> Result<(), KupferdrahtError> {
        self.inner.delete_secret(id)
    }
}
