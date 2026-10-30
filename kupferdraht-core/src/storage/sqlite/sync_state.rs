// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `sync_state` table CRUD. Singleton row (PK = 1) tracking the
//! Wire `synchronization` cursor across `/v14/notifications`
//! drains. No encrypted columns.

use rusqlite::{OptionalExtension, params};

use crate::error::KupferdrahtError;
use crate::storage::records::SyncStateRecord;

use super::SqliteStore;

pub(super) fn save_sync_state(
    store: &SqliteStore,
    state: &SyncStateRecord,
) -> Result<(), KupferdrahtError> {
    let conn = store.lock()?;
    conn.execute(
        "INSERT INTO sync_state (
            id, sync_marker_uuid, last_event_id, last_connected_at_ms,
            last_key_package_check_at_ms
         ) VALUES (1, ?1, ?2, ?3, ?4)
         ON CONFLICT (id) DO UPDATE SET
            sync_marker_uuid             = excluded.sync_marker_uuid,
            last_event_id                = excluded.last_event_id,
            last_connected_at_ms         = excluded.last_connected_at_ms,
            last_key_package_check_at_ms = excluded.last_key_package_check_at_ms",
        params![
            state.sync_marker_uuid,
            state.last_event_id,
            state.last_connected_at_ms,
            state.last_key_package_check_at_ms,
        ],
    )
    .map_err(|e| KupferdrahtError::Storage(format!("save_sync_state: {e}")))?;
    Ok(())
}

pub(super) fn load_sync_state(
    store: &SqliteStore,
) -> Result<Option<SyncStateRecord>, KupferdrahtError> {
    let conn = store.lock()?;
    conn.query_row(
        "SELECT sync_marker_uuid, last_event_id, last_connected_at_ms,
                last_key_package_check_at_ms
         FROM sync_state WHERE id = 1",
        [],
        |row| {
            Ok(SyncStateRecord {
                sync_marker_uuid: row.get(0)?,
                last_event_id: row.get(1)?,
                last_connected_at_ms: row.get(2)?,
                last_key_package_check_at_ms: row.get(3)?,
            })
        },
    )
    .optional()
    .map_err(|e| KupferdrahtError::Storage(format!("load_sync_state: {e}")))
}
