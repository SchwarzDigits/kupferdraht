// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 95d — the persisted per-room device set (`room_devices`, V29):
//! the baseline a device-set change is diffed against.

use rusqlite::params;

use crate::error::KupferdrahtError;
use crate::rooms::{DeviceCredential, RoomDevice};

use super::SqliteStore;

const BASIC: &str = "basic";
const X509: &str = "x509";

/// The persisted device set of `room_id`; `None` when no row exists.
pub(super) fn load_room_devices(
    store: &SqliteStore,
    room_id: &str,
) -> Result<Option<Vec<RoomDevice>>, KupferdrahtError> {
    let conn = store.lock()?;
    let mut stmt = conn
        .prepare(
            "SELECT user_id, device_id, credential, thumbprint
             FROM room_devices WHERE room_id = ?1",
        )
        .map_err(|e| KupferdrahtError::Storage(format!("load_room_devices prepare: {e}")))?;
    let devices = stmt
        .query_map(params![room_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(|e| KupferdrahtError::Storage(format!("load_room_devices query: {e}")))?
        .map(|row| {
            row.map(device_from_columns)
                .map_err(|e| KupferdrahtError::Storage(format!("load_room_devices row: {e}")))
        })
        .collect::<Result<Vec<RoomDevice>, KupferdrahtError>>()?;
    Ok((!devices.is_empty()).then_some(devices))
}

/// Replace the persisted device set of `room_id` in one transaction.
pub(super) fn replace_room_devices(
    store: &SqliteStore,
    room_id: &str,
    devices: &[RoomDevice],
) -> Result<(), KupferdrahtError> {
    let mut conn = store.lock()?;
    let tx = super::write_tx::begin(&mut conn)
        .map_err(|e| KupferdrahtError::Storage(format!("replace_room_devices begin tx: {e}")))?;
    tx.execute(
        "DELETE FROM room_devices WHERE room_id = ?1",
        params![room_id],
    )
    .map_err(|e| KupferdrahtError::Storage(format!("replace_room_devices delete: {e}")))?;
    for device in devices {
        tx.execute(
            "INSERT OR IGNORE INTO room_devices
                 (room_id, user_id, device_id, credential, thumbprint)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                room_id,
                device.user_id.as_deref().unwrap_or_default(),
                device.device_id.as_deref().unwrap_or_default(),
                credential_column(device.credential),
                device.thumbprint.as_deref().unwrap_or_default(),
            ],
        )
        .map_err(|e| KupferdrahtError::Storage(format!("replace_room_devices insert: {e}")))?;
    }
    tx.commit()
        .map_err(|e| KupferdrahtError::Storage(format!("replace_room_devices commit: {e}")))
}

fn credential_column(credential: DeviceCredential) -> &'static str {
    match credential {
        DeviceCredential::Basic => BASIC,
        DeviceCredential::X509 => X509,
    }
}

/// A row back into a [`RoomDevice`]; empty text means "none". An unknown
/// credential value (a row written by a newer build) reads as X.509 —
/// the unverified side.
fn device_from_columns(
    (user_id, device_id, credential, thumbprint): (String, String, String, String),
) -> RoomDevice {
    let non_empty = |s: String| (!s.is_empty()).then_some(s);
    RoomDevice {
        user_id: non_empty(user_id),
        device_id: non_empty(device_id),
        credential: if credential == BASIC {
            DeviceCredential::Basic
        } else {
            DeviceCredential::X509
        },
        thumbprint: non_empty(thumbprint),
    }
}
