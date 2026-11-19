// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `Client` — self-profile mutators.
//!
//! The profile-management slice of the `Client` runtime (Step 29):
//! `set_display_name`, `set_username`, `request_email_change`, and
//! `change_password`. Each lives on `Client` (matching the
//! `Client::create_room` / `Client::invite_members` pattern); the
//! matrix-rust-sdk analogs live on `Account`.
//!
//! The two that change a field this SDK also stores — the username and the
//! display name — are transport **plus** the persisted `self_user` row
//! (Step 126) and, since Step 152, the live `Account` the getters read
//! (`own_account.rs`).

#![allow(clippy::doc_markdown)]

use crate::error::KupferdrahtError;
use crate::storage::{SelfUserRecord, Store};

use super::Client;

impl Client {
    /// `PUT /v{N}/self` with a `name` body — change the
    /// authenticated user's display name. matrix-rust-sdk equivalent:
    /// `Account::set_display_name`. Kupferdraht puts the mutator on
    /// `Client` to match the existing `Client::create_room` /
    /// `Client::invite_members` pattern (transport calls live on
    /// `Client`, `Account` and `Room` are thin handles).
    ///
    /// `Client::account()` follows at once (Step 152), and the row
    /// `resume` reads.
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
        let _gate = self.account_gate.lock().await;
        self.http
            .update_self_profile_name(self.api_version, &self.current_access_token(), name)
            .await?;
        self.apply_own_profile(|row| name.clone_into(&mut row.name))
            .await
    }

    /// `PUT /v{N}/self/handle` — change the authenticated user's
    /// username. matrix-rust-sdk has no equivalent: a Matrix user id is
    /// fixed.
    ///
    /// The backend keeps usernames unique; one already taken surfaces as
    /// `BackendRejected` with a `409`. Caller is responsible for
    /// ASCII-lowering / length validation — the backend does its own
    /// check.
    ///
    /// # Errors
    /// * `InvalidInput` if `username` is empty.
    /// * `BackendRejected` on non-2xx from Wire (taken, malformed, …).
    pub async fn set_username(&self, username: &str) -> Result<(), KupferdrahtError> {
        if username.is_empty() {
            return Err(KupferdrahtError::InvalidInput(
                "set_username: username must be non-empty".into(),
            ));
        }
        let _gate = self.account_gate.lock().await;
        self.http
            .update_self_handle(self.api_version, &self.current_access_token(), username)
            .await?;
        self.apply_own_profile(|row| row.username = Some(username.to_owned()))
            .await
    }

    /// The `self_user` row, or one from the account snapshot when there is
    /// none yet: a write of this session is in the row, not in the snapshot.
    pub(super) async fn current_self_row(
        &self,
        store: &dyn Store,
    ) -> Result<SelfUserRecord, KupferdrahtError> {
        let account = self.account();
        let id = account.user_id().as_str().to_string();
        debug_assert!(!id.is_empty(), "a logged-in client has a user id");
        Ok(store
            .load_self_user(&id)
            .await?
            .unwrap_or_else(|| super::own_account::row_of(&account)))
    }

    /// Ask the backend to change the account's email address to `email`
    /// (Step 29, renamed in 162): `PUT /v{N}/access/self/email`. The backend
    /// mails a confirmation link to the new address; the address changes
    /// once the user follows it, so this returns on the `202`/`204`
    /// acknowledgement without touching the local `Account` — call
    /// `refresh_account` after the confirmation to read the new address
    /// (Step 152). matrix-rust-sdk's closest analog is `Account` 3PID
    /// management.
    ///
    /// # Errors
    /// * `InvalidInput` if `email` is empty.
    /// * `BackendRejected` when the backend refuses the address — notably
    ///   `400 invalid-email` and `409 key-exists` (in use by another
    ///   account); the message carries its label.
    pub async fn request_email_change(&self, email: &str) -> Result<(), KupferdrahtError> {
        if email.is_empty() {
            return Err(KupferdrahtError::InvalidInput(
                "request_email_change: email must be non-empty".into(),
            ));
        }
        self.http
            .update_self_email(
                self.api_version,
                &self.current_access_token(),
                &self.current_refresh_token(),
                email,
            )
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
