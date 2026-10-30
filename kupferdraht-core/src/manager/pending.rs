// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `PendingAccount` — the manager-aware post-register handle.
//!
//! Wraps the builder-level [`PendingClient`] and finishes what it
//! cannot: a successful activation registers the account in the device
//! registry, marks it active, and persists its session — exactly the
//! writes [`Kupferdraht::login`](super::Kupferdraht::login) makes, so the account
//! is immediately listable and resumable. Returned by
//! [`Kupferdraht::register`](super::Kupferdraht::register).

use std::sync::Arc;

use crate::client::{Client, PendingClient, ServerConfig};
use crate::error::KupferdrahtError;
use crate::storage::GlobalStore;

use super::{persist_session, register_account};

/// Post-register, pre-activation handle (manager flavor). Clones are
/// cheap and share the underlying register state, so a failed
/// activation can be retried from any clone.
#[derive(Clone)]
pub struct PendingAccount {
    inner: Arc<PendingClient>,
    server: ServerConfig,
    global: Arc<dyn GlobalStore>,
}

impl PendingAccount {
    pub(super) fn new(
        inner: Arc<PendingClient>,
        server: ServerConfig,
        global: Arc<dyn GlobalStore>,
    ) -> Self {
        Self {
            inner,
            server,
            global,
        }
    }

    /// The new account's Wire user id (UUID from `POST /register`).
    #[must_use]
    pub fn user_id(&self) -> &str {
        self.inner.user_id()
    }

    /// The email the account was registered under.
    #[must_use]
    pub fn email(&self) -> &str {
        self.inner.email()
    }

    /// Redeem the email-delivered activation code, then register the
    /// account in the device registry (marking it active) and persist
    /// its session. A failed activation leaves the registry untouched
    /// and the handle valid for a retry.
    ///
    /// # Errors
    /// Whatever [`PendingClient::activate`] surfaces, or a registry /
    /// session-persistence write failure.
    pub async fn activate(&self, code: &str) -> Result<Arc<Client>, KupferdrahtError> {
        let client = self.inner.clone().activate(code).await?;
        register_account(&self.global, &client, self.server.api_base_url.clone()).await?;
        persist_session(&client, &self.server).await?;
        Ok(client)
    }

    /// Ask Wire to re-deliver the activation email.
    ///
    /// # Errors
    /// See [`PendingClient::resend_activation_email`].
    pub async fn resend_activation_email(&self) -> Result<(), KupferdrahtError> {
        self.inner.resend_activation_email().await
    }
}

impl std::fmt::Debug for PendingAccount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingAccount")
            .field("user_id", &self.user_id())
            .field("email", &self.email())
            .finish_non_exhaustive()
    }
}
