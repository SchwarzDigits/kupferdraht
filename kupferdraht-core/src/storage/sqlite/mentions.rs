// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `mentions` table CRUD. One row per `(message_id, user_id, span)`
//! tuple. No encrypted columns — the user-id + span tuple is
//! considered metadata, the body lives in `timeline_events`.

use rusqlite::params;

use crate::error::KupferdrahtError;
use crate::storage::records::MentionRecord;

use super::SqliteStore;

pub(super) fn replace_mentions(
    store: &SqliteStore,
    room_id: &str,
    message_id: &str,
    mentions: &[MentionRecord],
) -> Result<(), KupferdrahtError> {
    let mut conn = store.lock()?;
    let tx = super::write_tx::begin(&mut conn)
        .map_err(|e| KupferdrahtError::Storage(format!("replace_mentions begin tx: {e}")))?;
    replace_mentions_on(&tx, room_id, message_id, mentions)?;
    tx.commit()
        .map_err(|e| KupferdrahtError::Storage(format!("replace_mentions commit: {e}")))?;
    Ok(())
}

/// [`replace_mentions`] inside a caller's transaction (Step 105).
pub(super) fn replace_mentions_on(
    tx: &rusqlite::Connection,
    room_id: &str,
    message_id: &str,
    mentions: &[MentionRecord],
) -> Result<(), KupferdrahtError> {
    tx.execute(
        "DELETE FROM mentions WHERE message_id = ?1 AND room_id = ?2",
        params![message_id, room_id],
    )
    .map_err(|e| KupferdrahtError::Storage(format!("replace_mentions clear: {e}")))?;
    for m in mentions {
        tx.execute(
            "INSERT INTO mentions (
                message_id, room_id, user_id, start, length
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![message_id, room_id, m.user_id, m.start, m.length],
        )
        .map_err(|e| {
            KupferdrahtError::Storage(format!(
                "replace_mentions insert {} @[{}+{}]: {e}",
                m.user_id, m.start, m.length
            ))
        })?;
    }
    Ok(())
}

pub(super) fn load_mentions_for_room(
    store: &SqliteStore,
    room_id: &str,
) -> Result<Vec<MentionRecord>, KupferdrahtError> {
    let conn = store.lock()?;
    let mut stmt = conn
        .prepare(
            "SELECT message_id, room_id, user_id, start, length
             FROM mentions
             WHERE room_id = ?1
             ORDER BY start ASC",
        )
        .map_err(|e| KupferdrahtError::Storage(format!("load_mentions_for_room prepare: {e}")))?;
    let rows: Vec<MentionRecord> = stmt
        .query_map(params![room_id], |row| {
            Ok(MentionRecord {
                message_id: row.get(0)?,
                room_id: row.get(1)?,
                user_id: row.get(2)?,
                start: row.get(3)?,
                length: row.get(4)?,
            })
        })
        .map_err(|e| KupferdrahtError::Storage(format!("load_mentions_for_room query: {e}")))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| KupferdrahtError::Storage(format!("load_mentions_for_room row: {e}")))?;
    Ok(rows)
}
