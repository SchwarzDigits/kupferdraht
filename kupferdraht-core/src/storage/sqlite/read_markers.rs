// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `room_read_markers` CRUD + the unread-count queries (Step 67) and the
//! hand-set unread flag beside the marker (Step 138, V31). The marker
//! only ever moves forward — marking an older message as read after a
//! newer one must not resurrect read messages as unread — and moving it
//! clears the flag: a read is the user's "this room is read". A row may
//! hold the flag without a position (a room flagged before it was ever
//! read); `last_read_at_ms` is NULL then and counts as "no marker".

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
    // `MAX` with a NULL argument is NULL in SQLite: a row flagged before
    // its first read takes the new position outright.
    conn.execute(
        "INSERT INTO room_read_markers (room_id, last_read_at_ms, marked_unread)
         VALUES (?1, ?2, 0)
         ON CONFLICT (room_id) DO UPDATE SET
            last_read_at_ms = MAX(COALESCE(last_read_at_ms, excluded.last_read_at_ms),
                                  excluded.last_read_at_ms),
            marked_unread = 0",
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
        |row| row.get::<_, Option<i64>>(0),
    )
    .optional()
    .map(Option::flatten)
    .map_err(|e| KupferdrahtError::Storage(format!("read_marker: {e}")))
}

/// Set or clear the hand-set flag (Step 138). Setting it on a room never
/// read inserts a row without a position; clearing it touches no row
/// that does not exist.
pub(super) fn set_unread_flag(
    store: &SqliteStore,
    room_id: &str,
    flag: bool,
) -> Result<(), KupferdrahtError> {
    let conn = store.lock()?;
    let sql = if flag {
        "INSERT INTO room_read_markers (room_id, last_read_at_ms, marked_unread)
         VALUES (?1, NULL, 1)
         ON CONFLICT (room_id) DO UPDATE SET marked_unread = 1"
    } else {
        "UPDATE room_read_markers SET marked_unread = 0 WHERE room_id = ?1"
    };
    conn.execute(sql, params![room_id])
        .map_err(|e| KupferdrahtError::Storage(format!("set_unread_flag: {e}")))?;
    Ok(())
}

pub(super) fn is_marked_unread(
    store: &SqliteStore,
    room_id: &str,
) -> Result<bool, KupferdrahtError> {
    let conn = store.lock()?;
    conn.query_row(
        "SELECT marked_unread FROM room_read_markers WHERE room_id = ?1",
        params![room_id],
        |row| row.get::<_, i64>(0),
    )
    .optional()
    .map(|flag| flag.is_some_and(|f| f != 0))
    .map_err(|e| KupferdrahtError::Storage(format!("is_marked_unread: {e}")))
}

/// Every marker row in one query (Step 138) — the room list's companion
/// to [`unread_counts`].
pub(super) fn read_states(
    store: &SqliteStore,
) -> Result<HashMap<String, crate::storage::ReadStateRecord>, KupferdrahtError> {
    let conn = store.lock()?;
    let mut stmt = conn
        .prepare("SELECT room_id, last_read_at_ms, marked_unread FROM room_read_markers")
        .map_err(|e| KupferdrahtError::Storage(format!("read_states prepare: {e}")))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                crate::storage::ReadStateRecord {
                    last_read_at_ms: row.get::<_, Option<i64>>(1)?,
                    marked_unread: row.get::<_, i64>(2)? != 0,
                },
            ))
        })
        .map_err(|e| KupferdrahtError::Storage(format!("read_states query: {e}")))?;
    let mut states = HashMap::new();
    for row in rows {
        let (room_id, state) =
            row.map_err(|e| KupferdrahtError::Storage(format!("read_states row: {e}")))?;
        states.insert(room_id, state);
    }
    Ok(states)
}

/// Move the marker to `at_ms` when that is further on, clearing the flag
/// with the move; a position at or behind the marker leaves row and flag
/// as they are. Returns whether the marker moved (Step 129a).
pub(super) fn advance_read_marker(
    store: &SqliteStore,
    room_id: &str,
    at_ms: i64,
) -> Result<bool, KupferdrahtError> {
    let conn = store.lock()?;
    // The upsert's WHERE keeps an older or equal position and, with it, the
    // flag: nothing changes, `changes()` stays 0.
    let changed = conn
        .execute(
            "INSERT INTO room_read_markers (room_id, last_read_at_ms, marked_unread)
             VALUES (?1, ?2, 0)
             ON CONFLICT (room_id) DO UPDATE SET
                last_read_at_ms = excluded.last_read_at_ms,
                marked_unread = 0
             WHERE last_read_at_ms IS NULL OR last_read_at_ms < excluded.last_read_at_ms",
            params![room_id, at_ms],
        )
        .map_err(|e| KupferdrahtError::Storage(format!("advance_read_marker: {e}")))?;
    debug_assert!(changed <= 1, "room_id is the primary key");
    Ok(changed == 1)
}

/// The latest `arrived_at_ms` among the room's rows whose `sent_at_ms` is
/// at or before `sent_at_ms` — where a read position another device
/// reported in the backend's time lands on this device (Step 129a).
pub(super) fn latest_arrival_sent_at_or_before(
    store: &SqliteStore,
    room_id: &str,
    sent_at_ms: i64,
) -> Result<Option<i64>, KupferdrahtError> {
    let conn = store.lock()?;
    conn.query_row(
        "SELECT MAX(arrived_at_ms) FROM timeline_events
         WHERE room_id = ?1 AND sent_at_ms <= ?2",
        params![room_id, sent_at_ms],
        |row| row.get::<_, Option<i64>>(0),
    )
    .map_err(|e| KupferdrahtError::Storage(format!("latest_arrival_sent_at_or_before: {e}")))
}

pub(super) fn unread_count_for_room(
    store: &SqliteStore,
    room_id: &str,
    own_user_id: &str,
) -> Result<u32, KupferdrahtError> {
    let conn = store.lock()?;
    conn.query_row(
        "SELECT COUNT(*) FROM timeline_events e
         WHERE e.room_id = ?1
           AND e.sender_user_id != ?2
           AND e.redacted_at_ms IS NULL
           AND e.kind NOT IN ('membership_change', 'room_name_change', 'room_created')
           AND e.arrived_at_ms > COALESCE(
               (SELECT m.last_read_at_ms FROM room_read_markers m
                 WHERE m.room_id = ?1), ?3)",
        params![room_id, own_user_id, NO_MARKER],
        |row| row.get(0),
    )
    .map_err(|e| KupferdrahtError::Storage(format!("unread_count_for_room: {e}")))
}

pub(super) fn unread_counts(
    store: &SqliteStore,
    own_user_id: &str,
) -> Result<HashMap<String, u32>, KupferdrahtError> {
    let conn = store.lock()?;
    let mut stmt = conn
        .prepare(
            "SELECT e.room_id, COUNT(*) FROM timeline_events e
             LEFT JOIN room_read_markers m ON m.room_id = e.room_id
             WHERE e.sender_user_id != ?1
               AND e.redacted_at_ms IS NULL
           AND e.kind NOT IN ('membership_change', 'room_name_change', 'room_created')
               AND e.arrived_at_ms > COALESCE(m.last_read_at_ms, ?2)
             GROUP BY e.room_id",
        )
        .map_err(|e| KupferdrahtError::Storage(format!("unread_counts prepare: {e}")))?;
    let rows = stmt
        .query_map(params![own_user_id, NO_MARKER], |row| {
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

#[cfg(test)]
mod tests {
    use super::super::fresh_store;
    use super::*;

    /// The V31 migration (Step 138) lifts `NOT NULL` from `last_read_at_ms`
    /// — a room flagged before it was ever read has no position — and adds
    /// `marked_unread`; the rows a V30 database holds survive the rebuild
    /// with the flag clear.
    #[test]
    fn v31_keeps_the_markers_of_a_v30_database_and_adds_the_flag() {
        let (_dir, store) = fresh_store();
        let migration = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/storage/migrations/V31__step_138_unread_flag.sql"
        ))
        .expect("the V31 migration ships");
        {
            let conn = store.lock().expect("lock");
            conn.execute_batch(
                "DROP TABLE room_read_markers;
                 CREATE TABLE room_read_markers (
                     room_id          TEXT    PRIMARY KEY NOT NULL,
                     last_read_at_ms  INTEGER NOT NULL
                 );
                 INSERT INTO room_read_markers (room_id, last_read_at_ms)
                 VALUES ('read@d', 1000);",
            )
            .expect("a V30-shaped table");
            conn.execute_batch(&migration).expect("V31 applies");
        }

        assert_eq!(read_marker(&store, "read@d").expect("marker"), Some(1000));
        assert!(!is_marked_unread(&store, "read@d").expect("flag"));
        set_unread_flag(&store, "never@d", true).expect("a room never read takes the flag");
        assert_eq!(read_marker(&store, "never@d").expect("marker"), None);
        assert!(is_marked_unread(&store, "never@d").expect("flag"));
    }

    /// The forward-only upsert of the marker clears the flag: a read is
    /// the user's "this room is read", whatever the flag said.
    #[test]
    fn a_read_marker_clears_the_flag_and_stays_forward_only() {
        let (_dir, store) = fresh_store();
        set_unread_flag(&store, "r@d", true).expect("flag");
        set_read_marker(&store, "r@d", 2_000).expect("marker");
        assert!(!is_marked_unread(&store, "r@d").expect("flag"));
        set_read_marker(&store, "r@d", 1_000).expect("older marker");
        assert_eq!(read_marker(&store, "r@d").expect("marker"), Some(2_000));
        let states = read_states(&store).expect("read states");
        assert_eq!(
            states.get("r@d"),
            Some(&crate::storage::ReadStateRecord {
                last_read_at_ms: Some(2_000),
                marked_unread: false,
            })
        );
    }
}

#[cfg(test)]
mod read_position_tests {
    use super::super::fresh_store;
    use super::*;
    use crate::storage::{Store as _, TimelineEventRecord};

    fn row(
        room_id: &str,
        message_id: &str,
        sent_at_ms: i64,
        arrived_at_ms: i64,
    ) -> TimelineEventRecord {
        TimelineEventRecord {
            message_id: message_id.into(),
            room_id: room_id.into(),
            sender_user_id: "bob@d".into(),
            kind: "text".into(),
            body: message_id.into(),
            sent_at_ms,
            arrived_at_ms,
            delivery_status: "received".into(),
            ..TimelineEventRecord::default()
        }
    }

    /// A reported position further on moves the marker and clears the flag;
    /// one at or behind it changes nothing, the flag included.
    #[test]
    fn advance_moves_forward_only_and_clears_the_flag_only_when_it_moves() {
        let (_dir, store) = fresh_store();
        assert!(advance_read_marker(&store, "r@d", 2_000).expect("first position"));
        assert_eq!(read_marker(&store, "r@d").expect("marker"), Some(2_000));

        set_unread_flag(&store, "r@d", true).expect("flag");
        assert!(!advance_read_marker(&store, "r@d", 1_000).expect("older position"));
        assert_eq!(read_marker(&store, "r@d").expect("marker"), Some(2_000));
        assert!(
            is_marked_unread(&store, "r@d").expect("flag"),
            "an older position leaves the flag"
        );
        assert!(!advance_read_marker(&store, "r@d", 2_000).expect("same position"));
        assert!(
            is_marked_unread(&store, "r@d").expect("flag"),
            "the same position leaves the flag"
        );

        assert!(advance_read_marker(&store, "r@d", 3_000).expect("newer position"));
        assert_eq!(read_marker(&store, "r@d").expect("marker"), Some(3_000));
        assert!(
            !is_marked_unread(&store, "r@d").expect("flag"),
            "a move clears the flag"
        );
    }

    /// A room flagged before it was ever read has a row without a position;
    /// the first reported position moves into it.
    #[test]
    fn advance_fills_a_flagged_row_without_a_position() {
        let (_dir, store) = fresh_store();
        set_unread_flag(&store, "r@d", true).expect("flag");
        assert!(advance_read_marker(&store, "r@d", 500).expect("position"));
        assert_eq!(read_marker(&store, "r@d").expect("marker"), Some(500));
        assert!(!is_marked_unread(&store, "r@d").expect("flag"));
    }

    /// A backend time lands on the latest arrival among the rows sent by
    /// then: a message sent earlier but arriving later (Step 151) counts.
    #[tokio::test]
    async fn the_latest_arrival_sent_by_then_is_the_position() {
        let (_dir, store) = fresh_store();
        for (id, sent, arrived) in [
            ("m1", 1_000, 1_000),
            ("m2", 2_000, 2_000),
            ("late", 1_500, 2_500),
            ("m3", 3_000, 3_000),
        ] {
            store
                .insert_timeline_event(&row("r@d", id, sent, arrived))
                .await
                .expect("seed");
        }
        store
            .insert_timeline_event(&row("other@d", "o1", 1_000, 1_000))
            .await
            .expect("seed other room");

        assert_eq!(
            latest_arrival_sent_at_or_before(&store, "r@d", 2_000).expect("position"),
            Some(2_500),
            "the late arrival of a message sent by then is the furthest position"
        );
        assert_eq!(
            latest_arrival_sent_at_or_before(&store, "r@d", 1_000).expect("position"),
            Some(1_000)
        );
        assert_eq!(
            latest_arrival_sent_at_or_before(&store, "r@d", 999).expect("position"),
            None,
            "nothing sent by then anchors no position"
        );
        assert_eq!(
            latest_arrival_sent_at_or_before(&store, "ghost@d", 5_000).expect("position"),
            None,
            "an unknown room has no position"
        );
    }
}
