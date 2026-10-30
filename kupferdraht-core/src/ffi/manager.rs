// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! FFI wrappers for the multi-account manager + registration (Step 20c-6).
//!
//! `ffi::Kupferdraht` wraps the native multi-account manager (registry-aware
//! login / resume / register); `ffi::PendingClient` wraps the
//! manager-aware register→activate handle
//! ([`PendingAccount`](crate::manager::PendingAccount)). `AccountEntry`
//! is FFI-shaped and derives on the core type.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use super::Client;
use crate::client::{DeviceInfo, ServerConfig};
use crate::error::KupferdrahtError;
use crate::keystore::SecureKeyStore;
use crate::manager::{
    Kupferdraht as CoreKupferdraht, PendingAccount, PendingLogin as CorePendingLogin,
};
use crate::storage::AccountEntry;

/// The device-wide, multi-account entry point — the FFI wrapper around
/// the native [`kupferdraht::Kupferdraht`](crate::manager::Kupferdraht).
#[derive(uniffi::Object)]
pub struct Kupferdraht {
    inner: CoreKupferdraht,
}

#[uniffi::export]
impl Kupferdraht {
    /// Open the manager rooted at `base_dir`, keyed by `keystore`.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    #[uniffi::constructor]
    pub fn open(
        base_dir: String,
        keystore: Arc<dyn SecureKeyStore>,
    ) -> Result<Arc<Self>, KupferdrahtError> {
        let inner = CoreKupferdraht::open(PathBuf::from(base_dir), keystore)?;
        Ok(Arc::new(Self { inner }))
    }

    /// The id of this home (Step 93, ADR-0025): 32 lowercase hex, persisted
    /// in `base_dir/home.id`; every keystore id of the home
    /// (`kupferdraht.<home-id>.…`) and its login cookie label carry it.
    #[must_use]
    pub fn home_id(&self) -> String {
        self.inner.home_id().to_owned()
    }

    /// The Wire login cookie label of this home, `kupferdraht-<home-id>`
    /// (Step 93) — one per home, so two homes of one user never evict each
    /// other's refresh cookie.
    #[must_use]
    pub fn cookie_label(&self) -> String {
        self.inner.cookie_label()
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl Kupferdraht {
    /// Every account registered on this device.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn accounts(&self) -> Result<Vec<AccountEntry>, KupferdrahtError> {
        self.inner.accounts().await
    }

    /// The most-recently-active account's qualified id, if any.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn active_account(&self) -> Result<Option<String>, KupferdrahtError> {
        self.inner.active_account().await
    }

    /// Phase 1 of login: authenticate against `server` and return a
    /// [`PendingLogin`]. Call [`PendingLogin::register_device`] to register
    /// the device, write the registry row, and obtain a ready `Client`. The
    /// split lets the caller recover from `TooManyDevices` without
    /// re-authenticating.
    ///
    /// `device_label` is the human-readable name shown in the user's
    /// device list (e.g. the OS device name). When `None`, a platform
    /// default is used.
    ///
    /// # Errors
    /// See `KupferdrahtError` (authentication failures surface here).
    pub async fn login(
        &self,
        server: ServerConfig,
        email: String,
        password: String,
        second_factor_code: Option<String>,
        device_label: Option<String>,
    ) -> Result<Arc<PendingLogin>, KupferdrahtError> {
        self.inner
            .login(server, email, password, second_factor_code, device_label)
            .await
            .map(PendingLogin::wrap)
    }

    /// SSO (Step 17β) — validate `code` against `server` and return the
    /// interactive initiate URL to open in a browser. The `zuid` cookie
    /// from the post-`IdP` redirect feeds `finalize_sso` (17b).
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn initiate_sso(
        &self,
        server: ServerConfig,
        code: &str,
        success_redirect: &str,
        error_redirect: &str,
    ) -> Result<String, KupferdrahtError> {
        self.inner
            .initiate_sso(server, code, success_redirect, error_redirect)
            .await
    }

    /// SSO email discovery (Step 17β) — resolve the backend's default SSO code
    /// (`GET /sso/settings`) and return the interactive initiate URL. Wire v14
    /// has no per-domain discovery, so the team default is used; multi-IdP
    /// backends must use `initiate_sso` with an explicit code.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn initiate_sso_by_email(
        &self,
        server: ServerConfig,
        email: &str,
        success_redirect: &str,
        error_redirect: &str,
    ) -> Result<String, KupferdrahtError> {
        self.inner
            .initiate_sso_by_email(server, email, success_redirect, error_redirect)
            .await
    }

    /// SSO finalize (Step 17β) — complete login from the `zuid` cookie handed
    /// back by Wire's `/sso/finalize-login` redirect, returning a
    /// [`PendingLogin`] to drive through `register_device` like the password
    /// flow.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn finalize_sso(
        &self,
        server: ServerConfig,
        sso_cookie: &str,
        device_label: Option<String>,
    ) -> Result<Arc<PendingLogin>, KupferdrahtError> {
        self.inner
            .finalize_sso(server, sso_cookie, device_label)
            .await
            .map(PendingLogin::wrap)
    }

    /// Resume a registered account by its qualified id.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn resume(&self, account_id: &str) -> Result<Arc<Client>, KupferdrahtError> {
        self.inner.resume(account_id).await.map(Client::wrap)
    }

    /// Create a fresh Wire account, returning a [`PendingClient`] the
    /// caller drives through [`PendingClient::activate`] with the
    /// emailed code. Activation registers the account in the device
    /// registry and persists its session — it becomes listable via
    /// `accounts` and resumable via `resume`.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn register(
        &self,
        server: ServerConfig,
        name: String,
        email: String,
        password: String,
    ) -> Result<Arc<PendingClient>, KupferdrahtError> {
        self.inner
            .register(server, name, email, password)
            .await
            .map(PendingClient::wrap)
    }
}

/// Post-authentication handle — the FFI wrapper around the native
/// [`kupferdraht::PendingLogin`](crate::manager::PendingLogin). `register_device`
/// completes login; on `TooManyDevices` the caller frees a slot via
/// `devices` / `delete_device` and calls `register_device` again, all on the
/// same captured session.
#[derive(uniffi::Object)]
pub struct PendingLogin {
    inner: Arc<CorePendingLogin>,
}

impl PendingLogin {
    fn wrap(inner: Arc<CorePendingLogin>) -> Arc<Self> {
        Arc::new(Self { inner })
    }
}

#[uniffi::export]
impl PendingLogin {
    /// The authenticated user's qualified Wire id.
    #[must_use]
    pub fn user_id(&self) -> String {
        self.inner.user_id().to_owned()
    }

    /// The email the session was authenticated under.
    #[must_use]
    pub fn email(&self) -> String {
        self.inner.email().to_owned()
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl PendingLogin {
    /// Register this device and obtain a ready, resumable `Client`. On
    /// `TooManyDevices` (Wire's per-user cap) the handle stays valid — free a
    /// slot via `devices` / `delete_device` and call this again.
    ///
    /// # Errors
    /// `TooManyDevices` (retryable), or whatever registration surfaces;
    /// `InvalidInput` if the login was already completed.
    pub async fn register_device(&self) -> Result<Arc<Client>, KupferdrahtError> {
        self.inner.register_device().await.map(Client::wrap)
    }

    /// List the account's registered devices, so the caller can pick one to
    /// remove and free a slot.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn devices(&self) -> Result<Vec<DeviceInfo>, KupferdrahtError> {
        self.inner.devices().await
    }

    /// Remove a device by its Wire `client_id` to free a slot.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn delete_device(&self, device_id: &str) -> Result<(), KupferdrahtError> {
        self.inner.delete_device(device_id).await
    }
}

/// A registered-but-not-yet-activated account — the FFI wrapper around
/// the native [`kupferdraht::PendingAccount`](crate::manager::PendingAccount).
#[derive(uniffi::Object)]
pub struct PendingClient {
    user_id: String,
    email: String,
    // Consumed on a successful `activate` (the native handle is once-only);
    // `None` afterwards. The id / email above stay readable regardless.
    inner: Mutex<Option<PendingAccount>>,
}

impl PendingClient {
    fn wrap(inner: PendingAccount) -> Arc<Self> {
        Arc::new(Self {
            user_id: inner.user_id().to_owned(),
            email: inner.email().to_owned(),
            inner: Mutex::new(Some(inner)),
        })
    }

    /// Clone the live handle without consuming it (for `resend`).
    fn peek(&self) -> Result<PendingAccount, KupferdrahtError> {
        self.inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .ok_or_else(|| {
                KupferdrahtError::InvalidInput("pending client already activated".into())
            })
    }
}

#[uniffi::export]
impl PendingClient {
    /// The new account's Wire UUID (from `/register`).
    #[must_use]
    pub fn user_id(&self) -> String {
        self.user_id.clone()
    }

    /// The email the account was registered with.
    #[must_use]
    pub fn email(&self) -> String {
        self.email.clone()
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl PendingClient {
    /// Redeem the email-delivered activation code, completing login and
    /// registering the account in the device registry (listable via
    /// `Kupferdraht.accounts`, resumable via `Kupferdraht.resume`).
    ///
    /// Once-only on success: a second call returns `InvalidInput`. A
    /// *failed* activation leaves the handle valid for a retry.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn activate(&self, code: &str) -> Result<Arc<Client>, KupferdrahtError> {
        let pending = self
            .inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
            .ok_or_else(|| {
                KupferdrahtError::InvalidInput("pending client already activated".into())
            })?;
        match pending.activate(code).await {
            Ok(client) => Ok(Client::wrap(client)),
            Err(error) => {
                // Activation failed — restore the handle so the caller can retry.
                *self.inner.lock().unwrap_or_else(PoisonError::into_inner) = Some(pending);
                Err(error)
            }
        }
    }

    /// Ask Wire to re-deliver the activation email.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn resend_activation_email(&self) -> Result<(), KupferdrahtError> {
        self.peek()?.resend_activation_email().await
    }
}
