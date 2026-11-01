// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Signing out and forgetting an account (Step 114 for the browser,
//! ADR-0035; Step 78 natively): `logout` with the purge options
//! `purge_device` (the device at the backend and, in the browser's server
//! mode, its databases there) and `purge_local` (the account's seed, keys,
//! local databases and registry entry on this device), and
//! `forget_account` for an account without a session. The backend first —
//! a refused password leaves everything as it was — then the account is
//! marked signed out, its two databases are closed and deleted where they
//! are the device's, and the account is forgotten here.

use std::sync::Arc;

use super::Kupferdraht;
use crate::client::Client;
use crate::error::KupferdrahtError;
use crate::keystore::flush_pending_writes;
use crate::storage::account_keys::{forget_account_secrets, has_seed};
use crate::storage::delete_account_databases;

/// What a `logout` removes besides the session (ADR-0035). The default —
/// nothing — is the plain logout: the account's keys and data stay for the
/// next login.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, uniffi::Record)]
pub struct PurgeOptions {
    /// Delete the device at the backend (needs the password) and, in the
    /// browser's server mode, its databases on the storage server.
    pub purge_device: bool,
    /// Forget the account on this device: seed, keys, local databases,
    /// registry entry.
    pub purge_local: bool,
}

impl Kupferdraht {
    /// Log `client` out with `options` (ADR-0035): revoke the session at
    /// the backend ([`Client::logout`]), then mark the account signed out —
    /// it is no longer the active account, and this manager no longer
    /// hands the client out. Without a purge everything else stays: the
    /// registry entry, the databases and the keys, so the next login
    /// reuses the device. With one, the client is closed afterwards (its
    /// store refuses every operation with `SessionExpired`): `purge_device`
    /// deletes the device at the backend first, `purge_local` forgets the
    /// account here as [`Kupferdraht::forget_account`] does.
    ///
    /// # Errors
    /// `InvalidInput` for `purge_device` without a password; the backend's
    /// (`BackendRejected` for a wrong password, or for an already revoked
    /// session without `purge_device`) — then nothing local has changed;
    /// storage and keystore errors of the purge.
    pub async fn logout(
        &self,
        client: &Arc<Client>,
        options: PurgeOptions,
        password: Option<&str>,
    ) -> Result<(), KupferdrahtError> {
        client.logout(options.purge_device, password).await?;
        let account_id = client.account().user_id().as_str().to_owned();
        self.clients.lock().await.remove(&account_id);
        self.clear_active_account(&account_id).await?;
        if !options.purge_device && !options.purge_local {
            return Ok(());
        }
        client.close_storage()?;
        if deletes_databases(options) {
            delete_account_databases(&self.home, &account_id)?;
            #[cfg(target_arch = "wasm32")]
            self.release_device_slot(&account_id, options)?;
        }
        if options.purge_local {
            self.forget_secrets_and_entry(&account_id).await?;
        }
        Ok(())
    }

    /// Forget `account_id` on this device without a session (Step 78): a
    /// client this manager still holds is closed first, then the account's
    /// databases are deleted where they are the device's, its secrets and
    /// its registry entry go, and it is no longer the active account. The
    /// device at the backend is not touched — that needs a session
    /// ([`Kupferdraht::logout`] with `purge_device`, or
    /// [`Client::delete_device`] from another device). An account whose
    /// directory is already gone is finished, not refused.
    ///
    /// # Errors
    /// `InvalidInput` for an account the registry does not know — an
    /// already forgotten one included; storage and keystore errors.
    pub async fn forget_account(&self, account_id: &str) -> Result<(), KupferdrahtError> {
        let known = self
            .global
            .list_accounts()
            .await?
            .into_iter()
            .any(|a| a.account_id == account_id);
        if !known {
            return Err(KupferdrahtError::InvalidInput(format!(
                "unknown account: {account_id}"
            )));
        }
        let live = self
            .clients
            .lock()
            .await
            .remove(account_id)
            .and_then(|weak| weak.upgrade());
        if let Some(client) = live {
            client.close_storage()?;
        }
        let local = PurgeOptions {
            purge_device: false,
            purge_local: true,
        };
        if deletes_databases(local) {
            delete_account_databases(&self.home, account_id)?;
        }
        self.forget_secrets_and_entry(account_id).await
    }

    /// Clear the active account when it is `account_id`.
    async fn clear_active_account(&self, account_id: &str) -> Result<(), KupferdrahtError> {
        if self.global.active_account().await?.as_deref() == Some(account_id) {
            self.global.set_active_account(None).await?;
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

    /// Forget `account_id`'s secrets and registry entry: its seed and keys
    /// (durably gone before the registry entry goes), the entry, and the
    /// active account when it was this one.
    pub(super) async fn forget_secrets_and_entry(
        &self,
        account_id: &str,
    ) -> Result<(), KupferdrahtError> {
        forget_account_secrets(&self.keystore, self.home.home_id(), account_id)?;
        flush_pending_writes().await?;
        self.global.remove_account(account_id).await?;
        self.clear_active_account(account_id).await?;
        debug_assert!(
            !has_seed(&self.keystore, self.home.home_id(), account_id)?,
            "the account's seed is gone"
        );
        Ok(())
    }
}

/// Whether a purge with `options` deletes the account's databases: where
/// they are the device's — on the storage server with `purge_device`, on
/// this device (native, or the browser's local mode) with `purge_local`.
fn deletes_databases(options: PurgeOptions) -> bool {
    #[cfg(target_arch = "wasm32")]
    if crate::storage::storage_server_configured() {
        return options.purge_device;
    }
    options.purge_local
}
