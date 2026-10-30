// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! The browser's side of a pending login (Step 110a, ADR-0035): whether the
//! account's seed is here, the hint whether recovery data exists, a first
//! setup, and cancelling.
//!
//! Only a home whose account keys come from a seed needs it — natively the
//! login opens the account at once and none of this runs.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::PendingLogin;
use crate::client::Client;
use crate::error::KupferdrahtError;
use crate::manager::ManagerConfig;
use crate::storage::account_keys::{SEED_LEN, forget_account_secrets, generate_seed, has_seed};
use crate::storage::{
    delete_account_databases, delete_leftover_account_databases, store_seed_durably,
};

/// Whether this browser holds the account's seed (ADR-0035). A `Random`
/// home — native — always reports `Present`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KeysState {
    /// The seed is here: `register_device` takes the native path.
    Present,
    /// No seed: restore recovery data, or set up a new device.
    Missing,
}

/// Options of [`PendingLogin::register_device_with`].
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct RegisterOptions {
    /// A first setup: create the account's seed when this browser holds
    /// none (after removing keys and databases left without one).
    pub(crate) new_setup: bool,
    /// Take the account's databases over from another instance that holds
    /// them (the storage server's lease, Step 111c); without it such a
    /// database throws `HeldElsewhere`.
    #[cfg(target_arch = "wasm32")]
    pub(crate) take_over: bool,
}

/// What a first setup needs beyond the client-level login.
pub(crate) struct FirstSetup {
    config: ManagerConfig,
    /// Whether this browser holds the account's seed: asked at login, set
    /// by a first setup.
    seed_present: AtomicBool,
    /// Whether the account has a registered browser device — asked once,
    /// at login, while the account's seed is missing.
    recovery_expected: bool,
    /// Whether a first setup of this login created the seed — what
    /// `cancel` removes again, with the account's databases.
    created_seed: AtomicBool,
    /// Whether this login recovered the seed (Step 112) — what `cancel`
    /// forgets again; the databases are the account's and stay.
    recovered_seed: AtomicBool,
    /// Step 120 (ADR-0040): the account's storage slot as the login found it
    /// — whom a first setup replaces.
    #[cfg(target_arch = "wasm32")]
    slot: Option<crate::storage::remote::slot::Slot>,
}

impl PendingLogin {
    /// Attach the first setup of a home whose account keys come from a
    /// seed. Without the account's seed it asks the account's device list
    /// whether a browser device is registered (`recovery_expected`).
    ///
    /// # Errors
    /// Keystore errors; `BackendRejected` / `Network` from the device list.
    pub(crate) async fn with_first_setup(
        mut self,
        config: ManagerConfig,
    ) -> Result<Self, KupferdrahtError> {
        let seed_present = has_seed(&config.keystore, config.home.home_id(), self.user_id())?;
        // Step 120 (ADR-0040): the account's storage slot names its device;
        // only a set-up one (labelled) has recovery data. Without a PIN
        // service (the local mode) there is none.
        let slot = match (&config.recovery_services, seed_present) {
            (Some(services), false) => {
                let session = crate::storage::remote::wire_session::WireSession::Login(
                    zeroize::Zeroizing::new(self.inner.login_access_token().to_owned()),
                );
                crate::recovery::flow::account_slot(services, &session).await?
            }
            _ => None,
        };
        let recovery_expected = crate::storage::remote::slot::recovery_expected(slot.as_ref());
        self.first_setup = Some(FirstSetup {
            config,
            seed_present: AtomicBool::new(seed_present),
            recovery_expected,
            created_seed: AtomicBool::new(false),
            recovered_seed: AtomicBool::new(false),
            #[cfg(target_arch = "wasm32")]
            slot,
        });
        Ok(self)
    }

    /// Whether this browser holds the account's seed (ADR-0035).
    pub(crate) fn keys(&self) -> KeysState {
        match &self.first_setup {
            Some(setup) if !setup.seed_present.load(Ordering::Acquire) => KeysState::Missing,
            _ => KeysState::Present,
        }
    }

    /// Whether the account has a registered browser device (model
    /// `Kupferdraht (Web)`): recovery data exists somewhere and would bring it
    /// back, where a first setup adds another device. Known while `keys`
    /// is `Missing`; `false` otherwise.
    pub(crate) fn recovery_expected(&self) -> bool {
        self.first_setup
            .as_ref()
            .is_some_and(|setup| setup.recovery_expected)
    }

    /// [`register_device`](Self::register_device) with `options`: without
    /// the account's seed only a first setup (`new_setup`) proceeds.
    ///
    /// # Errors
    /// `RecoveryDataRequired` without the seed and without `new_setup`;
    /// otherwise as `register_device` and the first setup.
    pub(crate) async fn register_device_with(
        &self,
        options: RegisterOptions,
    ) -> Result<Arc<Client>, KupferdrahtError> {
        let opened = self.open_for_registration(options).await;
        // Step 121 (ADR-0040): the account's storage slot refused the open, so
        // this browser's keys are a replaced device's. Forget them here as
        // `resume` does — the error still stands, but the next login finds no
        // seed and offers recovery instead of meeting this again. A failed
        // forgetting does not replace the refusal: the caller must hear
        // `DeviceRetired`, and the next login tries the forgetting again.
        if let Err(KupferdrahtError::DeviceRetired(_)) = &opened
            && let Err(e) = self.forget_retired_device().await
        {
            tracing::warn!(error = %e, "a retired device's keys could not be forgotten");
        }
        opened?;
        self.complete_registration().await
    }

    /// Before a registration in a seed home (ADR-0035): without the seed
    /// only a first setup proceeds; with it the account's storage opens now
    /// — the login opened nothing (Step 111c). Nothing once it is open (a
    /// retry after `TooManyDevices`) or in a `Random` home.
    ///
    /// # Errors
    /// `RecoveryDataRequired` without the seed and without `new_setup`;
    /// the first setup's and the storage's errors (`HeldElsewhere`, …).
    pub(super) async fn open_for_registration(
        &self,
        options: RegisterOptions,
    ) -> Result<(), KupferdrahtError> {
        if self.first_setup.is_none() || self.inner.storage_is_open() {
            return Ok(());
        }
        let setup = self.setup()?;
        if !setup.seed_present.load(Ordering::Acquire) {
            if !options.new_setup {
                return Err(KupferdrahtError::RecoveryDataRequired(
                    self.user_id().to_owned(),
                ));
            }
            self.create_account_seed().await?;
        }
        #[cfg(target_arch = "wasm32")]
        self.prepare_remote_databases(options.take_over).await?;
        #[cfg(target_arch = "wasm32")]
        if setup.created_seed.load(Ordering::Acquire) && crate::storage::storage_server_configured()
        {
            self.claim_device_slot().await?;
        }
        self.inner
            .open_account_storage(Some(setup.config.provider.clone()), &setup.config.keystore)
            .await?;
        debug_assert!(self.inner.storage_is_open(), "the storage is open");
        Ok(())
    }

    /// Restore the account from recovery data (Step 112, ADR-0035): with the
    /// seed missing, open `data` with `pin` for this account (the login's
    /// token buys the PIN token), store the seed durably, then register as
    /// `register_device_with` does — with `options.take_over`. A failure
    /// before the seed is recovered leaves nothing; `HeldElsewhere` after it
    /// keeps the seed (`keys` becomes present). `cancel` removes a
    /// recovered seed like a created one.
    ///
    /// # Errors
    /// `InvalidInput` with the seed present or without a PIN service;
    /// `InvalidRecoveryData`, `WrongPin`, `TooManyAttempts`,
    /// `ServiceUnavailable` from opening; the registration's errors.
    #[cfg(any(target_arch = "wasm32", test))]
    pub(crate) async fn recover(
        &self,
        data: &[u8],
        pin: &str,
        options: RegisterOptions,
    ) -> Result<Arc<Client>, KupferdrahtError> {
        let setup = self.setup()?;
        if setup.seed_present.load(Ordering::Acquire) {
            return Err(KupferdrahtError::InvalidInput(
                "the account's seed is here: register the device instead".into(),
            ));
        }
        let services = setup.config.recovery_services.as_ref().ok_or_else(|| {
            KupferdrahtError::InvalidInput(
                "recovery needs a PIN service (storage.pinServiceUrl)".into(),
            )
        })?;
        let session = crate::storage::remote::wire_session::WireSession::Login(
            zeroize::Zeroizing::new(self.inner.login_access_token().to_owned()),
        );
        // Step 120 (ADR-0040): the data must be the slot's device's.
        let client_id = self.slot_label_for_recovery(&session).await?;
        let device = crate::recovery::flow::RecoveryDevice {
            account_id: self.user_id(),
            client_id: &client_id,
        };
        let seed = crate::recovery::flow::open_for_account(
            services,
            &session,
            device,
            data,
            pin,
            &mut rand::rngs::OsRng,
        )
        .await?;
        self.install_account_seed(&seed).await?;
        setup.recovered_seed.store(true, Ordering::Release);
        self.register_device_with(options).await
    }

    /// Step 120 (ADR-0040): the label of the account's storage slot — the
    /// device recovery data must belong to; `oldDevice` without a device
    /// or while one is being set up.
    ///
    /// # Errors
    /// `InvalidRecoveryData` (`oldDevice`); the slot query's errors.
    async fn slot_label_for_recovery(
        &self,
        session: &crate::storage::remote::wire_session::WireSession,
    ) -> Result<String, KupferdrahtError> {
        let services = self
            .setup()?
            .config
            .recovery_services
            .as_ref()
            .ok_or_else(|| {
                KupferdrahtError::InvalidInput(
                    "recovery needs a PIN service (storage.pinServiceUrl)".into(),
                )
            })?;
        match crate::recovery::flow::account_slot(services, session).await? {
            Some(slot) if !slot.label.is_empty() => Ok(slot.label),
            _ => Err(crate::recovery::data::invalid("oldDevice")),
        }
    }

    /// A first setup: create the account's seed and install it.
    async fn create_account_seed(&self) -> Result<(), KupferdrahtError> {
        self.install_account_seed(&generate_seed()).await?;
        self.setup()?.created_seed.store(true, Ordering::Release);
        Ok(())
    }

    /// Remove keys and databases the account has without a seed (builds
    /// before ADR-0035), then store `seed` durably as the account's.
    async fn install_account_seed(&self, seed: &[u8; SEED_LEN]) -> Result<(), KupferdrahtError> {
        let setup = self.setup()?;
        let home = &setup.config.home;
        let keystore = &setup.config.keystore;
        let account_id = self.user_id();
        forget_account_secrets(keystore, home.home_id(), account_id)?;
        #[cfg(target_arch = "wasm32")]
        if !crate::storage::storage_server_configured() {
            crate::storage::prepare_account_databases(home, account_id).await?;
        }
        delete_leftover_account_databases(home, account_id)?;
        store_seed_durably(keystore, home.home_id(), account_id, seed).await?;
        setup.seed_present.store(true, Ordering::Release);
        debug_assert!(
            has_seed(keystore, home.home_id(), account_id)?,
            "the seed is stored"
        );
        Ok(())
    }

    /// Step 120 (ADR-0040): a new device claims the account's storage slot —
    /// another device holding it is replaced, its databases deleted — and
    /// retires that device's Wire client, best effort (a client that stays
    /// is an entry in the device list, nothing more).
    #[cfg(target_arch = "wasm32")]
    async fn claim_device_slot(&self) -> Result<(), KupferdrahtError> {
        let setup = self.setup()?;
        let replaced =
            crate::storage::claim_account_slot(&setup.config.home.account_db(self.user_id()))?;
        let Some(before) = &setup.slot else {
            return Ok(());
        };
        // A slot without a label: its device stopped during its setup.
        let replaced = if before.label.is_empty() {
            None
        } else {
            replaced.or_else(|| Some(before.label.clone()))
        };
        let devices = self.inner.devices().await?;
        for id in crate::storage::remote::slot::clients_to_retire(&devices, replaced.as_deref(), "")
        {
            if let Err(e) = self.inner.delete_device(&id).await {
                tracing::warn!(error = %e, "the replaced device's Wire client stays");
            }
        }
        Ok(())
    }

    /// Step 120 (ADR-0040): label the account's slot with the registered
    /// device — nothing outside a seed home.
    #[cfg(target_arch = "wasm32")]
    pub(super) fn label_device_slot(&self, client: &Client) {
        if let Some(setup) = &self.first_setup {
            crate::manager::remote_resume::label_device_slot(&setup.config.home, client);
        }
    }

    /// Step 111b/c: on the storage server, prepare the account's databases
    /// with the login's session — created when missing, taken over only
    /// when asked. Nothing in the local mode.
    #[cfg(target_arch = "wasm32")]
    async fn prepare_remote_databases(&self, take_over: bool) -> Result<(), KupferdrahtError> {
        let setup = self.setup()?;
        let session = crate::storage::WireSession::Login(zeroize::Zeroizing::new(
            self.inner.login_access_token().to_owned(),
        ));
        crate::storage::prepare_remote_account(
            &setup.config.home,
            &setup.config.keystore,
            self.user_id(),
            session,
            true,
            take_over,
        )
        .await
    }

    /// Step 121 (ADR-0040): forget the account's keys in this browser,
    /// because an open its storage slot refused means they belong to a
    /// device another setup replaced.
    ///
    /// ADR-0040 decided this for `resume`, and its consequences promise that
    /// a replaced browser "learns it (`DeviceRetired`) and resets itself". A
    /// login is the other way in and did not reset: the keys stayed, every
    /// further login met `DeviceRetired` again, and nothing a consumer can
    /// call drops them — `purge` with `purge_local` needs a `Client`, which
    /// such a browser can no longer get. A tester of a browser consumer sat
    /// in exactly that dead end.
    ///
    /// The registry entry needs no removal: `accounts()` lists only accounts
    /// that have a seed.
    ///
    /// # Errors
    /// Keystore and file errors.
    pub(crate) async fn forget_retired_device(&self) -> Result<(), KupferdrahtError> {
        let setup = self.setup()?;
        forget_account_secrets(
            &setup.config.keystore,
            setup.config.home.home_id(),
            self.user_id(),
        )?;
        // Durable before this returns: the keystore persists in the background
        // (Step 104), and a seed that outlives the forgetting would send the
        // next login into the same refusal.
        crate::keystore::flush_pending_writes().await
    }

    /// Abandon the login: drop its state — closing whatever it opened — and
    /// remove what a first setup of it created: seed, keys, databases. A
    /// seed it recovered is forgotten with its keys; the databases are the
    /// account's and stay. An account that was here before the login stays
    /// untouched.
    ///
    /// # Errors
    /// Keystore and file errors; `InvalidInput` once the login completed.
    pub(crate) fn cancel(&self) -> Result<(), KupferdrahtError> {
        self.inner.discard()?;
        let Some(setup) = &self.first_setup else {
            return Ok(());
        };
        let created = setup.created_seed.load(Ordering::Acquire);
        if !created && !setup.recovered_seed.load(Ordering::Acquire) {
            return Ok(());
        }
        let home = &setup.config.home;
        forget_account_secrets(&setup.config.keystore, home.home_id(), self.user_id())?;
        if created {
            delete_account_databases(home, self.user_id())?;
        }
        Ok(())
    }

    fn setup(&self) -> Result<&FirstSetup, KupferdrahtError> {
        debug_assert!(
            self.first_setup.is_some(),
            "Kupferdraht::login attaches the first setup of a seed home"
        );
        self.first_setup.as_ref().ok_or_else(|| {
            KupferdrahtError::InvalidInput("this login has no first setup: not a seed home".into())
        })
    }
}
