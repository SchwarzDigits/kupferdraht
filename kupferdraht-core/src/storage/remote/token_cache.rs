// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! The storage token as the VFS reads it (Step 111b): the VFS asks its
//! token source synchronously — SQLite waits meanwhile — so Kupferdraht keeps
//! the current token here and renews it in the background.

use std::sync::Mutex;

use zeroize::Zeroizing;

/// The current storage token and when it expires (ms since the epoch).
#[derive(Default)]
pub(crate) struct StorageTokenCache {
    current: Mutex<Option<(Zeroizing<String>, i64)>>,
}

impl StorageTokenCache {
    /// Hold `token` until `expires_at_ms`.
    pub(crate) fn set(&self, token: Zeroizing<String>, expires_at_ms: i64) {
        *self
            .current
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some((token, expires_at_ms));
    }

    /// The token, while it is valid at `now_ms`.
    ///
    /// # Errors
    /// A message when there is none or it expired — the VFS then fails the
    /// login as refused.
    pub(crate) fn current(&self, now_ms: i64) -> Result<Zeroizing<String>, String> {
        match &*self
            .current
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            Some((token, expires_at_ms)) if now_ms < *expires_at_ms => Ok(token.clone()),
            Some(_) => Err("the storage token expired".into()),
            None => Err("no storage token yet".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_a_token_there_is_none() {
        assert!(StorageTokenCache::default().current(0).is_err());
    }

    #[test]
    fn a_token_is_served_until_it_expires() {
        let cache = StorageTokenCache::default();
        cache.set(Zeroizing::new("storage.jwt".into()), 10_000);

        assert_eq!(cache.current(9_999).expect("valid").as_str(), "storage.jwt");
        assert!(cache.current(10_000).is_err(), "expired at its expiry");
    }

    #[test]
    fn a_renewed_token_replaces_the_old_one() {
        let cache = StorageTokenCache::default();
        cache.set(Zeroizing::new("old".into()), 10_000);
        cache.set(Zeroizing::new("new".into()), 20_000);

        assert_eq!(cache.current(15_000).expect("valid").as_str(), "new");
    }
}
