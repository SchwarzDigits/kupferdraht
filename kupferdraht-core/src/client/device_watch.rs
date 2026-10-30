// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 95d — the client half of the device verification surface: the
//! room device list, the own device's MLS thumbprint, and the
//! device-change subscription. Wherever a room's MLS group changes epoch
//! (a merged inbound commit, a Welcome, a (re)join, an own commit) the
//! caller runs [`Client::note_room_devices`]: the current leaf set is
//! diffed against the set persisted for the room, the new set is
//! persisted, and a change is reported to every listener. The baseline
//! is persisted, so a change that happened while no client ran is still
//! reported at the room's next epoch change — and a missed call site
//! only delays a report, it never loses one.

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex as StdMutex, Weak};

use tracing::warn;

use crate::client::Client;
use crate::error::KupferdrahtError;
use crate::rooms::{
    RoomDevice, RoomDevicesChange, RoomDevicesListener, device_set_change, room_device_from_leaf,
};
use crate::timeline::{Subscription, Unregister};

/// The device-change listeners, keyed like the timeline registry.
pub(crate) type DeviceListenerRegistry = StdMutex<HashMap<u64, Arc<dyn RoomDevicesListener>>>;

impl Client {
    /// The MLS thumbprint of this device's signature key — what the
    /// device shows up with in every room's [`crate::Room::devices`]
    /// and what an official Wire app shows as "MLS Thumbprint" for it.
    ///
    /// # Errors
    /// `KupferdrahtError::InvalidInput` before the MLS identity exists (no
    /// registered device); `KupferdrahtError::Generic` when the key has no
    /// thumbprint encoding (never for Kupferdraht's P-256 identity).
    pub fn own_device_thumbprint(&self) -> Result<String, KupferdrahtError> {
        let mls = self.mls_client_required("own_device_thumbprint")?;
        let guard = crate::encryption::lock_mls(mls);
        guard.own_thumbprint().ok_or_else(|| {
            KupferdrahtError::Generic("own signature key has no thumbprint encoding".into())
        })
    }

    /// Register `listener` for device-set changes in every room. Drop
    /// the returned [`Subscription`] (or call `unsubscribe`) to stop.
    /// matrix-rust-sdk analog: `Encryption::devices_stream`.
    pub fn subscribe_device_changes(&self, listener: Arc<dyn RoomDevicesListener>) -> Subscription {
        let id = self
            .timeline_listener_next_id
            .fetch_add(1, Ordering::Relaxed);
        if let Ok(mut guard) = self.device_listeners.lock() {
            guard.insert(id, listener);
        }
        let weak: Weak<DeviceListenerRegistry> = Arc::downgrade(&self.device_listeners);
        let registry: Weak<dyn Unregister> = weak;
        Subscription { id, registry }
    }

    /// The devices of `room_id` — one per leaf of its MLS group, sorted.
    ///
    /// # Errors
    /// `KupferdrahtError::InvalidInput` when the room has no local MLS group;
    /// `KupferdrahtError::Storage` from the group load.
    pub(crate) async fn room_devices(
        &self,
        room_id: &str,
    ) -> Result<Vec<RoomDevice>, KupferdrahtError> {
        let group_id = self.group_id_for_room(room_id).await.ok_or_else(|| {
            KupferdrahtError::InvalidInput(format!(
                "devices: {room_id} has no MLS group_id — run sync_metadata / join the room first"
            ))
        })?;
        let mls = self.mls_client_required("devices")?;
        let leaves = {
            let guard = crate::encryption::lock_mls(mls);
            guard.group_devices(&group_id)?
        };
        let mut devices: Vec<RoomDevice> = leaves.into_iter().map(room_device_from_leaf).collect();
        devices.sort();
        Ok(devices)
    }

    /// Best-effort [`Self::refresh_room_devices`] for an epoch-change
    /// site: a failure is logged, never propagated — the persisted
    /// baseline makes the next change of the room report it instead.
    pub(crate) async fn note_room_devices(&self, room_id: &str) {
        if let Err(e) = self.refresh_room_devices(room_id).await {
            warn!(room_id, error = %e, "device-set refresh failed; reported at the room's next change");
        }
    }

    /// Record a baseline for every cached room that has none yet — run
    /// by `sync_once` after the metadata refresh and BEFORE the drain, so
    /// the baseline is the pre-drain state and a room's first real change
    /// is reported, not swallowed as its first observation (rooms joined
    /// by a pre-95d build, rooms created mid-session, a missed call site).
    /// One indexed read per room; a group load only for a room without a
    /// baseline. Best effort, like [`Self::note_room_devices`].
    pub(crate) async fn ensure_device_baselines(&self) {
        let Some(store) = self.store_handle() else {
            return;
        };
        for room in self.rooms().await {
            let room_id = room.id();
            match store.load_room_devices(&room_id).await {
                Ok(Some(_)) => {}
                Ok(None) => self.note_room_devices(&room_id).await,
                Err(e) => warn!(room_id, error = %e, "device baseline check failed"),
            }
        }
    }

    /// Diff the room's current devices against the persisted baseline,
    /// persist the current set, and report a change to the listeners.
    /// The first observation of a room persists silently.
    async fn refresh_room_devices(&self, room_id: &str) -> Result<(), KupferdrahtError> {
        let Some(store) = self.store_handle() else {
            return Ok(());
        };
        let current = match self.room_devices(room_id).await {
            Ok(devices) => devices,
            // No local group (left, not joined yet): nothing to observe.
            Err(KupferdrahtError::InvalidInput(_)) => return Ok(()),
            Err(e) => return Err(e),
        };
        let baseline = store.load_room_devices(room_id).await?;
        let change = baseline
            .as_deref()
            .and_then(|before| device_set_change(room_id, before, &current));
        if baseline.is_none() || change.is_some() {
            store.replace_room_devices(room_id, &current).await?;
        }
        if let Some(change) = change {
            self.notify_device_listeners(&change);
        }
        Ok(())
    }

    /// Snapshot the listeners, then call each outside the lock (the
    /// `notify_message` shape — a listener may re-enter the client).
    fn notify_device_listeners(&self, change: &RoomDevicesChange) {
        // Step 109 (ADR-0037): inside a drained page, held until it commits.
        if crate::sync::outbox::hold(crate::sync::outbox::Held::Devices(change.clone())).is_some() {
            self.emit_device_change(change);
        }
    }

    /// Hand `change` to the device listeners now.
    pub(crate) fn emit_device_change(&self, change: &RoomDevicesChange) {
        let listeners: Vec<Arc<dyn RoomDevicesListener>> = self
            .device_listeners
            .lock()
            .map(|guard| guard.values().cloned().collect())
            .unwrap_or_default();
        for listener in listeners {
            listener.on_devices_changed(change.clone());
        }
    }
}
