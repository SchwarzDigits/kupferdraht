// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 95d — the wasm projection of the device verification surface:
//! `{ userId, deviceId, credential, thumbprint }` per device and the
//! `{ roomId, added, removed }` change handed to `startSync`'s optional
//! `onDevicesChanged` callback.

use send_wrapper::SendWrapper;
use wasm_bindgen::JsValue;

use crate::rooms::{DeviceCredential, RoomDevice, RoomDevicesChange, RoomDevicesListener};

/// Plain, JS-serializable view of one room device (MLS leaf).
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct DeviceView {
    user_id: Option<String>,
    device_id: Option<String>,
    /// `"basic"` | `"x509"`.
    credential: &'static str,
    /// Wire's "MLS Thumbprint" (RFC 7638, base64url).
    thumbprint: Option<String>,
}

impl From<&RoomDevice> for DeviceView {
    fn from(d: &RoomDevice) -> Self {
        Self {
            user_id: d.user_id.clone(),
            device_id: d.device_id.clone(),
            credential: match d.credential {
                DeviceCredential::Basic => "basic",
                DeviceCredential::X509 => "x509",
            },
            thumbprint: d.thumbprint.clone(),
        }
    }
}

/// Plain, JS-serializable view of a device-set change.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct DevicesChangeView {
    room_id: String,
    added: Vec<DeviceView>,
    removed: Vec<DeviceView>,
}

/// A JS callback as a core [`RoomDevicesListener`] (the `JsEventHandler`
/// shape: `SendWrapper` satisfies `Send + Sync` on the single-threaded
/// browser).
pub(super) struct JsDevicesListener {
    pub(super) on_devices_changed: SendWrapper<js_sys::Function>,
}

impl RoomDevicesListener for JsDevicesListener {
    fn on_devices_changed(&self, change: RoomDevicesChange) {
        let view = DevicesChangeView {
            room_id: change.room_id,
            added: change.added.iter().map(DeviceView::from).collect(),
            removed: change.removed.iter().map(DeviceView::from).collect(),
        };
        if let Ok(val) = serde_wasm_bindgen::to_value(&view) {
            let _ = self.on_devices_changed.call1(&JsValue::NULL, &val);
        }
    }
}
