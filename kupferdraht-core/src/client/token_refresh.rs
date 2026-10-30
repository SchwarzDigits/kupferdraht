// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `Client` — the in-session access-token refresh (Step 39) and its
//! cross-process critical section (Step 85).
//!
//! The `TokenRefresh` seam the transport calls back into on a
//! `401`, the refresh / swap / persist path, and the account's process
//! lock. Two layers of serialisation:
//!
//! 1. **In process** — `refresh_lock` collapses N concurrent 401s on one
//!    `Client` to a single `/access`; a caller whose `stale_issued_at` is
//!    already behind the current acquisition returns early.
//! 2. **Across processes** — the account's advisory file lock
//!    (`base/accounts/<id>/kupferdraht.lock`, [`ProcessLock`]) makes
//!    "re-read the persisted session → rebind → persist" atomic against
//!    a second process on the same home. Wire keeps one refresh cookie
//!    per `(user, label)` and invalidates it on every `/access`, so two
//!    processes rebinding the same cookie leave the second with a dead
//!    `zuid` — and its `SessionExpired` handling would blank the tokens
//!    the first one just wrote. Under the lock the persisted session is
//!    re-read first: a rotation another process already persisted is
//!    **adopted** instead of rebound. Fail-fast: a held lock is
//!    `KupferdrahtError::AccountLocked` (retryable), never a wait.
//!
//! The resume-time refresh (`resume::refresh_stale_access_token`) is the
//! same critical section and shares [`acquire_account_lock`].

use zeroize::Zeroizing;

use crate::error::KupferdrahtError;
#[cfg(not(target_arch = "wasm32"))]
use crate::storage::{KupferdrahtHome, ProcessLock};
use crate::storage::{SessionRecord, Store};

use super::{Client, TokenState};

/// Bounded retries for persisting the rotated `zuid` before the failure is
/// surfaced rather than swallowed (Step 39).
const PERSIST_MAX_ATTEMPTS: u32 = 3;
const PERSIST_RETRY_BACKOFF: std::time::Duration = std::time::Duration::from_millis(200);

/// Take the account's cross-process lock (Step 85) when a home is
/// configured — `None` for a metadata-only session or a custom
/// `StorageProvider` without a home, where there is no shared file to
/// guard. Native only; the browser has one instance per origin.
///
/// # Errors
/// `KupferdrahtError::AccountLocked` while another process is inside the
/// account's critical section; `KupferdrahtError::Storage` if the lock file
/// cannot be created.
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn acquire_account_lock(
    home: Option<&KupferdrahtHome>,
    account_id: &str,
) -> Result<Option<ProcessLock>, KupferdrahtError> {
    debug_assert!(!account_id.is_empty(), "account id must be non-empty");
    let Some(home) = home else {
        return Ok(None);
    };
    let path = home.account_lock(account_id);
    crate::storage::create_parent_dir(&path)?;
    ProcessLock::try_acquire(&path, &format!("account {account_id}")).map(Some)
}

/// The `Client` is the token-refresh seam the transport calls back into
/// on a `401`. Installed on `self.http` in [`Client::new`] so every
/// authenticated call funnels through one guarded refresh — no per-call
/// wrapping. Keeps the transport ignorant of `Client` (ADR-0009): it
/// only ever sees this trait.
impl crate::wire::http::TokenRefresh for Client {
    fn access_token_issued_at_ms(&self) -> i64 {
        self.token_issued_at_ms()
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn refresh_and_get_token<'a>(
        &'a self,
        stale_issued_at: i64,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<String, KupferdrahtError>> + Send + 'a>,
    > {
        Box::pin(async move {
            self.refresh_access_token(stale_issued_at).await?;
            Ok(self.current_access_token())
        })
    }

    // wasm: same body, no `Send` bound (single-threaded, fetch future is !Send).
    #[cfg(target_arch = "wasm32")]
    fn refresh_and_get_token<'a>(
        &'a self,
        stale_issued_at: i64,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, KupferdrahtError>> + 'a>>
    {
        Box::pin(async move {
            self.refresh_access_token(stale_issued_at).await?;
            Ok(self.current_access_token())
        })
    }
}

impl Client {
    /// Refresh the access token via `POST /v{N}/access` (cookie-auth
    /// with the persisted `zuid`), swap the in-memory token triple, and
    /// persist the rotated `zuid` + new token if a store is attached.
    ///
    /// Serialised in process by `refresh_lock` and across processes by
    /// the account lock (module docs). Under the lock a rotation another
    /// process persisted is adopted instead of rebound.
    ///
    /// # Errors
    /// * `AccountLocked` while another process holds the account lock
    ///   (retry).
    /// * `SessionExpired` when Wire rejects the cookie — the persisted
    ///   tokens are cleared and the caller must re-login.
    /// * `Network` / `Encoding` from the `/access` call.
    /// * `Generic` if the token lock is poisoned.
    async fn refresh_access_token(&self, stale_issued_at: i64) -> Result<(), KupferdrahtError> {
        let _guard = self.refresh_lock.lock().await;
        if self.token_issued_at_ms() > stale_issued_at {
            return Ok(());
        }
        // wasm has no process lock — one instance per origin (ADR-0023).
        #[cfg(not(target_arch = "wasm32"))]
        let _process_lock =
            acquire_account_lock(self.kupferdraht_home.as_ref(), self.user_id.as_str())?;
        let stale_access_token = self.current_access_token();
        if let Some(persisted) = self
            .newer_persisted_session(stale_issued_at, &stale_access_token)
            .await?
        {
            tracing::debug!("token refresh: adopting a rotation another process persisted");
            return self.swap_tokens(
                &persisted.access_token,
                &persisted.refresh_token,
                persisted.access_token_issued_at_ms,
            );
        }
        let refresh_token = self.current_refresh_token();
        let rebound = match self
            .http
            .rebind_access_token(self.api_version, &refresh_token, &self.device_id)
            .await
        {
            Ok(rebound) => rebound,
            Err(KupferdrahtError::SessionExpired) => {
                // The zuid is dead. Clear the persisted auth tokens — keeping
                // device_id + mls.db + chats so a re-login reuses the device —
                // so a later resume can't retry a dead cookie, then surface the
                // terminal error for the caller to drive a re-login. Step 39.
                self.clear_persisted_tokens().await;
                return Err(KupferdrahtError::SessionExpired);
            }
            Err(e) => return Err(e),
        };
        let issued_at = crate::time::now_ms();
        self.swap_tokens(&rebound.access_token, &rebound.refresh_token, issued_at)?;
        self.persist_refreshed_session(&rebound.access_token, &rebound.refresh_token, issued_at)
            .await?;
        Ok(())
    }

    /// The persisted session if another process refreshed it after the
    /// caller's token was issued (Step 85): a strictly newer
    /// `access_token_issued_at_ms` **and** a different, non-empty access
    /// token. The stamp alone is not a rotation — a just-logged-in
    /// client's row is stamped after the client was built and carries the
    /// very token that just got the 401; the differing token is the
    /// rotation, the newer stamp keeps a stale row (a failed persist of
    /// our own newer tokens) from being adopted. `None` without a store
    /// or a row.
    async fn newer_persisted_session(
        &self,
        stale_issued_at: i64,
        stale_access_token: &str,
    ) -> Result<Option<SessionRecord>, KupferdrahtError> {
        let Some(store) = self.store.as_ref() else {
            return Ok(None);
        };
        let persisted = store.load_session(self.user_id.as_str()).await?;
        Ok(persisted.filter(|record| {
            record.access_token_issued_at_ms > stale_issued_at
                && record.access_token != stale_access_token
                && !record.access_token.is_empty()
                && !record.refresh_token.is_empty()
        }))
    }

    /// Replace the in-memory token triple (the old `Zeroizing` buffers
    /// are wiped on drop).
    fn swap_tokens(
        &self,
        access: &str,
        refresh: &str,
        issued_at_ms: i64,
    ) -> Result<(), KupferdrahtError> {
        let mut guard = self
            .tokens
            .write()
            .map_err(|e| KupferdrahtError::Generic(format!("token lock: {e}")))?;
        *guard = TokenState {
            access_token: Zeroizing::new(access.to_string()),
            refresh_token: Zeroizing::new(refresh.to_string()),
            issued_at_ms,
        };
        Ok(())
    }

    /// Persist the refreshed session, retrying a bounded number of times
    /// before surfacing the failure. The rotated `zuid` **must** reach disk:
    /// Wire invalidated the old cookie the moment `/access` returned, so a
    /// stale on-disk `zuid` means the next process resume sends a dead cookie
    /// (`403`). A transient write failure usually clears on retry; a
    /// persistent one is returned rather than swallowed, so it can't
    /// silently rot until the next resume. The in-memory session stays valid
    /// regardless (the caller already swapped the new tokens in). Step 39.
    async fn persist_refreshed_session(
        &self,
        access: &str,
        refresh: &str,
        issued_at_ms: i64,
    ) -> Result<(), KupferdrahtError> {
        let Some(store) = self.store.as_ref() else {
            return Ok(());
        };
        let mut attempt = 1u32;
        loop {
            match self
                .write_refreshed_session(store.as_ref(), access, refresh, issued_at_ms)
                .await
            {
                Ok(()) => return Ok(()),
                Err(e) if attempt < PERSIST_MAX_ATTEMPTS => {
                    tracing::warn!(
                        error = %e,
                        attempt,
                        "token refresh: persisting the rotated zuid failed; retrying"
                    );
                    attempt += 1;
                    crate::time::sleep(PERSIST_RETRY_BACKOFF).await;
                }
                Err(e) => {
                    tracing::error!(
                        error = %e,
                        "token refresh: persisting the rotated zuid failed after retries; \
                         the on-disk session is stale and a later resume will need re-login"
                    );
                    return Err(e);
                }
            }
        }
    }

    /// Blank the persisted auth tokens (`access_token` + `refresh_token`) while
    /// keeping `device_id`, `cookie_label`, and everything else — so a later
    /// `resume` can't replay a dead `zuid`, but the device + its `mls.db` +
    /// chats survive for re-login's reuse path. Best-effort: a failure leaves
    /// the dead cookie on disk, which only means the next resume re-derives the
    /// same terminal `SessionExpired`. Step 39.
    async fn clear_persisted_tokens(&self) {
        let Some(store) = self.store.as_ref() else {
            return;
        };
        match store.load_session(self.user_id.as_str()).await {
            Ok(Some(mut record)) => {
                record.access_token = String::new();
                record.refresh_token = String::new();
                if let Err(e) = store.save_session(&record).await {
                    tracing::warn!(error = %e, "session expired: clearing persisted tokens failed");
                }
            }
            Ok(None) => {}
            Err(e) => {
                tracing::warn!(error = %e, "session expired: loading the session to clear it failed");
            }
        }
    }

    /// Load-then-mutate the persisted session so `cookie_label` is
    /// preserved; build a fresh row if none exists yet.
    async fn write_refreshed_session(
        &self,
        store: &dyn Store,
        access: &str,
        refresh: &str,
        issued_at_ms: i64,
    ) -> Result<(), KupferdrahtError> {
        let mut record = store
            .load_session(self.user_id.as_str())
            .await?
            .unwrap_or_else(|| SessionRecord {
                user_id: self.user_id.as_str().to_string(),
                device_id: self.device_id.clone(),
                access_token: String::new(),
                refresh_token: String::new(),
                cookie_label: Some(format!("kupferdraht-{}", self.user_id)),
                api_version: self.api_version,
                access_token_issued_at_ms: issued_at_ms,
            });
        record.access_token = access.to_string();
        record.refresh_token = refresh.to_string();
        record.access_token_issued_at_ms = issued_at_ms;
        store.save_session(&record).await
    }
}
