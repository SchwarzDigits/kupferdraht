// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `server_config` table CRUD. Singleton row (PK = 1) holding the
//! Wire backend URLs + title. No encrypted columns — the values
//! are non-sensitive routing info.

use rusqlite::{OptionalExtension, params};

use crate::error::KupferdrahtError;
use crate::storage::records::ServerConfigRecord;

use super::SqliteStore;

pub(super) fn save_server_config(
    store: &SqliteStore,
    config: &ServerConfigRecord,
) -> Result<(), KupferdrahtError> {
    debug_assert!(
        !config.api_base_url.is_empty(),
        "server_config.api_base_url must be non-empty"
    );
    let conn = store.lock()?;
    conn.execute(
        "INSERT INTO server_config (id, api_base_url, websocket_url, title)
         VALUES (1, ?1, ?2, ?3)
         ON CONFLICT(id) DO UPDATE SET
             api_base_url  = excluded.api_base_url,
             websocket_url = excluded.websocket_url,
             title         = excluded.title",
        params![config.api_base_url, config.websocket_url, config.title],
    )
    .map_err(|e| KupferdrahtError::Storage(format!("save_server_config: {e}")))?;
    Ok(())
}

pub(super) fn load_server_config(
    store: &SqliteStore,
) -> Result<Option<ServerConfigRecord>, KupferdrahtError> {
    let conn = store.lock()?;
    conn.query_row(
        "SELECT api_base_url, websocket_url, title FROM server_config WHERE id = 1",
        [],
        |row| {
            Ok(ServerConfigRecord {
                api_base_url: row.get(0)?,
                websocket_url: row.get(1)?,
                title: row.get(2)?,
            })
        },
    )
    .optional()
    .map_err(|e| KupferdrahtError::Storage(format!("load_server_config: {e}")))
}
