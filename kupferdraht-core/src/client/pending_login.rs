// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `PendingLogin` — the post-authentication, pre-device-registration handle.
//!
//! Login is two-phase. [`ClientBuilder::login`](super::ClientBuilder::login)
//! runs **phase 1** (version negotiation, `POST /login` + `GET /self`, storage
//! setup) and returns a `PendingLogin` that already holds a valid session.
//! [`PendingLogin::register_device`] runs **phase 2** (`POST /clients`, MLS
//! identity, finalize) and yields a ready [`Client`].
//!
//! Splitting the two lets a consumer recover from Wire's per-user device cap
//! without a second login: when `register_device` fails with
//! [`KupferdrahtError::TooManyDevices`], the `PendingLogin` stays valid — the caller
//! lists [`devices`](Self::devices), removes one with
//! [`delete_device`](Self::delete_device), and calls `register_device` again,
//! all on the **same** captured session. A future SSO login produces the same
//! handle (the session is captured regardless of how it was obtained), so this
//! recovery surface is auth-method-agnostic.
//!
//! The held password is `Option` — Wire requires it only for the 2nd, 3rd, …
//! client of a password account, and password-less accounts verify
//! differently. It is wrapped in `Zeroizing` per `CLAUDE.md` §Sensitive
//! Material, alongside the user-scoped access token.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use zeroize::Zeroizing;

use crate::client::login_pipeline::LoginPipeline;
use crate::client::resume::{DeviceIdentity, establish_device_identity};
use crate::client::{Client, DeviceInfo};
use crate::error::KupferdrahtError;
#[cfg(any(target_arch = "wasm32", test))]
use crate::keystore::SecureKeyStore;
use crate::storage::KupferdrahtHome;
#[cfg(any(target_arch = "wasm32", test))]
use crate::storage::StorageProvider;
use crate::storage::db_key::DbKey;
use crate::wire::HttpClient;
use crate::wire::dto::{LoginOutcome, SelfUser};

/// The phase-2 inputs `register_device` consumes. Held behind a
/// `Mutex<Option<_>>` so a `&self` (shared `Arc`) `register_device` can move
/// them into `finalize` on success while restoring them on failure.
struct ConsumableState {
    pipeline: LoginPipeline,
    login_outcome: LoginOutcome,
    self_user: SelfUser,
    mls_db_key: Option<DbKey>,
    home: Option<KupferdrahtHome>,
}

/// Post-authentication handle returned by
/// [`ClientBuilder::login`](super::ClientBuilder::login); consumed by
/// [`PendingLogin::register_device`].
pub struct PendingLogin {
    user_id: String,
    email: String,
    api_version: u32,
    /// Dedicated transport for the user-scoped `devices` / `delete_device`
    /// calls. A separate instance (not the pipeline's) because `HttpClient`
    /// is not `Clone` and must not be borrowed out from under the
    /// consume-on-success `consumable` state.
    http: HttpClient,
    /// User-scoped login token — valid for the device list/delete calls
    /// regardless of how authentication happened (auth-agnostic).
    access_token: Zeroizing<String>,
    /// `Some` for a password login; `None` for a password-less (SSO) account.
    password: Option<Zeroizing<String>>,
    consumable: Mutex<Option<ConsumableState>>,
    /// Whether the account's storage is open: at login in a `Random` home;
    /// in a `Seed` home — the browser — only once `registerDevice` opened
    /// it (Step 110a, Step 111c, ADR-0035). Whether the seed is here is the
    /// manager's to know.
    storage_open: AtomicBool,
}

impl PendingLogin {
    /// The authenticated user's qualified Wire id (`<uuid>@<domain>`).
    #[must_use]
    pub fn user_id(&self) -> &str {
        &self.user_id
    }

    /// The email the session was authenticated under. Echoes input verbatim.
    #[must_use]
    pub fn email(&self) -> &str {
        &self.email
    }

    /// List the devices currently registered for this user
    /// (`GET /v{N}/clients`). Used to free a slot when device registration
    /// hits the per-user cap. No device is marked "current" — at this point
    /// none has been registered for this session yet.
    ///
    /// # Errors
    /// * `BackendRejected` / `Unauthorized` / `Network` from the call.
    pub async fn devices(&self) -> Result<Vec<DeviceInfo>, KupferdrahtError> {
        debug_assert!(
            !self.access_token.is_empty(),
            "PendingLogin must hold a session token"
        );
        let wire = self
            .http
            .list_clients(self.api_version, self.access_token.as_str())
            .await?;
        Ok(wire
            .into_iter()
            .map(|c| DeviceInfo::from_wire(c, ""))
            .collect())
    }

    /// Revoke a registered device (`DELETE /v{N}/clients/{id}`) to free a
    /// slot, then retry [`register_device`](Self::register_device).
    ///
    /// # Errors
    /// * `BackendRejected` (e.g. wrong password) / `Network` from the call.
    pub async fn delete_device(&self, device_id: &str) -> Result<(), KupferdrahtError> {
        crate::client::input::require_non_empty("delete_device", &[("device_id", device_id)])?;
        self.http
            .delete_client(
                self.api_version,
                self.access_token.as_str(),
                device_id,
                self.password.as_ref().map(|p| p.as_str()),
            )
            .await
    }

    /// Phase 2: register this device on Wire and assemble a ready [`Client`].
    ///
    /// On success the captured session is consumed (a second call returns
    /// `InvalidInput`). On a recoverable failure — notably
    /// [`KupferdrahtError::TooManyDevices`] from `POST /clients` — the session is
    /// **restored**, so the caller can free a slot via
    /// [`devices`](Self::devices) / [`delete_device`](Self::delete_device) and
    /// call this again.
    ///
    /// # Errors
    /// * `TooManyDevices` when the per-user device cap is reached (retryable
    ///   after freeing a slot).
    /// * Whatever the device-registration / MLS / removal-key steps surface.
    /// * `InvalidInput` if the login was already completed.
    pub async fn register_device(&self) -> Result<Arc<Client>, KupferdrahtError> {
        debug_assert!(
            !self.access_token.is_empty(),
            "PendingLogin must hold a session token"
        );
        if !self.storage_is_open() {
            return Err(KupferdrahtError::InvalidInput(
                "register_device: the account's storage is not open — in a seed home the \
                 manager's register_device opens it"
                    .into(),
            ));
        }
        let state = self.take_state()?;
        let password = self.password.as_ref().map(|p| p.as_str());
        // `establish_device_identity` only borrows `state`; on failure the
        // session is intact, so put it back for a retry (e.g. after deleting
        // a device to clear `TooManyDevices`). On success `state` is consumed.
        match establish_device_identity(
            &state.pipeline,
            &state.login_outcome,
            &state.self_user,
            password,
            state.mls_db_key.as_ref(),
        )
        .await
        {
            Err(e) => {
                self.restore_state(state);
                Err(e)
            }
            Ok(identity) => finalize_into_client(state, identity).await,
        }
    }

    /// Whether the account's storage is open (ADR-0035): at login in a
    /// `Random` home; in a `Seed` home once `registerDevice` opened it.
    pub(crate) fn storage_is_open(&self) -> bool {
        self.storage_open.load(Ordering::Acquire)
    }

    /// Take the consumable phase-2 state, leaving the handle spent.
    /// `None` ⇒ a successful `register_device` already ran (single-use).
    fn take_state(&self) -> Result<ConsumableState, KupferdrahtError> {
        self.consumable
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
            .ok_or_else(|| KupferdrahtError::InvalidInput("login already completed".into()))
    }

    /// Put the phase-2 state back after a recoverable `register_device`
    /// failure, so the caller can retry on the same captured session.
    fn restore_state(&self, state: ConsumableState) {
        *self
            .consumable
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(state);
    }
}

/// What a first setup in the browser needs of the pending login
/// (ADR-0035; driven by `manager::PendingLogin`).
#[cfg(any(target_arch = "wasm32", test))]
impl PendingLogin {
    /// Open the account's storage — at `registerDevice`, with its seed here
    /// (a first setup creates it; Step 111c).
    ///
    /// # Errors
    /// Key creation and storage errors; `InvalidInput` once the login
    /// completed.
    pub(crate) async fn open_account_storage(
        &self,
        storage_provider: Option<Arc<dyn StorageProvider>>,
        keystore: &Arc<dyn SecureKeyStore>,
    ) -> Result<(), KupferdrahtError> {
        let mut state = self.take_state()?;
        let opened = super::builder::open_account_storage(
            storage_provider,
            state.home.as_ref(),
            keystore,
            &self.user_id,
        )
        .await;
        let result = opened.map(|(store, mls_db_key)| {
            let home = state.home.clone();
            state.pipeline.bind_storage(store, home);
            state.mls_db_key = mls_db_key;
            self.storage_open.store(true, Ordering::Release);
        });
        self.restore_state(state);
        result
    }

    /// The login's Wire token — what a storage token for the account's
    /// databases on the storage server (Step 111b) and a PIN token for
    /// recovery data (Step 112) are bought with.
    #[cfg(any(target_arch = "wasm32", test))]
    pub(crate) fn login_access_token(&self) -> &str {
        debug_assert!(!self.access_token.is_empty(), "a login holds its token");
        self.access_token.as_str()
    }

    /// Drop the login's state, closing whatever it opened.
    ///
    /// # Errors
    /// `InvalidInput` once the login completed.
    pub(crate) fn discard(&self) -> Result<(), KupferdrahtError> {
        drop(self.take_state()?);
        Ok(())
    }
}

/// Consume the phase-2 state into a ready [`Client`]: fetch the backend
/// removal key with the freshly-rebound token, then finalize. Split out so
/// `register_device` stays within the per-function line budget.
async fn finalize_into_client(
    state: ConsumableState,
    identity: DeviceIdentity,
) -> Result<Arc<Client>, KupferdrahtError> {
    let DeviceIdentity {
        registration,
        mls_identity,
        replaced,
    } = identity;
    let backend_removal_key = state
        .pipeline
        .fetch_backend_removal_key(&registration.access_token)
        .await?;
    let ConsumableState {
        pipeline,
        login_outcome,
        self_user,
        home,
        ..
    } = state;
    let client = pipeline.finalize(
        self_user,
        login_outcome,
        registration,
        backend_removal_key,
        mls_identity,
        home,
    );
    if replaced {
        client.mark_device_replaced();
    }
    Ok(client)
}

impl std::fmt::Debug for PendingLogin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Omit `password` and `access_token` — `Debug` is a logging surface
        // and must never leak credentials.
        f.debug_struct("PendingLogin")
            .field("user_id", &self.user_id)
            .field("email", &self.email)
            .finish_non_exhaustive()
    }
}

/// Crate-internal constructor used by `ClientBuilder::login`. Keeps
/// `PendingLogin`'s fields private so callers can't forge a half-built
/// session. Derives `user_id` / `access_token` from the captured phase-1
/// outputs; `http` is the dedicated device-ops transport.
#[allow(clippy::too_many_arguments)]
pub(super) fn new_pending_login(
    self_user: SelfUser,
    email: String,
    login_outcome: LoginOutcome,
    pipeline: LoginPipeline,
    http: HttpClient,
    password: Option<Zeroizing<String>>,
    mls_db_key: Option<DbKey>,
    home: Option<KupferdrahtHome>,
    storage_open: bool,
) -> Arc<PendingLogin> {
    let user_id = self_user.qualified_id.to_string();
    let api_version = pipeline.api_version();
    let access_token = Zeroizing::new(login_outcome.access_token.clone());
    Arc::new(PendingLogin {
        user_id,
        email,
        api_version,
        http,
        access_token,
        password,
        storage_open: AtomicBool::new(storage_open),
        consumable: Mutex::new(Some(ConsumableState {
            pipeline,
            login_outcome,
            self_user,
            mls_db_key,
            home,
        })),
    })
}
