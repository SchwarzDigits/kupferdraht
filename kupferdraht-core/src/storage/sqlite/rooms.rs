// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `rooms` + `room_members` tables CRUD. Whole-DB encryption protects
//! the `name` and per-member `display_name` columns at rest, so they
//! are stored and read verbatim; the rest (`room_id`, `group_id`,
//! `room_type`, `epoch`, `counterpart_user_id`, `is_self`, `handle`)
//! is likewise plain.

use rusqlite::{Connection, params};

use crate::error::KupferdrahtError;
use crate::storage::records::{RoomMemberRecord, RoomRecord};

use super::{SqliteStore, utf8_column};

pub(super) fn replace_rooms(
    store: &SqliteStore,
    rooms: &[(RoomRecord, Vec<RoomMemberRecord>)],
) -> Result<(), KupferdrahtError> {
    let mut conn = store.lock()?;
    let tx = super::write_tx::begin(&mut conn)
        .map_err(|e| KupferdrahtError::Storage(format!("replace_rooms begin tx: {e}")))?;
    tx.execute("DELETE FROM room_members", [])
        .map_err(|e| KupferdrahtError::Storage(format!("replace_rooms clear members: {e}")))?;
    tx.execute("DELETE FROM rooms", [])
        .map_err(|e| KupferdrahtError::Storage(format!("replace_rooms clear rooms: {e}")))?;
    for (room, members) in rooms {
        insert_room_row(&tx, room)?;
        for m in members {
            insert_room_member_row(&tx, m)?;
        }
    }
    tx.commit()
        .map_err(|e| KupferdrahtError::Storage(format!("replace_rooms commit: {e}")))?;
    Ok(())
}

pub(super) fn replace_room_members(
    store: &SqliteStore,
    room_id: &str,
    members: &[RoomMemberRecord],
) -> Result<(), KupferdrahtError> {
    let mut conn = store.lock()?;
    let tx = super::write_tx::begin(&mut conn)
        .map_err(|e| KupferdrahtError::Storage(format!("replace_room_members begin tx: {e}")))?;
    tx.execute(
        "DELETE FROM room_members WHERE room_id = ?1",
        params![room_id],
    )
    .map_err(|e| KupferdrahtError::Storage(format!("replace_room_members clear: {e}")))?;
    for m in members {
        insert_room_member_row(&tx, m)?;
    }
    tx.commit()
        .map_err(|e| KupferdrahtError::Storage(format!("replace_room_members commit: {e}")))?;
    Ok(())
}

pub(super) fn update_room_epoch(
    store: &SqliteStore,
    room_id: &str,
    new_epoch: i64,
) -> Result<(), KupferdrahtError> {
    let conn = store.lock()?;
    conn.execute(
        "UPDATE rooms SET epoch = ?1 WHERE room_id = ?2",
        params![new_epoch, room_id],
    )
    .map_err(|e| {
        KupferdrahtError::Storage(format!("update_room_epoch {room_id} → {new_epoch}: {e}"))
    })?;
    Ok(())
}

pub(super) fn load_rooms(
    store: &SqliteStore,
) -> Result<Vec<(RoomRecord, Vec<RoomMemberRecord>)>, KupferdrahtError> {
    // Pull raw rooms + their member rows under one connection-lock
    // pass, then drop the lock before any UTF-8 decode so errors
    // surface outside the `rusqlite` row closure.
    let (rooms, member_rows) = read_raw_rooms_and_members(store)?;
    let mut out = Vec::with_capacity(rooms.len());
    for (raw, raw_members) in rooms.into_iter().zip(member_rows) {
        let room = assemble_room(raw)?;
        let members = assemble_room_members(&room.room_id, raw_members)?;
        out.push((room, members));
    }
    Ok(out)
}

fn assemble_room(raw: RawRoomRow) -> Result<RoomRecord, KupferdrahtError> {
    Ok(RoomRecord {
        room_id: raw.room_id,
        group_id: raw.group_id,
        name: utf8_column(raw.name, "rooms.name")?,
        room_type: raw.room_type,
        epoch: raw.epoch,
        counterpart_user_id: raw.counterpart_user_id,
        last_synced_at_ms: raw.last_synced_at_ms,
    })
}

fn read_raw_rooms_and_members(
    store: &SqliteStore,
) -> Result<(Vec<RawRoomRow>, Vec<Vec<RawRoomMemberRow>>), KupferdrahtError> {
    let conn = store.lock()?;
    let mut stmt = conn
        .prepare(
            "SELECT room_id, group_id, name,
                    room_type, epoch, counterpart_user_id, last_synced_at_ms
             FROM rooms
             ORDER BY last_synced_at_ms DESC, room_id ASC",
        )
        .map_err(|e| KupferdrahtError::Storage(format!("load_rooms prepare: {e}")))?;
    let rows: Vec<RawRoomRow> = stmt
        .query_map([], read_raw_room_row)
        .map_err(|e| KupferdrahtError::Storage(format!("load_rooms query: {e}")))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| KupferdrahtError::Storage(format!("load_rooms row: {e}")))?;
    drop(stmt);
    let mut member_rows: Vec<Vec<RawRoomMemberRow>> = Vec::with_capacity(rows.len());
    for raw in &rows {
        member_rows.push(read_raw_room_members(&conn, &raw.room_id)?);
    }
    Ok((rows, member_rows))
}

fn assemble_room_members(
    room_id: &str,
    raw_members: Vec<RawRoomMemberRow>,
) -> Result<Vec<RoomMemberRecord>, KupferdrahtError> {
    let mut out = Vec::with_capacity(raw_members.len());
    for r in raw_members {
        let display_name = r
            .display_name
            .map(|b| utf8_column(b, "room_members.display_name"))
            .transpose()?;
        out.push(RoomMemberRecord {
            room_id: room_id.to_string(),
            user_id: r.user_id,
            is_self: r.is_self,
            display_name,
            handle: r.handle,
            role: r.role,
        });
    }
    Ok(out)
}

fn insert_room_row(tx: &Connection, room: &RoomRecord) -> Result<(), KupferdrahtError> {
    tx.execute(
        "INSERT INTO rooms (
            room_id, group_id, name, room_type, epoch,
            counterpart_user_id, last_synced_at_ms
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            room.room_id,
            room.group_id,
            room.name.as_bytes(),
            room.room_type,
            room.epoch,
            room.counterpart_user_id,
            room.last_synced_at_ms,
        ],
    )
    .map_err(|e| {
        KupferdrahtError::Storage(format!("replace_rooms insert room {}: {e}", room.room_id))
    })?;
    Ok(())
}

fn insert_room_member_row(tx: &Connection, m: &RoomMemberRecord) -> Result<(), KupferdrahtError> {
    tx.execute(
        "INSERT INTO room_members (
            room_id, user_id, is_self, display_name, handle, role
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            m.room_id,
            m.user_id,
            i64::from(m.is_self),
            m.display_name.as_deref().map(str::as_bytes),
            m.handle,
            m.role,
        ],
    )
    .map_err(|e| {
        KupferdrahtError::Storage(format!(
            "replace_rooms insert member {}@{}: {e}",
            m.user_id, m.room_id
        ))
    })?;
    Ok(())
}

struct RawRoomRow {
    room_id: String,
    group_id: Option<String>,
    name: Vec<u8>,
    room_type: String,
    epoch: i64,
    counterpart_user_id: Option<String>,
    last_synced_at_ms: i64,
}

fn read_raw_room_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawRoomRow> {
    Ok(RawRoomRow {
        room_id: row.get(0)?,
        group_id: row.get(1)?,
        name: row.get(2)?,
        room_type: row.get(3)?,
        epoch: row.get(4)?,
        counterpart_user_id: row.get(5)?,
        last_synced_at_ms: row.get(6)?,
    })
}

fn read_raw_room_members(
    conn: &Connection,
    room_id: &str,
) -> Result<Vec<RawRoomMemberRow>, KupferdrahtError> {
    let mut stmt = conn
        .prepare(
            "SELECT user_id, is_self, display_name, handle, role
             FROM room_members WHERE room_id = ?1
             ORDER BY is_self DESC, user_id ASC",
        )
        .map_err(|e| KupferdrahtError::Storage(format!("load_room_members prepare: {e}")))?;
    let members: Vec<RawRoomMemberRow> = stmt
        .query_map(params![room_id], |row| {
            let user_id: String = row.get(0)?;
            let is_self: i64 = row.get(1)?;
            let display_name: Option<Vec<u8>> = row.get(2)?;
            let handle: Option<String> = row.get(3)?;
            let role: String = row.get(4)?;
            Ok(RawRoomMemberRow {
                user_id,
                is_self: is_self != 0,
                display_name,
                handle,
                role,
            })
        })
        .map_err(|e| KupferdrahtError::Storage(format!("load_room_members query: {e}")))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| KupferdrahtError::Storage(format!("load_room_members row: {e}")))?;
    Ok(members)
}

struct RawRoomMemberRow {
    user_id: String,
    is_self: bool,
    display_name: Option<Vec<u8>>,
    handle: Option<String>,
    role: String,
}

#[cfg(test)]
mod tests {
    use super::super::{INVALID_UTF8, fresh_store};
    use crate::error::KupferdrahtError;
    use crate::storage::Store as _;
    use crate::storage::records::{RoomMemberRecord, RoomRecord};
    use rusqlite::params;

    fn sample_room(
        room_id: &str,
        name: &str,
        members: &[(&str, bool)],
    ) -> (RoomRecord, Vec<RoomMemberRecord>) {
        let room = RoomRecord {
            room_id: room_id.to_string(),
            group_id: Some("Z3JwLWFh".to_string()),
            name: name.to_string(),
            room_type: "group".to_string(),
            epoch: 1,
            counterpart_user_id: None,
            last_synced_at_ms: 1_700_000_000_000,
        };
        let member_records = members
            .iter()
            .map(|(uid, is_self)| RoomMemberRecord {
                room_id: room_id.to_string(),
                user_id: (*uid).to_string(),
                is_self: *is_self,
                display_name: Some(format!("Name for {uid}")),
                handle: Some(format!("h_{uid}")),
                role: "member".to_string(),
            })
            .collect();
        (room, member_records)
    }

    #[tokio::test]
    async fn replace_rooms_round_trips_rooms_and_members() {
        let (_dir, store) = fresh_store();
        let snapshot = vec![
            sample_room(
                "room-a@example.test",
                "Engineering",
                &[("me@example.test", true), ("bob@example.test", false)],
            ),
            sample_room(
                "room-b@example.test",
                "Design",
                &[("me@example.test", true)],
            ),
        ];

        store.replace_rooms(&snapshot).await.expect("replace");
        let loaded = store.load_rooms().await.expect("load");

        assert_eq!(loaded.len(), 2);
        let room_a = loaded
            .iter()
            .find(|(r, _)| r.room_id == "room-a@example.test")
            .expect("room-a present");
        assert_eq!(room_a.0.name, "Engineering");
        assert_eq!(room_a.1.len(), 2);
        // Members order is is_self DESC then user_id ASC → self first.
        assert!(room_a.1[0].is_self);
        assert_eq!(room_a.1[0].user_id, "me@example.test");
        assert_eq!(
            room_a.1[0].display_name.as_deref(),
            Some("Name for me@example.test")
        );
        assert!(!room_a.1[1].is_self);
        assert_eq!(room_a.1[1].user_id, "bob@example.test");
    }

    #[tokio::test]
    async fn replace_rooms_is_full_replacement() {
        let (_dir, store) = fresh_store();
        store
            .replace_rooms(&[sample_room(
                "old@example.test",
                "Old",
                &[("me@example.test", true)],
            )])
            .await
            .expect("first replace");
        assert_eq!(store.load_rooms().await.expect("load #1").len(), 1);

        // Second replace drops the old row and inserts a fresh one.
        store
            .replace_rooms(&[sample_room(
                "new@example.test",
                "New",
                &[("me@example.test", true)],
            )])
            .await
            .expect("second replace");
        let loaded = store.load_rooms().await.expect("load #2");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].0.room_id, "new@example.test");
    }

    #[tokio::test]
    async fn replace_rooms_with_empty_input_clears_tables() {
        let (_dir, store) = fresh_store();
        store
            .replace_rooms(&[sample_room(
                "room@example.test",
                "X",
                &[("me@example.test", true)],
            )])
            .await
            .expect("initial");
        store.replace_rooms(&[]).await.expect("clear");
        assert!(store.load_rooms().await.expect("load").is_empty());
    }

    #[tokio::test]
    async fn load_rooms_surfaces_non_utf8_name_as_encoding_error() {
        let (_dir, store) = fresh_store();
        {
            let conn = store.lock().expect("lock");
            conn.execute(
                "INSERT INTO rooms (
                    room_id, group_id, name, room_type, epoch,
                    counterpart_user_id, last_synced_at_ms
                 ) VALUES (?1, NULL, ?2, 'group', 1, NULL, 1)",
                params!["corrupt@example.test", INVALID_UTF8],
            )
            .expect("raw insert");
        }
        let err = store
            .load_rooms()
            .await
            .expect_err("corrupted name must surface as Encoding");
        assert!(
            matches!(err, KupferdrahtError::Encoding(_)),
            "expected Encoding, got {err:?}"
        );
    }
}
