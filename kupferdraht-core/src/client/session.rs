// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `Client` — session lifecycle and getters.
//!
//! The session-management slice of the `Client` runtime (Step 29): the
//! read-only getters that mirror `matrix_sdk::Client` (`user_id`,
//! `device_id`, `access_token`, …), `logout`, and the two MLS-key
//! accessors. The in-session token refresh — the `TokenRefresh`
//! seam, the refresh / swap / persist path and its cross-process lock —
//! lives in `token_refresh` (Steps 39 + 85). Public API surface mirrors
//! `matrix_sdk::Client` — see `docs/api-mapping-matrix-rust-sdk.md`.

#![allow(clippy::doc_markdown)]

use std::sync::Arc;

use crate::client::Account;
use crate::error::KupferdrahtError;
use crate::identity::UserId;
use crate::storage::{StorageEngineInfo, Store};

use super::Client;

impl Client {
    /// The app-data store this client persists to, or `None` for a
    /// metadata-only / in-memory session. Lets a consumer reach the
    /// store the SDK opened (via the `StorageProvider` / `base_dir`) for
    /// direct reads the high-level API does not wrap.
    #[must_use]
    pub fn store(&self) -> Option<Arc<dyn Store>> {
        self.store.clone()
    }

    /// Engine, cipher scheme and header mode of the app-data store's
    /// whole-DB encryption, read off the live connection (Step 90,
    /// ADR-0021) — what `kupferdraht-cli status` prints. `None` for a
    /// metadata-only / in-memory session or a backend that reports no
    /// engine (a custom `StorageProvider`).
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` if the store cannot answer.
    pub async fn storage_engine(&self) -> Result<Option<StorageEngineInfo>, KupferdrahtError> {
        match self.store() {
            Some(store) => store.storage_engine().await,
            None => Ok(None),
        }
    }

    /// Sign this client out of Wire. Step 6e-2; matrix-rust-sdk
    /// equivalent: `Client::logout`.
    ///
    /// Pipeline:
    /// 1. If `purge_device == true`, **first** delete the registered
    ///    device via `DELETE /v14/clients/{device_id}` — Wire requires
    ///    the user's password for this (Step 1d's surface). Doing this
    ///    *before* the cookie-revocation step ensures the access token
    ///    still works for the device-delete authorization.
    /// 2. `POST /v14/access/logout` — revoke the `zuid` refresh
    ///    cookie server-side. The access token itself expires
    ///    naturally within its ~15 min TTL; no further `/access`
    ///    rebinds are possible after this.
    /// 3. (Caller — typically the CLI — optionally wipes the local
    ///    `~/.kupferdraht/<account>/` directory afterwards. The library
    ///    does not touch local storage in `logout` — that is the
    ///    caller's policy decision, and would mean discarding the
    ///    `MlsClient` and `SqliteStore` handles which Kupferdraht does
    ///    not own from `&self`.)
    ///
    /// `purge_device == false` keeps the device registered on Wire
    /// (so a subsequent `login` reuses it via Step 7e's reuse path)
    /// and only kills the current refresh-cookie session. Useful
    /// for "logout but next login on the same machine should be
    /// fast and not eat a 7-cap slot".
    ///
    /// `password` is required iff `purge_device == true`. Empty
    /// password with `purge_device == true` returns `InvalidInput`.
    ///
    /// # Errors
    /// * `InvalidInput` if `purge_device` is true and `password` is
    ///   `None` / empty.
    /// * `BackendRejected` on non-2xx from the device deletion
    ///   (wrong password, missing client), or — without
    ///   `purge_device` — from `/access/logout` (already-revoked
    ///   cookie). **With** `purge_device`, a failing trailing
    ///   `/access/logout` is NOT an error: deleting the device
    ///   invalidates the session server-side, so Wire rejects the
    ///   still-held token ("Invalid zauth token") — the session died
    ///   with the device (live-verified).
    /// * Network failures bubble up untyped.
    pub async fn logout(
        &self,
        purge_device: bool,
        password: Option<&str>,
    ) -> Result<(), KupferdrahtError> {
        if purge_device {
            let pw = password.unwrap_or("");
            if pw.is_empty() {
                return Err(KupferdrahtError::InvalidInput(
                    "logout: --purge-device requires a non-empty password".into(),
                ));
            }
            self.http
                .delete_client(
                    self.api_version,
                    &self.current_access_token(),
                    &self.device_id,
                    Some(pw),
                )
                .await?;
            // Best-effort from here: the deletion above already killed
            // this session server-side, so the cookie revocation can
            // only fail because it is no longer needed.
            if let Err(e) = self
                .http
                .logout(
                    self.api_version,
                    &self.current_access_token(),
                    &self.current_refresh_token(),
                )
                .await
            {
                tracing::warn!(
                    error = ?e,
                    "post-purge /access/logout failed (session already dead with the device) — ignored"
                );
            }
            return Ok(());
        }
        self.http
            .logout(
                self.api_version,
                &self.current_access_token(),
                &self.current_refresh_token(),
            )
            .await
    }

    /// Close the account's two databases (Step 114, a browser purge): the
    /// store refuses every later operation with `SessionExpired`, and MLS
    /// operations find no group. The client is done afterwards.
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` when a database cannot be closed.
    #[cfg(any(target_arch = "wasm32", test))]
    pub(crate) fn close_storage(&self) -> Result<(), KupferdrahtError> {
        if let Some(store) = &self.store {
            store.close()?;
        }
        if let Some(mls) = &self.mls_client {
            crate::encryption::lock_mls(mls).close_storage()?;
        }
        Ok(())
    }

    /// Wire user id (`<uuid>@<domain>`).
    ///
    /// Returns `Option` to mirror matrix-rust-sdk's `Client::user_id`,
    /// which is `Option<&UserId>` because the type permits a
    /// not-yet-logged-in state. In Kupferdraht an `Arc<Client>` is only
    /// produced via successful `ClientBuilder::login`, so this getter
    /// always returns `Some` today — the `Option` shape mirrors
    /// matrix-rust-sdk.
    #[must_use]
    pub fn user_id(&self) -> Option<&UserId> {
        Some(&self.user_id)
    }

    /// Wire device id (the hexadecimal `client_id` returned from
    /// `POST /v14/clients`).
    ///
    /// Optional for the same reason as `user_id`.
    #[must_use]
    pub fn device_id(&self) -> Option<&str> {
        Some(&self.device_id)
    }

    /// Whether this client's device replaced the one recorded for the
    /// account, because the backend no longer had it (Step 40's device
    /// reuse): the superseded MLS state and history were wiped. Step 110a.
    #[must_use]
    pub(crate) fn device_replaced(&self) -> bool {
        self.device_replaced
            .load(std::sync::atomic::Ordering::Acquire)
    }

    /// Record that this client's device replaced the recorded one — the
    /// login that registered it calls this once.
    pub(crate) fn mark_device_replaced(&self) {
        debug_assert!(!self.device_replaced(), "a device is replaced once");
        self.device_replaced
            .store(true, std::sync::atomic::Ordering::Release);
    }

    /// API base URL of the Wire backend (HTTPS).
    ///
    /// matrix-rust-sdk equivalent: `Client::homeserver()`.
    #[must_use]
    pub fn homeserver_url(&self) -> &str {
        &self.server.api_base_url
    }

    /// WebSocket base URL of the Wire backend (WSS).
    ///
    /// Wire-specific extension — matrix-rust-sdk has no separate
    /// WebSocket URL because Matrix multiplexes everything over the
    /// homeserver.
    #[must_use]
    pub fn websocket_url(&self) -> &str {
        &self.server.websocket_url
    }

    /// Human-readable backend title (e.g. `"nginz-https.example.org"`).
    ///
    /// Wire-specific. matrix-rust-sdk has no equivalent.
    #[must_use]
    pub fn server_title(&self) -> &str {
        &self.server.title
    }

    /// Negotiated Wire API version. ADR-0008 pins this to `14`.
    ///
    /// Kupferdraht-specific — Matrix has no analogous numeric versioning.
    #[must_use]
    pub fn api_version(&self) -> u32 {
        self.api_version
    }

    /// Current access token, owned. Returns `Option` to mirror
    /// matrix-rust-sdk's `Client::access_token`. In Kupferdraht today it is
    /// always `Some` (an `Arc<Client>` only exists post-login).
    #[must_use]
    pub fn access_token(&self) -> Option<String> {
        Some(self.current_access_token())
    }

    /// Snapshot of the current access token (read-lock + clone). The
    /// owned `String` lives long enough to be borrowed across the
    /// subsequent HTTP `.await` at the call site.
    pub(super) fn current_access_token(&self) -> String {
        self.tokens
            .read()
            .map(|t| (*t.access_token).clone())
            .unwrap_or_default()
    }

    /// Wire's `zuid` refresh-cookie value. The CLI persists this in
    /// `SessionRecord.refresh_token` so a later `kupferdraht-cli sync`
    /// can drive `ClientBuilder::resume` without the password.
    ///
    /// Wire-specific extension — matrix-rust-sdk has no equivalent.
    /// Returned as `Option` for symmetry with `access_token`.
    #[must_use]
    pub fn refresh_token(&self) -> Option<String> {
        Some(self.current_refresh_token())
    }

    /// Snapshot of the current `zuid` refresh cookie (read-lock + clone).
    pub(super) fn current_refresh_token(&self) -> String {
        self.tokens
            .read()
            .map(|t| (*t.refresh_token).clone())
            .unwrap_or_default()
    }

    pub(super) fn token_issued_at_ms(&self) -> i64 {
        self.tokens.read().map_or(0, |t| t.issued_at_ms)
    }

    /// User-profile accessor. Mirrors `matrix_sdk::Client::account`.
    #[must_use]
    pub fn account(&self) -> Arc<Account> {
        self.account.clone()
    }

    /// MLS signature public key for this device (P-256 / ECDSA).
    /// Raw bytes, matching what is persisted in openmls's storage and
    /// what `MlsClient::resume_with_storage` expects as a lookup key.
    ///
    /// Wire-specific extension. Callers that persist the device record
    /// use this to populate `devices.mls_public_key` so a subsequent
    /// process start can resume the MLS identity without hitting the
    /// backend.
    #[must_use]
    pub fn mls_signature_public_key(&self) -> &[u8] {
        &self.mls_signature_public_key
    }

    /// Wire's backend external-sender public key for the P-256
    /// signature scheme (`removal` purpose). Raw bytes, base64-decoded
    /// from the Wire response.
    ///
    /// Fetched during login from `GET /v14/mls/public-keys` and embedded
    /// into every MLS group's `external_senders` extension so
    /// admin-driven Remove proposals signed by the backend validate
    /// (ADR-0004, `docs/wire/mls-deviations.md` W-5).
    ///
    /// Wire-specific extension — matrix-rust-sdk has no equivalent.
    #[must_use]
    pub fn backend_removal_key_ecdsa_p256(&self) -> &[u8] {
        &self.backend_removal_key_ecdsa_p256
    }
}
