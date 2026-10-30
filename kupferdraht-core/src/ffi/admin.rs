// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! FFI wrappers for the account-admin / server-config / utility tail
//! (Step 20c-7).
//!
//! `JoinSummary` / `ResyncSummary` are mirrored because the core types
//! carry a `Vec<(String, String)>` tuple field `UniFFI` can't marshal — the
//! mirror reshapes it to `Vec<FailedJoin>`. The leaf types (`DeviceInfo`,
//! `FeatureConfigEntry`, `UserPropertyEntry`, `FileSharingPolicy`,
//! `ReceiptMode`) are FFI-shaped and derive on the core type.

use std::sync::Arc;

use super::Client;
use super::room::Room;
use super::timeline::TimelineItem;
use crate::client::{
    DeviceInfo, FileSharingPolicy, JoinSummary as CoreJoinSummary, ReceiptMode,
    ResyncSummary as CoreResyncSummary,
};
use crate::error::KupferdrahtError;
use crate::storage::{FeatureConfigEntry, UserPropertyEntry};
use crate::timeline::Mention;

/// One per-room failure in a [`JoinSummary`] / [`ResyncSummary`].
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FailedJoin {
    pub room_id: String,
    pub error: String,
}

fn map_failures(failed: &[(String, String)]) -> Vec<FailedJoin> {
    failed
        .iter()
        .map(|(room_id, error)| FailedJoin {
            room_id: room_id.clone(),
            error: error.clone(),
        })
        .collect()
}

/// Outcome of [`Client::join_all_rooms`].
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct JoinSummary {
    pub attempted: u32,
    pub joined: u32,
    pub already_joined: u32,
    pub failed: Vec<FailedJoin>,
}

impl From<&CoreJoinSummary> for JoinSummary {
    fn from(summary: &CoreJoinSummary) -> Self {
        Self {
            attempted: summary.attempted,
            joined: summary.joined,
            already_joined: summary.already_joined,
            failed: map_failures(&summary.failed),
        }
    }
}

/// Outcome of [`Client::resync_existing_rooms`].
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ResyncSummary {
    pub attempted: u32,
    pub already_current: u32,
    pub resynced: u32,
    pub failed: Vec<FailedJoin>,
}

impl From<&CoreResyncSummary> for ResyncSummary {
    fn from(summary: &CoreResyncSummary) -> Self {
        Self {
            attempted: summary.attempted,
            already_current: summary.already_current,
            resynced: summary.resynced,
            failed: map_failures(&summary.failed),
        }
    }
}

#[uniffi::export]
impl Client {
    /// Raw ECDSA-P256 MLS signature public key (65 bytes).
    #[must_use]
    pub fn mls_signature_public_key(&self) -> Vec<u8> {
        self.inner.mls_signature_public_key().to_vec()
    }

    /// Wire's backend external-sender removal key (raw bytes).
    #[must_use]
    pub fn backend_removal_key_ecdsa_p256(&self) -> Vec<u8> {
        self.inner.backend_removal_key_ecdsa_p256().to_vec()
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl Client {
    /// Revoke the session; `purge_device` also deletes the device
    /// server-side (requires `password`).
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn logout(
        &self,
        purge_device: bool,
        password: Option<String>,
    ) -> Result<(), KupferdrahtError> {
        self.inner.logout(purge_device, password.as_deref()).await
    }

    /// Set the user's display name.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn set_display_name(&self, name: &str) -> Result<(), KupferdrahtError> {
        self.inner.set_display_name(name).await
    }

    /// Set the user's `@handle` (globally unique).
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn set_handle(&self, handle: &str) -> Result<(), KupferdrahtError> {
        self.inner.set_handle(handle).await
    }

    /// Request a change of the user's email. Wire sends a verification
    /// mail to the new address; the change takes effect only after the
    /// user confirms, so this returns once the request is accepted (it
    /// does not update the local profile). `400 invalid-email` and
    /// `409 key-exists` surface as `KupferdrahtError` with the Wire label.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn update_email(&self, email: &str) -> Result<(), KupferdrahtError> {
        self.inner.update_email(email).await
    }

    /// Change the account password (`old_password` re-authenticates).
    /// On success Wire invalidates every device's session, so all
    /// devices must re-login; registered devices and MLS state survive.
    /// `403 invalid-credentials` (wrong old password) and `400`
    /// (policy) surface as `KupferdrahtError` with the Wire label.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn change_password(
        &self,
        old_password: &str,
        new_password: &str,
    ) -> Result<(), KupferdrahtError> {
        self.inner.change_password(old_password, new_password).await
    }

    /// List the user's registered devices.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn devices(&self) -> Result<Vec<DeviceInfo>, KupferdrahtError> {
        self.inner.devices().await
    }

    /// Delete a device (not the current one); requires the password.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn delete_device(
        &self,
        device_id: &str,
        password: &str,
    ) -> Result<(), KupferdrahtError> {
        self.inner.delete_device(device_id, password).await
    }

    /// Rename a device's user-visible label.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn rename_device(
        &self,
        device_id: &str,
        label: &str,
    ) -> Result<(), KupferdrahtError> {
        self.inner.rename_device(device_id, label).await
    }

    /// Snapshot of every team-feature config (read-only).
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn feature_configs(&self) -> Result<Vec<FeatureConfigEntry>, KupferdrahtError> {
        self.inner.feature_configs().await
    }

    /// One feature config by Wire's exact key.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn feature_config(
        &self,
        name: &str,
    ) -> Result<Option<FeatureConfigEntry>, KupferdrahtError> {
        self.inner.feature_config(name).await
    }

    /// Snapshot of every user-property entry (read-only).
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn user_properties(&self) -> Result<Vec<UserPropertyEntry>, KupferdrahtError> {
        self.inner.user_properties().await
    }

    /// One user property by Wire's exact key.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn user_property(
        &self,
        key: &str,
    ) -> Result<Option<UserPropertyEntry>, KupferdrahtError> {
        self.inner.user_property(key).await
    }

    /// The team file-sharing policy (typed projection).
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn file_sharing(&self) -> Result<FileSharingPolicy, KupferdrahtError> {
        self.inner.file_sharing().await
    }

    /// The user's read-receipt mode (typed projection).
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn receipt_mode(&self) -> Result<ReceiptMode, KupferdrahtError> {
        self.inner.receipt_mode().await
    }

    /// Remaining unclaimed MLS key packages for this device (Step 63).
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn key_package_count(&self) -> Result<u64, KupferdrahtError> {
        self.inner.key_package_count().await
    }

    /// Top the key-package pool back up when it has fallen below half
    /// of the 100-package target; returns the number uploaded (Step
    /// 63). The sync surface also runs this automatically once per
    /// 24 h.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn replenish_key_packages(&self) -> Result<u32, KupferdrahtError> {
        self.inner.replenish_key_packages().await
    }

    /// Whether the server's `/v14/events` push delivery is operational
    /// (`consumableNotifications` feature, typed projection — Step
    /// 62c). The transport-choice input for `SyncTransport`.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn server_supports_push_events(&self) -> Result<bool, KupferdrahtError> {
        self.inner.server_supports_push_events().await
    }

    /// Per-room unread counts in one grouped query (Step 67); rooms
    /// without unread messages are absent — treat missing as 0.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn unread_counts(
        &self,
    ) -> Result<std::collections::HashMap<String, u32>, KupferdrahtError> {
        self.inner.unread_counts().await
    }

    /// Rooms that contain a message / edit-event id (local lookup).
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn find_message_room(
        &self,
        target_id: &str,
    ) -> Result<Vec<String>, KupferdrahtError> {
        self.inner.find_message_room(target_id).await
    }

    /// Resolve a single timeline item by id from the local store (stable
    /// id or edit-event id). `None` when the id is not stored locally.
    /// Lightweight — the item carries no overlay data (reactions / read
    /// receipts are empty); use `Room::messages` for a hydrated item.
    /// Resolves a reply's parent for a quote preview.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn find_message_by_id(
        &self,
        message_id: &str,
    ) -> Result<Option<TimelineItem>, KupferdrahtError> {
        Ok(self
            .inner
            .find_message_by_id(message_id)
            .await?
            .as_ref()
            .map(TimelineItem::from))
    }

    /// Resolve `@handle` tokens in `body` to mentions with UTF-16 offsets.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn resolve_mentions_in_body(
        &self,
        body: &str,
    ) -> Result<Vec<Mention>, KupferdrahtError> {
        self.inner.resolve_mentions_in_body(body).await
    }

    /// Join every cached room not already joined locally.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn join_all_rooms(&self) -> Result<JoinSummary, KupferdrahtError> {
        self.inner
            .join_all_rooms()
            .await
            .map(|summary| JoinSummary::from(&summary))
    }

    /// Rejoin one room whose persisted leaf belongs to a prior device.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn resync_room(&self, room_id: &str) -> Result<(), KupferdrahtError> {
        self.inner.resync_room(room_id).await
    }

    /// Resync every cached room whose leaf belongs to a prior device.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn resync_existing_rooms(&self) -> Result<ResyncSummary, KupferdrahtError> {
        self.inner
            .resync_existing_rooms()
            .await
            .map(|summary| ResyncSummary::from(&summary))
    }

    /// Lightweight metadata-only sync (no notifications drain).
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn sync_metadata(&self) -> Result<(), KupferdrahtError> {
        self.inner.sync_metadata().await
    }

    /// Hydrate the in-memory room cache from the store (no network).
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn load_rooms_from_store(&self) -> Result<(), KupferdrahtError> {
        self.inner.load_rooms_from_store().await
    }

    /// Join the room a guest link names, by its `code` and `key` (+ the
    /// link's password, if it has one).
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn join_via_link(
        &self,
        code: String,
        key: String,
        password: Option<String>,
    ) -> Result<Arc<Room>, KupferdrahtError> {
        self.inner
            .join_via_link(code, key, password)
            .await
            .map(Room::wrap)
    }
}
