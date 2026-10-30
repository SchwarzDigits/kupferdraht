// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `Client` device-management surface (Step 1d).
//!
//! Wire enforces a hard 7-device-per-user cap; once it's hit,
//! `POST /v14/clients` (Step 1b) fails with
//! `400 too-many-clients`. Kupferdraht needs a CLI-driven escape
//! hatch so users don't have to log into the Wire web UI to
//! delete an old device. This module provides:
//!
//! - [`Client::devices`] — `GET /v14/clients`, returns one
//!   [`DeviceInfo`] per registered device.
//! - [`Client::delete_device`] — `DELETE /v14/clients/{id}`
//!   with the user's password (Wire's threat model treats this
//!   as account-level).
//! - [`Client::rename_device`] — `PUT /v14/clients/{id}` with
//!   a new `label`.
//!
//! matrix-rust-sdk analogs: `Client::devices`,
//! `Client::delete_devices`, `Client::rename_device`.

use crate::client::Client;
use crate::error::KupferdrahtError;
use crate::wire::dto::WireClient;

/// User-visible device record. Subset of Wire's `Client` schema.
///
/// matrix-rust-sdk's `Device` carries a richer surface (keys,
/// verification state); Kupferdraht's 1d scope is intentionally narrow:
/// list / delete / rename. Cryptographic identity surfaces stay
/// out until 1d is needed for that.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct DeviceInfo {
    pub device_id: String,
    pub class: Option<String>,
    pub label: Option<String>,
    pub model: Option<String>,
    pub registered_at: Option<String>,
    pub last_active: Option<String>,
    pub is_self: bool,
}

impl DeviceInfo {
    pub(crate) fn from_wire(c: WireClient, self_device_id: &str) -> Self {
        let is_self = c.id == self_device_id;
        Self {
            device_id: c.id,
            class: c.class,
            label: c.label,
            model: c.model,
            registered_at: c.time,
            last_active: c.last_active,
            is_self,
        }
    }
}

impl Client {
    /// `GET /v14/clients` — list every device registered for the
    /// authenticated user. The current device is flagged
    /// (`DeviceInfo::is_self == true`).
    ///
    /// Wire's 7-device cap means the returned `Vec` has length
    /// at most 7.
    ///
    /// # Errors
    /// Transport / backend errors bubble up as `KupferdrahtError::Network`
    /// / `BackendRejected` / `Encoding`.
    pub async fn devices(&self) -> Result<Vec<DeviceInfo>, KupferdrahtError> {
        let self_id = self.device_id().unwrap_or_default().to_string();
        let raw = self.http_list_clients(&self.current_access_token()).await?;
        Ok(raw
            .into_iter()
            .map(|c| DeviceInfo::from_wire(c, &self_id))
            .collect())
    }

    /// `DELETE /v14/clients/{device_id}` — revoke a registered
    /// device. Wire requires the user's password for this call;
    /// the CLI prompts via `--password-stdin` (consistent with
    /// `logout --purge-device`).
    ///
    /// Caller is responsible for not deleting the *current* device
    /// — that's a logout flow (Step 6e), and revoking your own
    /// access token mid-call leaves the local DB in a state the
    /// next operation cannot recover from. To prevent accidental
    /// self-revocation, this function refuses to act on
    /// `self.device_id()`. Use `Client::logout(purge_device=true)`
    /// once Step 6e is merged.
    ///
    /// # Errors
    /// * `InvalidInput` if `device_id` matches the current device.
    /// * `BackendRejected` from Wire (wrong password, unknown id, …).
    /// * Network / encoding failures bubble up untyped.
    pub async fn delete_device(
        &self,
        device_id: &str,
        password: &str,
    ) -> Result<(), KupferdrahtError> {
        if device_id.is_empty() {
            return Err(KupferdrahtError::InvalidInput(
                "delete_device: device_id must be non-empty".into(),
            ));
        }
        if password.is_empty() {
            return Err(KupferdrahtError::InvalidInput(
                "delete_device: password must be non-empty".into(),
            ));
        }
        if Some(device_id) == self.device_id() {
            return Err(KupferdrahtError::InvalidInput(
                "delete_device: refusing to delete the current device — \
                 use Client::logout(purge_device=true) instead"
                    .into(),
            ));
        }
        self.http_delete_client(&self.current_access_token(), device_id, password)
            .await
    }

    /// `PUT /v14/clients/{device_id}` with a `label` body — rename
    /// a registered device. Either the current device or another
    /// of the user's devices.
    ///
    /// # Errors
    /// * `InvalidInput` if `device_id` or `label` is empty.
    /// * `BackendRejected` on non-2xx from Wire.
    pub async fn rename_device(
        &self,
        device_id: &str,
        label: &str,
    ) -> Result<(), KupferdrahtError> {
        if device_id.is_empty() {
            return Err(KupferdrahtError::InvalidInput(
                "rename_device: device_id must be non-empty".into(),
            ));
        }
        if label.is_empty() {
            return Err(KupferdrahtError::InvalidInput(
                "rename_device: label must be non-empty".into(),
            ));
        }
        self.http_update_client_label(&self.current_access_token(), device_id, label)
            .await
    }
}
