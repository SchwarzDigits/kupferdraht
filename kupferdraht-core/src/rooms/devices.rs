// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 95d (ADR-0027 follow-through) — the read-only device
//! verification surface.
//!
//! A room's devices are the leaves of its MLS group: who they belong to
//! (the W-4 identity of a Basic credential; unparsed for an e2ei X.509
//! leaf) and the MLS thumbprint of each leaf's signature key — the RFC
//! 7638 JWK thumbprint official Wire apps show as "MLS Thumbprint", so a
//! user can compare devices across clients. Every change of a room's
//! device set (a leaf added, removed, or re-keyed) is reported to the
//! [`RoomDevicesListener`]s as a diff against the last persisted set, so
//! a change is caught even when it happened while the app was closed.
//! Under `BasicCredential`s the backend distributes the keys: this makes
//! an injected device *visible*, it does not prevent it.

/// The credential type of a device's MLS leaf.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, uniffi::Enum)]
pub enum DeviceCredential {
    /// A Wire `BasicCredential` (`{uuid}:{client_id}@{domain}`).
    Basic,
    /// An e2ei X.509 credential — its identity is not parsed yet.
    X509,
}

/// One device (MLS leaf) of a room.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, uniffi::Record)]
pub struct RoomDevice {
    /// Qualified user id; `None` for an X.509 leaf or a Basic leaf whose
    /// identity is not Wire's W-4 shape.
    pub user_id: Option<String>,
    /// Wire `client_id`; `None` exactly when `user_id` is.
    pub device_id: Option<String>,
    /// The leaf's credential type.
    pub credential: DeviceCredential,
    /// RFC 7638 JWK thumbprint (SHA-256, base64url, no padding) of the
    /// leaf's signature key — Wire's "MLS Thumbprint". `None` when the
    /// key is not a P-256 SEC1 point.
    pub thumbprint: Option<String>,
}

/// A change of one room's device set, relative to the last set Kupferdraht
/// persisted for that room.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct RoomDevicesChange {
    /// Qualified room id.
    pub room_id: String,
    /// Devices that joined (or re-keyed: the new leaf).
    pub added: Vec<RoomDevice>,
    /// Devices that left (or re-keyed: the old leaf).
    pub removed: Vec<RoomDevice>,
}

/// Observer for device-set changes across all rooms, registered with
/// [`crate::Client::subscribe_device_changes`].
pub trait RoomDevicesListener: Send + Sync {
    /// One call per room whose device set changed.
    fn on_devices_changed(&self, change: RoomDevicesChange);
}

impl crate::rooms::Room {
    /// The room's devices — one entry per leaf of its MLS group, the
    /// client's own device included.
    ///
    /// # Errors
    /// `KupferdrahtError::InvalidInput` when the room has no local MLS group
    /// (not joined yet); `KupferdrahtError::Storage` from the group load.
    pub async fn devices(&self) -> Result<Vec<RoomDevice>, crate::error::KupferdrahtError> {
        self.owning_client("devices")?
            .room_devices(&self.id())
            .await
    }
}

/// A group leaf as a consumer-facing [`RoomDevice`].
pub(crate) fn room_device_from_leaf(leaf: crate::encryption::LeafDevice) -> RoomDevice {
    let (user_id, device_id) = match leaf.identity {
        Ok(identity) => (Some(identity.user_id), Some(identity.device_id)),
        Err(_) => (None, None),
    };
    RoomDevice {
        user_id,
        device_id,
        credential: if leaf.basic {
            DeviceCredential::Basic
        } else {
            DeviceCredential::X509
        },
        thumbprint: leaf.thumbprint,
    }
}

/// `added` / `removed` between the last persisted set and the current
/// one; `None` when nothing changed.
#[must_use]
pub(crate) fn device_set_change(
    room_id: &str,
    before: &[RoomDevice],
    after: &[RoomDevice],
) -> Option<RoomDevicesChange> {
    let mut added: Vec<RoomDevice> = after
        .iter()
        .filter(|d| !before.contains(d))
        .cloned()
        .collect();
    let mut removed: Vec<RoomDevice> = before
        .iter()
        .filter(|d| !after.contains(d))
        .cloned()
        .collect();
    if added.is_empty() && removed.is_empty() {
        return None;
    }
    added.sort();
    removed.sort();
    Some(RoomDevicesChange {
        room_id: room_id.to_string(),
        added,
        removed,
    })
}

#[cfg(test)]
mod tests {
    use super::{DeviceCredential, RoomDevice, device_set_change};

    fn device(user: &str, dev: &str, tp: &str) -> RoomDevice {
        RoomDevice {
            user_id: Some(user.into()),
            device_id: Some(dev.into()),
            credential: DeviceCredential::Basic,
            thumbprint: Some(tp.into()),
        }
    }

    #[test]
    fn an_unchanged_set_is_no_change() {
        let set = vec![device("bob@d", "b1", "tp1"), device("me@d", "m1", "tp2")];
        let reordered = vec![set[1].clone(), set[0].clone()];
        assert_eq!(device_set_change("r@d", &set, &reordered), None);
    }

    #[test]
    fn additions_and_removals_are_reported() {
        let before = vec![device("bob@d", "b1", "tp1"), device("carol@d", "c1", "tp3")];
        let after = vec![device("bob@d", "b1", "tp1"), device("dave@d", "d1", "tp4")];
        let change = device_set_change("r@d", &before, &after).expect("a change");
        assert_eq!(change.room_id, "r@d");
        assert_eq!(change.added, vec![device("dave@d", "d1", "tp4")]);
        assert_eq!(change.removed, vec![device("carol@d", "c1", "tp3")]);
    }

    #[test]
    fn a_re_keyed_device_is_removed_and_added() {
        let before = vec![device("bob@d", "b1", "old-key")];
        let after = vec![device("bob@d", "b1", "new-key")];
        let change = device_set_change("r@d", &before, &after).expect("a change");
        assert_eq!(change.added, vec![device("bob@d", "b1", "new-key")]);
        assert_eq!(change.removed, vec![device("bob@d", "b1", "old-key")]);
    }
}
