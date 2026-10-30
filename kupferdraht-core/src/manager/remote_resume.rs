// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Resuming an account whose databases live on the storage server
//! (Step 111b, ADR-0035). **wasm-only.**
//!
//! The databases open only with a storage token, and the token needs a
//! Wire session — but the device id a session rebind names sits in the
//! remote `kupferdraht.db`. So `resume` first renews the session with the
//! browser's login cookie alone (`POST /access` without `client_id`), at
//! the backend the account's registry entry names, and prepares the
//! databases with that token, without creating them: a database the
//! server no longer has is `NothingStored`. A database another instance
//! holds is taken over only when the call asks (Step 111c, ADR-0035):
//! otherwise its open throws `HeldElsewhere`.

use std::sync::{Arc, Weak};

use zeroize::Zeroizing;

use super::Kupferdraht;
use crate::client::Client;
use crate::error::KupferdrahtError;
use crate::storage::{AccountEntry, WireSession};
use crate::wire::{HttpClient, select_api_version};

impl Kupferdraht {
    /// [`Kupferdraht::resume`] in the browser: on the storage server the
    /// account's databases are prepared first — taken over from another
    /// instance with `take_over` — and the resumed client's session renews
    /// their storage token. A live client is returned as it is.
    ///
    /// # Errors
    /// As [`Kupferdraht::resume`], and the preparation's (`SessionExpired`,
    /// `StorageAccessDenied`, `ServiceUnavailable`, …); `HeldElsewhere` for
    /// a database another instance holds, without `take_over`.
    pub(crate) async fn resume_with(
        &self,
        account_id: &str,
        take_over: bool,
    ) -> Result<Arc<Client>, KupferdrahtError> {
        let resumed = self.resume_remote(account_id, take_over).await;
        // Step 120 (ADR-0040): another device holds the account's slot — this
        // browser's keys are a replaced device's. Forget them: the next login
        // offers recovery or a new setup.
        if let Err(KupferdrahtError::DeviceRetired(_)) = &resumed {
            self.forget_account(account_id).await?;
        }
        let client = resumed?;
        label_device_slot(&self.home, &client);
        Ok(client)
    }

    /// Prepare the databases unless a live client has them, then resume.
    async fn resume_remote(
        &self,
        account_id: &str,
        take_over: bool,
    ) -> Result<Arc<Client>, KupferdrahtError> {
        let live = self
            .clients
            .lock()
            .await
            .get(account_id)
            .and_then(Weak::upgrade);
        if live.is_none() {
            self.prepare_remote_resume(account_id, take_over).await?;
        }
        let client = self.resume(account_id).await?;
        attach_client_session(&client);
        Ok(client)
    }

    /// Prepare the server databases of `account_id` with a session the
    /// browser's login cookie renews, at the backend of its registry
    /// entry. Nothing in the local mode.
    ///
    /// # Errors
    /// `SessionExpired` when the cookie is gone or belongs to another
    /// user — log in again; `InvalidInput` for an account the registry
    /// does not list; the Wire call's errors; the preparation's
    /// (`StorageAccessDenied`, `ServiceUnavailable`, …).
    async fn prepare_remote_resume(
        &self,
        account_id: &str,
        take_over: bool,
    ) -> Result<(), KupferdrahtError> {
        if !crate::storage::storage_server_configured() {
            return Ok(());
        }
        let entry = self.registry_entry(account_id).await?;
        let http = HttpClient::new(&entry.server, self.permit_http)?;
        let api_version = select_api_version(&http.api_version().await?.supported)?;
        let outcome = http.access_with_browser_cookie(api_version).await?;
        if entry.account_id.split('@').next() != Some(outcome.user.as_str()) {
            tracing::info!("the browser's login cookie belongs to another user");
            return Err(KupferdrahtError::SessionExpired);
        }
        crate::storage::prepare_remote_account(
            &self.home,
            &self.keystore,
            &entry.account_id,
            WireSession::Login(Zeroizing::new(outcome.access_token)),
            false,
            take_over,
        )
        .await
    }

    async fn registry_entry(&self, account_id: &str) -> Result<AccountEntry, KupferdrahtError> {
        self.global
            .list_accounts()
            .await?
            .into_iter()
            .find(|a| a.account_id == account_id)
            .ok_or_else(|| KupferdrahtError::InvalidInput(format!("unknown account: {account_id}")))
    }
}

/// From now on `client`'s own session renews its storage token.
pub(super) fn attach_client_session(client: &Arc<Client>) {
    let Some(account_id) = client.user_id() else {
        return;
    };
    let refresher: Arc<dyn crate::wire::http::TokenRefresh> = client.clone();
    crate::storage::attach_account_client(account_id.as_str(), Arc::downgrade(&refresher));
}

/// Step 120 (ADR-0040): label the account's storage slot with this device's
/// Wire client id — at every registration and resume, so a setup that
/// stopped before labelling heals at the next start. Best effort: a failure
/// leaves the slot "being set up" until then.
pub(super) fn label_device_slot(home: &crate::storage::KupferdrahtHome, client: &Client) {
    if !crate::storage::storage_server_configured() {
        return;
    }
    let (Some(account), Some(device)) = (client.user_id(), client.device_id()) else {
        return;
    };
    if let Err(e) = crate::storage::label_account_slot(&home.account_db(account.as_str()), device) {
        tracing::warn!(error = %e, "the storage slot stays unlabelled for now");
    }
}
