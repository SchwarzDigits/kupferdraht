// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `devices` table CRUD. `mls_public_key` is base64 `TEXT`, not
//! encrypted — the public-key half of the local MLS leaf identity
//! is intentionally readable so `client/resume.rs` can rebuild
//! the openmls signature credential without staging the DEK.

use rusqlite::{OptionalExtension, params};

use crate::error::KupferdrahtError;
use crate::storage::records::DeviceRecord;

use super::SqliteStore;

pub(super) fn save_device(
    store: &SqliteStore,
    device: &DeviceRecord,
) -> Result<(), KupferdrahtError> {
    debug_assert!(!device.user_id.is_empty(), "user_id must be non-empty");
    debug_assert!(!device.client_id.is_empty(), "client_id must be non-empty");
    let conn = store.lock()?;
    conn.execute(
        "INSERT INTO devices (user_id, client_id, mls_public_key)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(user_id, client_id) DO UPDATE SET
             mls_public_key = excluded.mls_public_key",
        params![device.user_id, device.client_id, device.mls_public_key],
    )
    .map_err(|e| KupferdrahtError::Storage(format!("save_device: {e}")))?;
    Ok(())
}

pub(super) fn load_device(
    store: &SqliteStore,
    user_id: &str,
    client_id: &str,
) -> Result<Option<DeviceRecord>, KupferdrahtError> {
    debug_assert!(!user_id.is_empty(), "user_id must be non-empty");
    debug_assert!(!client_id.is_empty(), "client_id must be non-empty");
    let conn = store.lock()?;
    conn.query_row(
        "SELECT mls_public_key FROM devices
         WHERE user_id = ?1 AND client_id = ?2",
        params![user_id, client_id],
        |row| {
            Ok(DeviceRecord {
                user_id: user_id.to_string(),
                client_id: client_id.to_string(),
                mls_public_key: row.get(0)?,
            })
        },
    )
    .optional()
    .map_err(|e| KupferdrahtError::Storage(format!("load_device: {e}")))
}
