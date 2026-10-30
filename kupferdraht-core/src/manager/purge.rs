// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Leaving a browser (Step 114, ADR-0035): `logout` with the purge options
//! `purgeDevice` (the device at the backend and, in the server mode, its
//! databases there) and `purgeLocal` (the account's seed, keys, local
//! databases and registry entry in this browser). The backend first — a
//! refused password leaves everything as it was — then the account's two
//! databases are closed, deleted where they are the device's, and the
//! account is forgotten here.

use std::sync::Arc;

use super::Kupferdraht;
use crate::client::Client;
use crate::error::KupferdrahtError;
use crate::keystore::flush_pending_writes;
use crate::storage::account_keys::{forget_account_secrets, has_seed};
use crate::storage::delete_account_databases;

/// What a browser `logout` removes besides the session.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct PurgeOptions {
    /// Delete the device at the backend (needs the password) and, in the
    /// server mode, its databases on the storage server.
    pub(crate) purge_device: bool,
    /// Forget the account in this browser: seed, keys, local databases,
    /// registry entry.
    pub(crate) purge_local: bool,
}

impl Kupferdraht {
    /// Log `client` out with `options` (ADR-0035). Without a purge this is
    /// the native `logout`. With one, the client is closed afterwards: its
    /// store refuses every operation with `SessionExpired`, and the manager
    /// no longer hands it out.
    ///
    /// # Errors
    /// `InvalidInput` for `purge_device` without a password; the backend's
    /// (`BackendRejected` for a wrong password) — then nothing local has
    /// changed; storage and keystore errors of the purge.
    pub(crate) async fn logout_client(
        &self,
        client: &Arc<Client>,
        options: PurgeOptions,
        password: Option<&str>,
    ) -> Result<(), KupferdrahtError> {
        client.logout(options.purge_device, password).await?;
        if !options.purge_device && !options.purge_local {
            return Ok(());
        }
        let account_id = client.account().user_id().as_str().to_owned();
        self.clients.lock().await.remove(&account_id);
        client.close_storage()?;
        if deletes_databases(options) {
            delete_account_databases(&self.home, &account_id)?;
            #[cfg(target_arch = "wasm32")]
            self.release_device_slot(&account_id, options)?;
        }
        if options.purge_local {
            self.forget_account(&account_id).await?;
        }
        Ok(())
    }

    /// Step 120 (ADR-0040): with `purge_device` on the storage server, give
    /// the account's slot up — the next login finds no device. Another
    /// device holding it already means there is nothing of this one left.
    #[cfg(target_arch = "wasm32")]
    fn release_device_slot(
        &self,
        account_id: &str,
        options: PurgeOptions,
    ) -> Result<(), KupferdrahtError> {
        if !options.purge_device || !crate::storage::storage_server_configured() {
            return Ok(());
        }
        match crate::storage::delete_account_slot(&self.home.account_db(account_id)) {
            Err(KupferdrahtError::DeviceRetired(_)) | Ok(()) => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Forget `account_id` in this browser: its seed and keys (stored
    /// durably gone before the registry entry goes), its registry entry,
    /// and the active account when it was this one.
    pub(super) async fn forget_account(&self, account_id: &str) -> Result<(), KupferdrahtError> {
        forget_account_secrets(&self.keystore, self.home.home_id(), account_id)?;
        flush_pending_writes().await?;
        self.global.remove_account(account_id).await?;
        if self.global.active_account().await?.as_deref() == Some(account_id) {
            self.global.set_active_account(None).await?;
        }
        debug_assert!(
            !has_seed(&self.keystore, self.home.home_id(), account_id)?,
            "the account's seed is gone"
        );
        Ok(())
    }
}

/// Whether a purge with `options` deletes the account's databases: where
/// they are the device's — on the storage server with `purge_device`, in
/// this browser's local mode with `purge_local`.
fn deletes_databases(options: PurgeOptions) -> bool {
    #[cfg(target_arch = "wasm32")]
    if crate::storage::storage_server_configured() {
        return options.purge_device;
    }
    options.purge_local
}
