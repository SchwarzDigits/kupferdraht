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
//!   a new display name.
//!
//! matrix-rust-sdk analogs: `Client::devices`,
//! `Client::delete_devices`, `Client::rename_device`.

use crate::client::Client;
use crate::error::KupferdrahtError;
use crate::wire::dto::{ClientClass, WireClient};
use crate::wire::time::parse_time_ms;

/// A device registered for the account.
///
/// matrix-rust-sdk's `Device` carries a richer surface (keys,
/// verification state); Kupferdraht's 1d scope is intentionally narrow:
/// list / delete / rename. Cryptographic identity surfaces stay
/// out until 1d is needed for that.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct DeviceInfo {
    pub device_id: String,
    pub kind: DeviceKind,
    /// The name the user gave the device, if any.
    pub display_name: Option<String>,
    /// The device's model as it registered itself, if it named one.
    pub model: Option<String>,
    /// When the device was registered, in milliseconds since the UNIX
    /// epoch; `None` when the backend did not say.
    pub registered_at_ms: Option<i64>,
    /// When the device was last seen, in milliseconds since the UNIX
    /// epoch; `None` when the backend did not say.
    pub last_seen_ms: Option<i64>,
    /// Whether this is the device the `Client` runs on.
    pub is_own: bool,
}

/// What kind of device a registered device is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum DeviceKind {
    Phone,
    Tablet,
    Desktop,
    /// A device that keeps a copy of the account's messages for a legal
    /// hold.
    LegalHold,
    /// The backend named no kind, or one this release does not know.
    Other,
}

impl From<ClientClass> for DeviceKind {
    fn from(class: ClientClass) -> Self {
        match class {
            ClientClass::Phone => Self::Phone,
            ClientClass::Tablet => Self::Tablet,
            ClientClass::Desktop => Self::Desktop,
            ClientClass::Legalhold => Self::LegalHold,
        }
    }
}

impl DeviceInfo {
    pub(crate) fn from_wire(c: WireClient, own_device_id: &str) -> Self {
        let is_own = c.id == own_device_id;
        let kind = c
            .class
            .as_deref()
            .and_then(ClientClass::from_wire)
            .map_or(DeviceKind::Other, DeviceKind::from);
        Self {
            device_id: c.id,
            kind,
            display_name: c.label,
            model: c.model,
            registered_at_ms: c.time.as_deref().and_then(parse_time_ms),
            last_seen_ms: c.last_active.as_deref().and_then(parse_time_ms),
            is_own,
        }
    }
}

impl Client {
    /// `GET /v14/clients` — list every device registered for the
    /// authenticated user. The current device is flagged
    /// (`DeviceInfo::is_own == true`).
    ///
    /// Wire's 7-device cap means the returned `Vec` has length
    /// at most 7.
    ///
    /// # Errors
    /// Transport / backend errors bubble up as `KupferdrahtError::Network`
    /// / `BackendRejected` / `Encoding`.
    pub async fn devices(&self) -> Result<Vec<DeviceInfo>, KupferdrahtError> {
        let own_id = self.device_id().unwrap_or_default().to_string();
        let raw = self.http_list_devices(&self.current_access_token()).await?;
        Ok(raw
            .into_iter()
            .map(|c| DeviceInfo::from_wire(c, &own_id))
            .collect())
    }

    /// `DELETE /v14/clients/{device_id}` — revoke a registered
    /// device. Wire re-authenticates the delete by the account's
    /// password, and an account without one — an SSO user's — by
    /// nothing (Step 164, `kalium-readings.md` #31): `None` or an
    /// empty password sends none, and the backend decides. An
    /// account with a password that sends none gets
    /// `BackendRejected` naming `missing-auth`.
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
    /// * `BackendRejected` from Wire (`missing-auth`, a wrong or an
    ///   unwanted password as `invalid-credentials`, unknown id, …).
    /// * Network / encoding failures bubble up untyped.
    pub async fn delete_device(
        &self,
        device_id: &str,
        password: Option<&str>,
    ) -> Result<(), KupferdrahtError> {
        if device_id.is_empty() {
            return Err(KupferdrahtError::InvalidInput(
                "delete_device: device_id must be non-empty".into(),
            ));
        }
        if Some(device_id) == self.device_id() {
            return Err(KupferdrahtError::InvalidInput(
                "delete_device: refusing to delete the current device — \
                 use Client::logout(purge_device=true) instead"
                    .into(),
            ));
        }
        self.http_delete_device(
            &self.current_access_token(),
            device_id,
            given_password(password),
        )
        .await
    }

    /// `PUT /v14/clients/{device_id}` — give a registered device a new
    /// display name. Either the current device or another of the user's
    /// devices.
    ///
    /// # Errors
    /// * `InvalidInput` if `device_id` or `display_name` is empty.
    /// * `BackendRejected` on non-2xx from Wire.
    pub async fn rename_device(
        &self,
        device_id: &str,
        display_name: &str,
    ) -> Result<(), KupferdrahtError> {
        if device_id.is_empty() {
            return Err(KupferdrahtError::InvalidInput(
                "rename_device: device_id must be non-empty".into(),
            ));
        }
        if display_name.is_empty() {
            return Err(KupferdrahtError::InvalidInput(
                "rename_device: display_name must be non-empty".into(),
            ));
        }
        self.http_rename_device(&self.current_access_token(), device_id, display_name)
            .await
    }
}

/// The password a device delete sends: an empty one is none, so a caller's
/// empty answer for an account without a password (Step 164) is not refused
/// as a wrong password.
pub(crate) fn given_password(password: Option<&str>) -> Option<&str> {
    password.filter(|p| !p.is_empty())
}
