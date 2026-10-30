// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! The account's drain lock (Step 97d, ADR-0031): one cursor-advancing
//! dispatch per account at a time — across `Client`s and processes on
//! one account, not only within one `Client` (the Step-14b `sync_lock`
//! already serialises a `Client`'s own sync iterations).
//!
//! Two layers: an async mutex per `Client` (tasks of one process queue
//! without polling), then the advisory file lock
//! `base/accounts/<id>/sync.lock` (`storage::ProcessLock`, native
//! only — the browser has one instance per origin, ADR-0023). Unlike
//! `kupferdraht.lock` a held `sync.lock` is waited for, for a bounded time.
//! The holder only does local work: callers fetch before and run
//! network follow-ups after (never `kupferdraht.lock` while holding this).

#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;

#[cfg(not(target_arch = "wasm32"))]
use crate::storage::ProcessLock;

use crate::client::Client;
use crate::error::KupferdrahtError;

/// How long a drain waits for another holder of `sync.lock` before it
/// gives up with a retryable `AccountLocked`.
#[cfg(not(target_arch = "wasm32"))]
const DRAIN_LOCK_WAIT: Duration = Duration::from_secs(5);

/// Pause between two tries of a held `sync.lock`.
#[cfg(not(target_arch = "wasm32"))]
const DRAIN_LOCK_RETRY: Duration = Duration::from_millis(20);

/// Held while one page (or one push frame) is dispatched; releases both
/// layers on drop.
pub(crate) struct DrainLockGuard<'a> {
    _local: tokio::sync::MutexGuard<'a, ()>,
    #[cfg(not(target_arch = "wasm32"))]
    _process: Option<ProcessLock>,
}

impl Client {
    /// Take the account's drain lock: the in-process mutex, then (native,
    /// with a home) `sync.lock`, waiting up to [`DRAIN_LOCK_WAIT`].
    ///
    /// # Errors
    /// `KupferdrahtError::AccountLocked` when another holder kept `sync.lock`
    /// past the wait (retryable); `KupferdrahtError::Storage` when the lock
    /// file cannot be created.
    pub(crate) async fn lock_drain(&self) -> Result<DrainLockGuard<'_>, KupferdrahtError> {
        let local = self.drain_lock.lock().await;
        #[cfg(not(target_arch = "wasm32"))]
        let process = self.acquire_sync_file_lock().await?;
        Ok(DrainLockGuard {
            _local: local,
            #[cfg(not(target_arch = "wasm32"))]
            _process: process,
        })
    }

    /// `sync.lock` of this account, retried until [`DRAIN_LOCK_WAIT`]
    /// ran out — `None` without a home (no shared file to guard).
    #[cfg(not(target_arch = "wasm32"))]
    async fn acquire_sync_file_lock(&self) -> Result<Option<ProcessLock>, KupferdrahtError> {
        let Some(home) = self.kupferdraht_home.as_ref() else {
            return Ok(None);
        };
        let account_id = self.user_id.as_str();
        let path = home.account_sync_lock(account_id);
        crate::storage::create_parent_dir(&path)?;
        let scope = format!("sync of account {account_id}");
        let deadline = tokio::time::Instant::now() + DRAIN_LOCK_WAIT;
        loop {
            match ProcessLock::try_acquire(&path, &scope) {
                Ok(lock) => return Ok(Some(lock)),
                Err(KupferdrahtError::AccountLocked(holder))
                    if tokio::time::Instant::now() < deadline =>
                {
                    tracing::debug!(%holder, "drain lock held elsewhere; waiting");
                    tokio::time::sleep(DRAIN_LOCK_RETRY).await;
                }
                Err(e) => return Err(e),
            }
        }
    }
}
