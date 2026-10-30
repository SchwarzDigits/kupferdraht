// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `PendingLogin` — the manager-aware post-authentication handle.
//!
//! Wraps the builder-level [`crate::client::PendingLogin`] and finishes what
//! it cannot: a successful `register_device` additionally writes the
//! device-registry row (marking the account active) and persists the session
//! — the same writes a full [`Kupferdraht::login`](super::Kupferdraht::login) makes, so
//! the account is immediately listable via `Kupferdraht::accounts` and resumable
//! via `Kupferdraht::resume`. Returned by [`Kupferdraht::login`](super::Kupferdraht::login).

use std::sync::Arc;

use crate::client::{Client, DeviceInfo, PendingLogin as ClientPendingLogin, ServerConfig};
use crate::error::KupferdrahtError;
use crate::storage::GlobalStore;

use super::{persist_session, register_account};

#[cfg(any(target_arch = "wasm32", test))]
mod first_setup;

#[cfg(any(target_arch = "wasm32", test))]
pub(crate) use first_setup::{KeysState, RegisterOptions};

/// Post-authentication handle (manager flavor). Returned by
/// [`Kupferdraht::login`](super::Kupferdraht::login); drive
/// [`PendingLogin::register_device`] to obtain a registered, resumable
/// [`Client`].
pub struct PendingLogin {
    inner: Arc<ClientPendingLogin>,
    server: ServerConfig,
    global: Arc<dyn GlobalStore>,
    /// Step 110a: the browser's first setup (ADR-0035), attached by
    /// `Kupferdraht::login` in a home whose account keys come from a seed.
    #[cfg(any(target_arch = "wasm32", test))]
    first_setup: Option<first_setup::FirstSetup>,
}

impl PendingLogin {
    pub(super) fn new(
        inner: Arc<ClientPendingLogin>,
        server: ServerConfig,
        global: Arc<dyn GlobalStore>,
    ) -> Self {
        Self {
            inner,
            server,
            global,
            #[cfg(any(target_arch = "wasm32", test))]
            first_setup: None,
        }
    }

    /// The authenticated user's qualified Wire id.
    #[must_use]
    pub fn user_id(&self) -> &str {
        self.inner.user_id()
    }

    /// The email the session was authenticated under.
    #[must_use]
    pub fn email(&self) -> &str {
        self.inner.email()
    }

    /// List the user's registered devices to free a slot when registration
    /// hit the per-user cap. See [`crate::client::PendingLogin::devices`].
    ///
    /// # Errors
    /// See [`crate::client::PendingLogin::devices`].
    pub async fn devices(&self) -> Result<Vec<DeviceInfo>, KupferdrahtError> {
        self.inner.devices().await
    }

    /// Revoke a device to free a slot, then retry
    /// [`register_device`](Self::register_device). See
    /// [`crate::client::PendingLogin::delete_device`].
    ///
    /// # Errors
    /// See [`crate::client::PendingLogin::delete_device`].
    pub async fn delete_device(&self, device_id: &str) -> Result<(), KupferdrahtError> {
        self.inner.delete_device(device_id).await
    }

    /// Phase 2: register this device, then write the registry row (marking the
    /// account active) and persist the session — the account becomes listable
    /// and resumable. A failed registration (e.g. `TooManyDevices`) leaves the
    /// registry untouched and the handle valid for a retry.
    ///
    /// **Failure window:** if the registry / session writes fail *after* the
    /// device was registered on Wire, that device exists server-side but is not
    /// recorded locally, and the (now spent) handle can't retry — a fresh login
    /// would register another device. These are local-SQLite writes (rarely
    /// fail); this mirrors the posture of `PendingAccount::activate`.
    ///
    /// # Errors
    /// Whatever [`crate::client::PendingLogin::register_device`] surfaces, or a
    /// registry / session-persistence write failure.
    pub async fn register_device(&self) -> Result<Arc<Client>, KupferdrahtError> {
        // Step 111c: a seed home opens the account's storage here, not at
        // login (ADR-0035).
        #[cfg(any(target_arch = "wasm32", test))]
        self.open_for_registration(RegisterOptions::default())
            .await?;
        self.complete_registration().await
    }

    /// Register the device on the open storage, then write the registry
    /// row and persist the session.
    async fn complete_registration(&self) -> Result<Arc<Client>, KupferdrahtError> {
        let client = self.inner.register_device().await?;
        debug_assert!(
            client.user_id().is_some(),
            "a registered client must carry an identity to key the registry row"
        );
        // Step 111b: the client's session renews the storage token now.
        #[cfg(target_arch = "wasm32")]
        super::remote_resume::attach_client_session(&client);
        // Step 120 (ADR-0040): the slot names this device from now on.
        #[cfg(target_arch = "wasm32")]
        self.label_device_slot(&client);
        register_account(&self.global, &client, self.server.api_base_url.clone()).await?;
        persist_session(&client, &self.server).await?;
        Ok(client)
    }
}

impl std::fmt::Debug for PendingLogin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingLogin")
            .field("user_id", &self.user_id())
            .field("email", &self.email())
            .finish_non_exhaustive()
    }
}
