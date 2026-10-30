// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `room_read_markers` CRUD + the unread-count queries (Step 67).
//! The marker only ever moves forward — marking an older message as
//! read after a newer one must not resurrect read messages as unread.

use std::collections::HashMap;

use rusqlite::{OptionalExtension, params};

use crate::error::KupferdrahtError;

use super::SqliteStore;

/// `i64::MIN` as the "never marked" sentinel inside the queries — a
/// missing marker row means everything inbound counts (Step-67
/// decision: the honest default).
const NO_MARKER: i64 = i64::MIN;

pub(super) fn set_read_marker(
    store: &SqliteStore,
    room_id: &str,
    at_ms: i64,
) -> Result<(), KupferdrahtError> {
    let conn = store.lock()?;
    conn.execute(
        "INSERT INTO room_read_markers (room_id, last_read_at_ms)
         VALUES (?1, ?2)
         ON CONFLICT (room_id) DO UPDATE SET
            last_read_at_ms = MAX(last_read_at_ms, excluded.last_read_at_ms)",
        params![room_id, at_ms],
    )
    .map_err(|e| KupferdrahtError::Storage(format!("set_read_marker: {e}")))?;
    Ok(())
}

pub(super) fn read_marker(
    store: &SqliteStore,
    room_id: &str,
) -> Result<Option<i64>, KupferdrahtError> {
    let conn = store.lock()?;
    conn.query_row(
        "SELECT last_read_at_ms FROM room_read_markers WHERE room_id = ?1",
        params![room_id],
        |row| row.get(0),
    )
    .optional()
    .map_err(|e| KupferdrahtError::Storage(format!("read_marker: {e}")))
}

pub(super) fn unread_count_for_room(
    store: &SqliteStore,
    room_id: &str,
    self_user_id: &str,
) -> Result<u32, KupferdrahtError> {
    let conn = store.lock()?;
    conn.query_row(
        "SELECT COUNT(*) FROM timeline_events e
         WHERE e.room_id = ?1
           AND e.sender_user_id != ?2
           AND e.redacted_at_ms IS NULL
           AND e.sent_at_ms > COALESCE(
               (SELECT m.last_read_at_ms FROM room_read_markers m
                 WHERE m.room_id = ?1), ?3)",
        params![room_id, self_user_id, NO_MARKER],
        |row| row.get(0),
    )
    .map_err(|e| KupferdrahtError::Storage(format!("unread_count_for_room: {e}")))
}

pub(super) fn unread_counts(
    store: &SqliteStore,
    self_user_id: &str,
) -> Result<HashMap<String, u32>, KupferdrahtError> {
    let conn = store.lock()?;
    let mut stmt = conn
        .prepare(
            "SELECT e.room_id, COUNT(*) FROM timeline_events e
             LEFT JOIN room_read_markers m ON m.room_id = e.room_id
             WHERE e.sender_user_id != ?1
               AND e.redacted_at_ms IS NULL
               AND e.sent_at_ms > COALESCE(m.last_read_at_ms, ?2)
             GROUP BY e.room_id",
        )
        .map_err(|e| KupferdrahtError::Storage(format!("unread_counts prepare: {e}")))?;
    let rows = stmt
        .query_map(params![self_user_id, NO_MARKER], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, u32>(1)?))
        })
        .map_err(|e| KupferdrahtError::Storage(format!("unread_counts query: {e}")))?;
    let mut counts = HashMap::new();
    for row in rows {
        let (room_id, count) =
            row.map_err(|e| KupferdrahtError::Storage(format!("unread_counts row: {e}")))?;
        counts.insert(room_id, count);
    }
    Ok(counts)
}
