// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `reactions` table CRUD. Wire's `Reaction` payload is the
//! sender's full set; Kupferdraht decomposes it into one row per emoji
//! so per-message aggregations are cheap. Step 6-II-e.

use rusqlite::params;

use crate::error::KupferdrahtError;
use crate::storage::records::ReactionRecord;

use super::SqliteStore;

pub(super) fn replace_user_reactions(
    store: &SqliteStore,
    room_id: &str,
    canonical_message_id: &str,
    sender_user_id: &str,
    emojis: &[String],
    sent_at_ms: i64,
) -> Result<(), KupferdrahtError> {
    let mut conn = store.lock()?;
    let tx = super::write_tx::begin(&mut conn)
        .map_err(|e| KupferdrahtError::Storage(format!("replace_user_reactions begin tx: {e}")))?;
    tx.execute(
        "DELETE FROM reactions
          WHERE message_id = ?1 AND sender_user_id = ?2",
        params![canonical_message_id, sender_user_id],
    )
    .map_err(|e| KupferdrahtError::Storage(format!("replace_user_reactions clear: {e}")))?;
    for emoji in emojis {
        tx.execute(
            "INSERT INTO reactions (
                message_id, room_id, sender_user_id, emoji, sent_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                canonical_message_id,
                room_id,
                sender_user_id,
                emoji,
                sent_at_ms,
            ],
        )
        .map_err(|e| {
            KupferdrahtError::Storage(format!("replace_user_reactions insert {emoji}: {e}"))
        })?;
    }
    tx.commit()
        .map_err(|e| KupferdrahtError::Storage(format!("replace_user_reactions commit: {e}")))?;
    Ok(())
}

pub(super) fn load_reactions_for_room(
    store: &SqliteStore,
    room_id: &str,
) -> Result<Vec<ReactionRecord>, KupferdrahtError> {
    let conn = store.lock()?;
    let mut stmt = conn
        .prepare(
            "SELECT message_id, room_id, sender_user_id, emoji, sent_at_ms
             FROM reactions
             WHERE room_id = ?1
             ORDER BY sent_at_ms ASC",
        )
        .map_err(|e| KupferdrahtError::Storage(format!("load_reactions_for_room prepare: {e}")))?;
    let rows: Vec<ReactionRecord> = stmt
        .query_map(params![room_id], |row| {
            Ok(ReactionRecord {
                message_id: row.get(0)?,
                room_id: row.get(1)?,
                sender_user_id: row.get(2)?,
                emoji: row.get(3)?,
                sent_at_ms: row.get(4)?,
            })
        })
        .map_err(|e| KupferdrahtError::Storage(format!("load_reactions_for_room query: {e}")))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| KupferdrahtError::Storage(format!("load_reactions_for_room row: {e}")))?;
    Ok(rows)
}
