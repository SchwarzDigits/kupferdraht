// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `Client` — self-profile mutators.
//!
//! The profile-management slice of the `Client` runtime (Step 29):
//! `set_display_name`, `set_handle`, `update_email`, and
//! `change_password`. Each lives on `Client` (matching the
//! `Client::create_room` / `Client::invite_members` pattern); the
//! matrix-rust-sdk analogs live on `Account`.
//!
//! The two that change a field this SDK also stores — the handle and the
//! display name — are transport **plus** a write to the persisted
//! `self_user` row (Step 126). Without that the change would be invisible to
//! this SDK until the next login: `Account` is immutable and `resume`
//! rebuilds it from that row.

#![allow(clippy::doc_markdown)]

use crate::error::KupferdrahtError;
use crate::storage::SelfUserRecord;

use super::Client;

impl Client {
    /// `PUT /v{N}/self` with a `name` body — change the
    /// authenticated user's display name. matrix-rust-sdk equivalent:
    /// `Account::set_display_name`. Kupferdraht puts the mutator on
    /// `Client` to match the existing `Client::create_room` /
    /// `Client::invite_members` pattern (transport calls live on
    /// `Client`, `Account` and `Room` are thin handles).
    ///
    /// The local `Account` snapshot returned by `Client::account()`
    /// reflects the value at login time; call `sync_metadata` to
    /// refresh it after a successful `set_display_name`.
    ///
    /// # Errors
    /// * `InvalidInput` if `name` is empty.
    /// * `BackendRejected` on non-2xx from Wire.
    /// * Network / encoding failures bubble up untyped.
    pub async fn set_display_name(&self, name: &str) -> Result<(), KupferdrahtError> {
        if name.is_empty() {
            return Err(KupferdrahtError::InvalidInput(
                "set_display_name: name must be non-empty".into(),
            ));
        }
        self.http
            .update_self_profile_name(self.api_version, &self.current_access_token(), name)
            .await?;
        self.remember_self_profile(None, Some(name)).await
    }

    /// `PUT /v{N}/self/handle` — change the authenticated user's
    /// handle. matrix-rust-sdk equivalent: `Account::set_handle`.
    ///
    /// Wire enforces global uniqueness; a collision surfaces as
    /// `BackendRejected("update_self_handle 409: handle-exists")`.
    /// Caller is responsible for ASCII-lowering / length validation
    /// — Wire does its own server-side check.
    ///
    /// # Errors
    /// * `InvalidInput` if `handle` is empty.
    /// * `BackendRejected` on non-2xx from Wire (collision, malformed
    ///   handle, …).
    pub async fn set_handle(&self, handle: &str) -> Result<(), KupferdrahtError> {
        if handle.is_empty() {
            return Err(KupferdrahtError::InvalidInput(
                "set_handle: handle must be non-empty".into(),
            ));
        }
        self.http
            .update_self_handle(self.api_version, &self.current_access_token(), handle)
            .await?;
        self.remember_self_profile(Some(handle), None).await
    }

    /// Step 126: keep the persisted `self_user` row in step with a profile
    /// change the backend has accepted, so the next `resume` rebuilds the
    /// right [`Account`](super::Account) — it is immutable and
    /// `build_resume_account` reads this row.
    ///
    /// The row is written whole (`save_self_user` upserts on `id`): the
    /// fields this call did not change come from the row itself — an earlier
    /// write of this session is there, not in the account snapshot, which
    /// keeps the login's values — and from the snapshot only when there is
    /// no row yet. The live snapshot itself is deliberately untouched: it is
    /// an `Arc` the whole `Client` reads, and a consumer can cover its own
    /// writes for the session's lifetime.
    ///
    /// A session without a store (metadata-only) has nothing to remember.
    async fn remember_self_profile(
        &self,
        handle: Option<&str>,
        name: Option<&str>,
    ) -> Result<(), KupferdrahtError> {
        let Some(store) = self.store_handle() else {
            return Ok(());
        };
        let account = self.account();
        let id = account.user_id().as_str().to_string();
        debug_assert!(!id.is_empty(), "a logged-in client has a user id");
        let mut row = store
            .load_self_user(&id)
            .await?
            .unwrap_or_else(|| SelfUserRecord {
                id: id.clone(),
                handle: account.handle().map(str::to_owned),
                name: account.display_name().to_owned(),
                email: account.email().map(str::to_owned),
            });
        if let Some(handle) = handle {
            row.handle = Some(handle.to_owned());
        }
        if let Some(name) = name {
            row.name = name.to_owned();
        }
        store.save_self_user(&row).await
    }

    /// `PUT /v{N}/access/self/email` — request a change of the
    /// authenticated user's email. Wire sends a verification mail to the
    /// new address; the change takes effect only once the user confirms
    /// it. This therefore returns `Ok(())` on the `202`/`204`
    /// acknowledgement **without** touching the local `Account`
    /// snapshot — call `sync_metadata` after the user has verified to
    /// pick up the new address. matrix-rust-sdk's closest analog is
    /// `Account` 3PID management.
    ///
    /// # Errors
    /// * `InvalidInput` if `email` is empty.
    /// * `BackendRejected` on non-2xx from Wire — notably
    ///   `400 invalid-email` and `409 key-exists` (the address is already
    ///   in use); the response body carries the label for the caller to
    ///   branch on.
    pub async fn update_email(&self, email: &str) -> Result<(), KupferdrahtError> {
        if email.is_empty() {
            return Err(KupferdrahtError::InvalidInput(
                "update_email: email must be non-empty".into(),
            ));
        }
        self.http
            .update_self_email(self.api_version, &self.current_access_token(), email)
            .await
    }

    /// `PUT /v{N}/self/password` — change the account password. The
    /// `old_password` re-authenticates the change. matrix-rust-sdk's
    /// closest analog is `Account::change_password`.
    ///
    /// On success Wire invalidates **every** device's refresh cookie:
    /// all sessions (including this one, once the current access token
    /// expires) must re-`login` afterwards. Registered devices and MLS
    /// group state survive. This call deliberately does **not** touch
    /// the in-memory session — the current access token keeps working
    /// until its ~15-min expiry; the next `ClientBuilder::resume` past
    /// that point fails its `/access` rebind and surfaces the normal
    /// re-login path.
    ///
    /// # Errors
    /// * `InvalidInput` if either password is empty.
    /// * `BackendRejected` on non-2xx from Wire — notably
    ///   `403 invalid-credentials` (wrong `old_password`) and `400`
    ///   (password-policy violation); the body carries the label.
    pub async fn change_password(
        &self,
        old_password: &str,
        new_password: &str,
    ) -> Result<(), KupferdrahtError> {
        if old_password.is_empty() || new_password.is_empty() {
            return Err(KupferdrahtError::InvalidInput(
                "change_password: old and new password must be non-empty".into(),
            ));
        }
        self.http
            .update_self_password(
                self.api_version,
                &self.current_access_token(),
                old_password,
                new_password,
            )
            .await
    }
}
