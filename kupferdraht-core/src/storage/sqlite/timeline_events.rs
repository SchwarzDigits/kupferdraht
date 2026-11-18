// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `timeline_events` table CRUD: insert + load + edit + delete +
//! the canonical-id chain lookup. Whole-DB encryption protects the
//! `body` column at rest, so it is stored and read verbatim; `kind`,
//! `delivery_status`, and the timestamps are plain.

use rusqlite::{OptionalExtension, params};

use crate::error::KupferdrahtError;
use crate::storage::records::{ExpiredMessageRecord, TimelineEventRecord};

use super::{SqliteStore, utf8_column};

pub(super) fn insert_timeline_event(
    store: &SqliteStore,
    event: &TimelineEventRecord,
) -> Result<(), KupferdrahtError> {
    let conn = store.lock()?;
    insert_timeline_event_on(&conn, event).map_err(|e| {
        KupferdrahtError::Storage(format!(
            "insert_timeline_event {}@{}: {e}",
            event.message_id, event.room_id
        ))
    })
}

/// Insert `event` unless its `(message_id, room_id)` row exists —
/// `Ok(false)` then, nothing written (Step 105: the combined writes).
pub(super) fn try_insert_timeline_event_on(
    conn: &rusqlite::Connection,
    event: &TimelineEventRecord,
) -> Result<bool, KupferdrahtError> {
    match insert_timeline_event_on(conn, event) {
        Ok(()) => Ok(true),
        Err(rusqlite::Error::SqliteFailure(failure, _))
            if failure.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_PRIMARYKEY =>
        {
            Ok(false)
        }
        Err(e) => Err(KupferdrahtError::Storage(format!(
            "insert_timeline_event {}@{}: {e}",
            event.message_id, event.room_id
        ))),
    }
}

fn insert_timeline_event_on(
    conn: &rusqlite::Connection,
    event: &TimelineEventRecord,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO timeline_events (
            message_id, room_id, sender_user_id, sender_device_id,
            kind, body,
            sent_at_ms, delivery_status, reply_to_id, expires_at_ms,
            ephemeral_ttl_ms, arrived_at_ms
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            event.message_id,
            event.room_id,
            event.sender_user_id,
            event.sender_device_id,
            event.kind,
            event.body.as_bytes(),
            event.sent_at_ms,
            event.delivery_status,
            event.reply_to_id,
            event.expires_at_ms,
            event.ttl_ms,
            event.arrived_at_ms,
        ],
    )?;
    Ok(())
}

pub(super) fn load_timeline_events(
    store: &SqliteStore,
    room_id: &str,
) -> Result<Vec<TimelineEventRecord>, KupferdrahtError> {
    let raw = read_raw_timeline_rows(store, room_id)?;
    let mut out = Vec::with_capacity(raw.len());
    for row in raw {
        out.push(assemble_timeline_event(row)?);
    }
    Ok(out)
}

pub(super) fn load_timeline_event(
    store: &SqliteStore,
    target_id: &str,
) -> Result<Option<TimelineEventRecord>, KupferdrahtError> {
    let conn = store.lock()?;
    let mut stmt = conn
        .prepare(
            "SELECT message_id, room_id, sender_user_id, sender_device_id,
                    kind, body,
                    sent_at_ms, delivery_status, reply_to_id,
                    edited_at_ms, redacted_at_ms, expires_at_ms, ephemeral_ttl_ms, arrived_at_ms
             FROM timeline_events
             WHERE message_id = ?1 OR current_edit_event_id = ?1
             LIMIT 1",
        )
        .map_err(|e| KupferdrahtError::Storage(format!("load_timeline_event prepare: {e}")))?;
    let raw = stmt
        .query_map(params![target_id], read_raw_timeline_row)
        .map_err(|e| KupferdrahtError::Storage(format!("load_timeline_event query: {e}")))?
        .next()
        .transpose()
        .map_err(|e| KupferdrahtError::Storage(format!("load_timeline_event row: {e}")))?;
    raw.map(assemble_timeline_event).transpose()
}

/// [`load_timeline_event`] within one room (Step 95b) — the id space is
/// per room (`PRIMARY KEY (message_id, room_id)`).
pub(super) fn load_timeline_event_in_room(
    store: &SqliteStore,
    room_id: &str,
    target_id: &str,
) -> Result<Option<TimelineEventRecord>, KupferdrahtError> {
    let conn = store.lock()?;
    let mut stmt = conn
        .prepare(
            "SELECT message_id, room_id, sender_user_id, sender_device_id,
                    kind, body,
                    sent_at_ms, delivery_status, reply_to_id,
                    edited_at_ms, redacted_at_ms, expires_at_ms, ephemeral_ttl_ms, arrived_at_ms
             FROM timeline_events
             WHERE room_id = ?1
               AND (message_id = ?2 OR current_edit_event_id = ?2)
             LIMIT 1",
        )
        .map_err(|e| {
            KupferdrahtError::Storage(format!("load_timeline_event_in_room prepare: {e}"))
        })?;
    let raw = stmt
        .query_map(params![room_id, target_id], read_raw_timeline_row)
        .map_err(|e| KupferdrahtError::Storage(format!("load_timeline_event_in_room query: {e}")))?
        .next()
        .transpose()
        .map_err(|e| KupferdrahtError::Storage(format!("load_timeline_event_in_room row: {e}")))?;
    raw.map(assemble_timeline_event).transpose()
}

pub(super) fn find_message_room(
    store: &SqliteStore,
    target_id: &str,
) -> Result<Vec<String>, KupferdrahtError> {
    let conn = store.lock()?;
    let mut stmt = conn
        .prepare(
            "SELECT DISTINCT room_id FROM timeline_events
              WHERE message_id = ?1 OR current_edit_event_id = ?1",
        )
        .map_err(|e| KupferdrahtError::Storage(format!("find_message_room prepare: {e}")))?;
    let rows: Vec<String> = stmt
        .query_map(params![target_id], |row| row.get::<_, String>(0))
        .map_err(|e| KupferdrahtError::Storage(format!("find_message_room query: {e}")))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| KupferdrahtError::Storage(format!("find_message_room row: {e}")))?;
    Ok(rows)
}

pub(super) fn canonical_message_id(
    store: &SqliteStore,
    room_id: &str,
    target_id: &str,
) -> Result<Option<String>, KupferdrahtError> {
    let conn = store.lock()?;
    conn.query_row(
        "SELECT message_id FROM timeline_events
          WHERE room_id = ?1
            AND (message_id = ?2 OR current_edit_event_id = ?2)
          LIMIT 1",
        params![room_id, target_id],
        |row| row.get::<_, String>(0),
    )
    .optional()
    .map_err(|e| KupferdrahtError::Storage(format!("canonical_message_id: {e}")))
}

pub(super) fn apply_message_delete(
    store: &SqliteStore,
    room_id: &str,
    target_message_id: &str,
    redacted_at_ms: i64,
) -> Result<bool, KupferdrahtError> {
    debug_assert!(
        !target_message_id.is_empty(),
        "apply_message_delete: target must be non-empty"
    );
    let storage_err = |e: rusqlite::Error| {
        KupferdrahtError::Storage(format!(
            "apply_message_delete {target_message_id}@{room_id}: {e}"
        ))
    };
    let mut conn = store.lock()?;
    let tx = super::write_tx::begin(&mut conn).map_err(storage_err)?;
    let applied =
        tombstone_message(&tx, room_id, target_message_id, redacted_at_ms).map_err(storage_err)?;
    if applied {
        forget_message_files(&tx, room_id, target_message_id).map_err(storage_err)?;
    }
    tx.commit().map_err(storage_err)?;
    Ok(applied)
}

/// Clear the deleted message's body, so the row no longer carries its
/// content at rest, and stamp the delete. Same chain-pointer semantics as
/// [`apply_message_edit`]: a delete targeting an already-edited row
/// references the latest edit-event-id rather than the original. `false`
/// when no row matched or a later delete is already persisted.
fn tombstone_message(
    conn: &rusqlite::Connection,
    room_id: &str,
    target_message_id: &str,
    redacted_at_ms: i64,
) -> rusqlite::Result<bool> {
    let updated = conn.execute(
        "UPDATE timeline_events
            SET body           = ?1,
                redacted_at_ms = ?2
          WHERE (message_id            = ?3
                 OR current_edit_event_id = ?3)
            AND room_id        = ?4
            AND (redacted_at_ms IS NULL OR redacted_at_ms < ?2)",
        params![b"".as_slice(), redacted_at_ms, target_message_id, room_id],
    )?;
    Ok(updated > 0)
}

/// Drop the attachment row of the deleted message — its key, token and
/// locator (Step 84). The cached ciphertext is the client's to remove.
fn forget_message_files(
    conn: &rusqlite::Connection,
    room_id: &str,
    target_message_id: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM attachments
          WHERE room_id = ?2
            AND message_id IN (SELECT message_id FROM timeline_events
                                WHERE (message_id            = ?1
                                       OR current_edit_event_id = ?1)
                                  AND room_id = ?2)",
        params![target_message_id, room_id],
    )?;
    Ok(())
}

pub(super) fn apply_message_edit(
    store: &SqliteStore,
    room_id: &str,
    replacing_message_id: &str,
    edit_event_id: &str,
    new_body: &str,
    edited_at_ms: i64,
) -> Result<bool, KupferdrahtError> {
    let conn = store.lock()?;
    apply_message_edit_on(
        &conn,
        room_id,
        replacing_message_id,
        edit_event_id,
        new_body,
        edited_at_ms,
    )
}

/// [`apply_message_edit`] on a given connection or transaction (Step 105).
pub(super) fn apply_message_edit_on(
    conn: &rusqlite::Connection,
    room_id: &str,
    replacing_message_id: &str,
    edit_event_id: &str,
    new_body: &str,
    edited_at_ms: i64,
) -> Result<bool, KupferdrahtError> {
    // Idempotence guard + edit chain: match either the row's
    // original message_id (first edit after creation) or its
    // current_edit_event_id (subsequent chain links — the
    // previous edit-event's id we recorded last time). Update
    // body + edited_at_ms only when this edit is strictly newer
    // than any persisted edit on the row.
    //
    // `kind = 'text'` guard: Wire's `MessageEdit` only ever carries
    // `Text`, so an edit targeting a non-text row (ping / location /
    // attachment) is malformed. Without this guard such an edit would
    // overwrite the packed `body` while leaving `kind` intact,
    // silently corrupting the row on the next `from_record`.
    let updated = conn
        .execute(
            "UPDATE timeline_events
                SET body                  = ?1,
                    edited_at_ms          = ?2,
                    current_edit_event_id = ?5
              WHERE (message_id            = ?3
                     OR current_edit_event_id = ?3)
                AND room_id               = ?4
                AND kind                  = 'text'
                AND (edited_at_ms IS NULL OR edited_at_ms < ?2)",
            params![
                new_body.as_bytes(),
                edited_at_ms,
                replacing_message_id,
                room_id,
                edit_event_id,
            ],
        )
        .map_err(|e| {
            KupferdrahtError::Storage(format!(
                "apply_message_edit {replacing_message_id}@{room_id}: {e}"
            ))
        })?;
    Ok(updated > 0)
}

fn read_raw_timeline_rows(
    store: &SqliteStore,
    room_id: &str,
) -> Result<Vec<RawTimelineRow>, KupferdrahtError> {
    let conn = store.lock()?;
    let mut stmt = conn
        .prepare(
            "SELECT message_id, room_id, sender_user_id, sender_device_id,
                    kind, body,
                    sent_at_ms, delivery_status, reply_to_id,
                    edited_at_ms, redacted_at_ms, expires_at_ms, ephemeral_ttl_ms, arrived_at_ms
             FROM timeline_events
             WHERE room_id = ?1
             ORDER BY arrived_at_ms ASC, rowid ASC",
        )
        .map_err(|e| KupferdrahtError::Storage(format!("load_timeline_events prepare: {e}")))?;
    stmt.query_map(params![room_id], read_raw_timeline_row)
        .map_err(|e| KupferdrahtError::Storage(format!("load_timeline_events query: {e}")))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| KupferdrahtError::Storage(format!("load_timeline_events row: {e}")))
}

pub(super) fn assemble_timeline_event(
    raw: RawTimelineRow,
) -> Result<TimelineEventRecord, KupferdrahtError> {
    Ok(TimelineEventRecord {
        message_id: raw.message_id,
        room_id: raw.room_id,
        sender_user_id: raw.sender_user_id,
        sender_device_id: raw.sender_device_id,
        kind: raw.kind,
        body: utf8_column(raw.body, "body")?,
        sent_at_ms: raw.sent_at_ms,
        arrived_at_ms: raw.arrived_at_ms,
        delivery_status: raw.delivery_status,
        reply_to_id: raw.reply_to_id,
        edited_at_ms: raw.edited_at_ms,
        redacted_at_ms: raw.redacted_at_ms,
        expires_at_ms: raw.expires_at_ms,
        ttl_ms: raw.ttl_ms,
    })
}

pub(super) struct RawTimelineRow {
    message_id: String,
    room_id: String,
    sender_user_id: String,
    sender_device_id: String,
    kind: String,
    body: Vec<u8>,
    sent_at_ms: i64,
    delivery_status: String,
    reply_to_id: Option<String>,
    edited_at_ms: Option<i64>,
    redacted_at_ms: Option<i64>,
    expires_at_ms: Option<i64>,
    ttl_ms: Option<i64>,
    arrived_at_ms: i64,
}

pub(super) fn read_raw_timeline_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawTimelineRow> {
    Ok(RawTimelineRow {
        message_id: row.get(0)?,
        room_id: row.get(1)?,
        sender_user_id: row.get(2)?,
        sender_device_id: row.get(3)?,
        kind: row.get(4)?,
        body: row.get(5)?,
        sent_at_ms: row.get(6)?,
        delivery_status: row.get(7)?,
        reply_to_id: row.get(8)?,
        edited_at_ms: row.get(9)?,
        redacted_at_ms: row.get(10)?,
        expires_at_ms: row.get(11)?,
        ttl_ms: row.get(12)?,
        arrived_at_ms: row.get(13)?,
    })
}

/// Step 89: tombstone every live row whose disappearing timer elapsed
/// (`expires_at_ms <= now_ms AND redacted_at_ms IS NULL`) — body
/// cleared, `redacted_at_ms = now_ms` — and return the retired keys.
pub(super) fn expire_disappearing_messages(
    store: &SqliteStore,
    now_ms: i64,
) -> Result<Vec<ExpiredMessageRecord>, KupferdrahtError> {
    let mut conn = store.lock()?;
    // Select-then-update inside one transaction: the UPDATE reuses the
    // exact predicate, so the returned keys are precisely the rows it
    // tombstoned (no RETURNING dependency on the bundled engine).
    // `IMMEDIATE` like every writing transaction of the store (Step 97d):
    // a `DEFERRED` read-then-write fails at once, not after
    // `busy_timeout`, when another process commits in between.
    let tx = super::write_tx::begin(&mut conn).map_err(|e| {
        KupferdrahtError::Storage(format!("expire_disappearing_messages begin: {e}"))
    })?;
    let due = select_due_disappearing_rows(&tx, now_ms)?;
    let updated = tx
        .execute(
            "UPDATE timeline_events
                SET body           = ?1,
                    redacted_at_ms = ?2
              WHERE expires_at_ms IS NOT NULL
                AND expires_at_ms <= ?2
                AND redacted_at_ms IS NULL",
            params![b"".as_slice(), now_ms],
        )
        .map_err(|e| {
            KupferdrahtError::Storage(format!("expire_disappearing_messages update: {e}"))
        })?;
    tx.commit().map_err(|e| {
        KupferdrahtError::Storage(format!("expire_disappearing_messages commit: {e}"))
    })?;
    assert_eq!(
        updated,
        due.len(),
        "expire_disappearing_messages: the tombstone UPDATE must retire exactly the selected rows"
    );
    Ok(due)
}

/// The keys of every live disappearing row whose timer elapsed at
/// `now_ms` — the same predicate the tombstone UPDATE applies.
fn select_due_disappearing_rows(
    conn: &rusqlite::Connection,
    now_ms: i64,
) -> Result<Vec<ExpiredMessageRecord>, KupferdrahtError> {
    let mut stmt = conn
        .prepare(
            "SELECT room_id, message_id, sender_user_id FROM timeline_events
              WHERE expires_at_ms IS NOT NULL
                AND expires_at_ms <= ?1
                AND redacted_at_ms IS NULL",
        )
        .map_err(|e| {
            KupferdrahtError::Storage(format!("expire_disappearing_messages prepare: {e}"))
        })?;
    stmt.query_map(params![now_ms], |row| {
        Ok(ExpiredMessageRecord {
            room_id: row.get(0)?,
            message_id: row.get(1)?,
            sender_user_id: row.get(2)?,
        })
    })
    .map_err(|e| KupferdrahtError::Storage(format!("expire_disappearing_messages query: {e}")))?
    .collect::<rusqlite::Result<Vec<_>>>()
    .map_err(|e| KupferdrahtError::Storage(format!("expire_disappearing_messages row: {e}")))
}

/// Step 89: distinct future expiry instants of live disappearing rows,
/// ascending — the resume path re-arms one timer per instant.
pub(super) fn pending_disappearing_expiries(
    store: &SqliteStore,
    now_ms: i64,
) -> Result<Vec<i64>, KupferdrahtError> {
    let conn = store.lock()?;
    let mut stmt = conn
        .prepare(
            "SELECT DISTINCT expires_at_ms FROM timeline_events
              WHERE expires_at_ms IS NOT NULL
                AND expires_at_ms > ?1
                AND redacted_at_ms IS NULL
              ORDER BY expires_at_ms ASC",
        )
        .map_err(|e| {
            KupferdrahtError::Storage(format!("pending_disappearing_expiries prepare: {e}"))
        })?;
    stmt.query_map(params![now_ms], |row| row.get::<_, i64>(0))
        .map_err(|e| {
            KupferdrahtError::Storage(format!("pending_disappearing_expiries query: {e}"))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| KupferdrahtError::Storage(format!("pending_disappearing_expiries row: {e}")))
}

/// Step 65 (review S12): upgrade a `submitted` row to `received` when
/// the Wire echo of an own message arrives — also a `sending` row whose
/// echo overtook its own outcome update (Step 97a). The WHERE guard
/// makes it idempotent — absent rows and rows already past `submitted`
/// report no upgrade.
pub(super) fn mark_delivery_received(
    store: &SqliteStore,
    room_id: &str,
    message_id: &str,
) -> Result<bool, KupferdrahtError> {
    let conn = store.lock()?;
    let changed = conn
        .execute(
            "UPDATE timeline_events SET delivery_status = 'received'
             WHERE room_id = ?1 AND message_id = ?2
               AND delivery_status IN ('submitted', 'sending')",
            rusqlite::params![room_id, message_id],
        )
        .map_err(|e| KupferdrahtError::Storage(format!("mark_delivery_received: {e}")))?;
    Ok(changed > 0)
}

/// Step 97a: settle an own send after its upload — `sending` →
/// `submitted` (`delivered`, with `expires_at_ms` when given) or
/// `failed`. The WHERE guard never downgrades a row the echo already
/// marked `received`, nor touches another row.
pub(super) fn mark_send_outcome(
    store: &SqliteStore,
    room_id: &str,
    message_id: &str,
    delivered: bool,
    expires_at_ms: Option<i64>,
    sent_at_ms: Option<i64>,
) -> Result<bool, KupferdrahtError> {
    let status = if delivered { "submitted" } else { "failed" };
    let conn = store.lock()?;
    let changed = conn
        .execute(
            "UPDATE timeline_events
                SET delivery_status = ?3,
                    expires_at_ms   = COALESCE(?4, expires_at_ms),
                    sent_at_ms      = COALESCE(?5, sent_at_ms)
              WHERE room_id = ?1 AND message_id = ?2
                AND delivery_status = 'sending'",
            params![room_id, message_id, status, expires_at_ms, sent_at_ms],
        )
        .map_err(|e| KupferdrahtError::Storage(format!("mark_send_outcome: {e}")))?;
    debug_assert!(
        changed <= 1,
        "mark_send_outcome: message_id is a PK per room"
    );
    // A repeat of the send's key answers this handle even when the row is gone.
    if changed > 0 && sent_at_ms.is_some() {
        conn.execute(
            "UPDATE send_keys SET sent_at_ms = ?3
              WHERE room_id = ?1 AND message_id = ?2 AND kind = 'message'",
            params![room_id, message_id, sent_at_ms],
        )
        .map_err(|e| KupferdrahtError::Storage(format!("mark_send_outcome key: {e}")))?;
    }
    Ok(changed > 0)
}

/// Step 79: flip a live `failed` row back to `submitted` after a
/// successful resend — `sent_at_ms` re-stamped to the resend instant,
/// `expires_at_ms` set to the re-armed timer (or cleared). Step 97a: an
/// abandoned `sending` row is a failed send too (the caller checked it
/// is no longer in flight). The WHERE guard makes it idempotent and
/// refuses rows that are not failed sends.
pub(super) fn mark_message_resent(
    store: &SqliteStore,
    room_id: &str,
    message_id: &str,
    times: &crate::storage::ResentTimes,
) -> Result<bool, KupferdrahtError> {
    let conn = store.lock()?;
    let changed = conn
        .execute(
            "UPDATE timeline_events
                SET delivery_status = 'submitted',
                    sent_at_ms      = ?3,
                    expires_at_ms   = ?4,
                    arrived_at_ms   = ?5
              WHERE room_id = ?1 AND message_id = ?2
                AND delivery_status IN ('failed', 'sending')
                AND redacted_at_ms IS NULL",
            params![
                room_id,
                message_id,
                times.sent_at_ms,
                times.expires_at_ms,
                times.arrived_at_ms
            ],
        )
        .map_err(|e| KupferdrahtError::Storage(format!("mark_message_resent: {e}")))?;
    debug_assert!(
        changed <= 1,
        "mark_message_resent: message_id is a PK per room"
    );
    Ok(changed > 0)
}

#[cfg(test)]
mod tests {
    use super::super::fresh_store;
    use crate::storage::Store as _;
    use crate::storage::records::{ExpiredMessageRecord, ResentTimes, TimelineEventRecord};

    /// A text row of `u@wire.test`'s device in `room@wire.test`, sent and
    /// arrived at 1000, in delivery state `status`.
    fn text_row(message_id: &str, status: &str, body: &str) -> TimelineEventRecord {
        TimelineEventRecord {
            message_id: message_id.into(),
            room_id: "room@wire.test".into(),
            sender_user_id: "u@wire.test".into(),
            sender_device_id: "dev".into(),
            kind: "text".into(),
            body: body.into(),
            sent_at_ms: 1000,
            arrived_at_ms: 1000,
            delivery_status: status.into(),
            ..TimelineEventRecord::default()
        }
    }

    /// Step 79: the resend flip is guarded — it moves exactly a live
    /// `failed` row to `submitted` (re-stamping `sent_at_ms`, setting
    /// the timer), reports `false` for anything else, and is
    /// idempotent.
    #[tokio::test]
    async fn mark_message_resent_flips_only_live_failed_rows() {
        let (_dir, store) = fresh_store();
        let room_id = "room@wire.test";
        let failed = TimelineEventRecord {
            ttl_ms: Some(5_000),
            ..text_row("m-failed", "failed", "stuck")
        };
        store.insert_timeline_event(&failed).await.expect("insert");
        let submitted = TimelineEventRecord {
            message_id: "m-sent".into(),
            delivery_status: "submitted".into(),
            ..failed.clone()
        };
        store
            .insert_timeline_event(&submitted)
            .await
            .expect("insert");

        assert!(
            store
                .mark_message_resent(
                    room_id,
                    "m-failed",
                    &ResentTimes {
                        sent_at_ms: 2000,
                        arrived_at_ms: 2500,
                        expires_at_ms: Some(7_000)
                    }
                )
                .await
                .expect("flip")
        );
        let rows = store.load_timeline_events(room_id).await.expect("load");
        let row = rows
            .iter()
            .find(|r| r.message_id == "m-failed")
            .expect("row");
        assert_eq!(row.delivery_status, "submitted");
        assert_eq!(row.sent_at_ms, 2000, "the backend's time of the resend");
        assert_eq!(row.arrived_at_ms, 2500, "to the tail of the timeline");
        assert_eq!(row.expires_at_ms, Some(7_000), "timer armed");
        assert_eq!(row.ttl_ms, Some(5_000), "the ttl survives (V28)");
        assert_eq!(row.body, "stuck");

        let later = ResentTimes {
            sent_at_ms: 3000,
            arrived_at_ms: 3000,
            expires_at_ms: None,
        };
        for (id, why) in [
            ("m-failed", "idempotent: the row is no longer failed"),
            ("m-sent", "a submitted row is not a failed send"),
            ("nope", "an absent row is not flipped"),
        ] {
            let flipped = store
                .mark_message_resent(room_id, id, &later)
                .await
                .expect(id);
            assert!(!flipped, "{why}");
        }
    }

    /// Step 97a: a `sending` row settles once — `submitted` (with the
    /// timer stamp) or `failed` — and an echo that overtook the outcome
    /// update (`received`) is never downgraded by it.
    #[tokio::test]
    async fn mark_send_outcome_settles_only_sending_rows_and_never_downgrades_an_echo() {
        let (_dir, store) = fresh_store();
        let room_id = "room@wire.test";
        let sending = text_row("m-sending", "sending", "on its way");
        store.insert_timeline_event(&sending).await.expect("insert");
        let echoed = TimelineEventRecord {
            message_id: "m-echoed".into(),
            ..sending.clone()
        };
        store.insert_timeline_event(&echoed).await.expect("insert");
        let status = |rows: &[TimelineEventRecord], id: &str| {
            rows.iter()
                .find(|r| r.message_id == id)
                .map(|r| (r.delivery_status.clone(), r.expires_at_ms))
                .expect("row")
        };

        assert!(
            store
                .mark_send_outcome(room_id, "m-sending", true, Some(9_000), Some(1_500))
                .await
                .expect("settle")
        );
        assert!(
            !store
                .mark_send_outcome(room_id, "m-sending", false, None, None)
                .await
                .expect("second settle"),
            "a settled row is not settled again"
        );
        assert!(
            store
                .mark_delivery_received(room_id, "m-echoed")
                .await
                .expect("echo"),
            "the echo upgrades a sending row"
        );
        assert!(
            !store
                .mark_send_outcome(room_id, "m-echoed", true, None, Some(1_500))
                .await
                .expect("late outcome"),
            "the outcome update never downgrades an echoed row"
        );

        let rows = store.load_timeline_events(room_id).await.expect("load");
        let sent_at = |id: &str| {
            rows.iter()
                .find(|r| r.message_id == id)
                .map(|r| r.sent_at_ms)
                .expect("row")
        };
        assert_eq!(sent_at("m-sending"), 1_500, "the answer's time");
        assert_eq!(sent_at("m-echoed"), 1_000, "a settled row keeps its own");
        assert_eq!(
            status(&rows, "m-sending"),
            ("submitted".to_string(), Some(9_000))
        );
        assert_eq!(status(&rows, "m-echoed"), ("received".to_string(), None));
    }

    /// After advancing the edit-chain pointer (`current_edit_event_id`),
    /// the former link id (`edit-1`) no longer resolves via
    /// `canonical_message_id`. Dispatch must snapshot the stable row PK
    /// *before* `apply_message_edit` when recording mentions /
    /// timeline diffs.
    #[tokio::test]
    async fn canonical_message_id_stops_matching_prior_chain_after_advance() {
        let (_dir, store) = fresh_store();
        let room_id = "room@wire.test";
        let base = TimelineEventRecord {
            message_id: "m-original".into(),
            room_id: room_id.into(),
            sender_user_id: "u@wire.test".into(),
            sender_device_id: "dev".into(),
            kind: "text".into(),
            body: "v0".into(),
            sent_at_ms: 1000,
            arrived_at_ms: 1000,
            delivery_status: "received".into(),
            reply_to_id: None,
            edited_at_ms: None,
            redacted_at_ms: None,
            expires_at_ms: None,
            ttl_ms: None,
        };
        store.insert_timeline_event(&base).await.expect("insert");
        store
            .apply_message_edit(room_id, "m-original", "edit-1", "v1", 2000)
            .await
            .expect("first edit");
        assert_eq!(
            store
                .canonical_message_id(room_id, "edit-1")
                .await
                .expect("canon lookup")
                .as_deref(),
            Some("m-original")
        );
        store
            .apply_message_edit(room_id, "edit-1", "edit-2", "v2", 3000)
            .await
            .expect("second edit");
        let vanished = store
            .canonical_message_id(room_id, "edit-1")
            .await
            .expect("canon after advance");
        assert!(
            vanished.is_none(),
            "expected prior link edit-1 to not resolve after advance, got {vanished:?}"
        );
    }

    fn sample_event(message_id: &str, room_id: &str) -> TimelineEventRecord {
        TimelineEventRecord {
            message_id: message_id.into(),
            room_id: room_id.into(),
            sender_user_id: "u@wire.test".into(),
            sender_device_id: "dev".into(),
            kind: "text".into(),
            body: "body".into(),
            sent_at_ms: 1000,
            arrived_at_ms: 1000,
            delivery_status: "received".into(),
            reply_to_id: None,
            edited_at_ms: None,
            redacted_at_ms: None,
            expires_at_ms: None,
            ttl_ms: None,
        }
    }

    /// Wire's `MessageEdit` only carries `Text`; an edit targeting a
    /// non-text row (a Step-26 ping here) must be a no-op so the packed
    /// `body` is never overwritten while `kind` stays put. Step 26.
    #[tokio::test]
    async fn apply_message_edit_skips_non_text_rows() {
        let (_dir, store) = fresh_store();
        let room_id = "room@wire.test";
        let mut ping = sample_event("m-ping", room_id);
        ping.kind = "ping".into();
        ping.body = "true".into();
        store.insert_timeline_event(&ping).await.expect("insert");

        let applied = store
            .apply_message_edit(room_id, "m-ping", "edit-1", "evil text", 2000)
            .await
            .expect("edit call");
        assert!(!applied, "edit on a non-text row must be a no-op");

        let rows = store
            .load_timeline_events(room_id)
            .await
            .expect("load events");
        let row = rows
            .iter()
            .find(|r| r.message_id == "m-ping")
            .expect("ping row present");
        assert_eq!(row.body, "true", "ping body must be unchanged");
        assert!(row.edited_at_ms.is_none(), "edited_at_ms must stay None");
    }

    #[tokio::test]
    async fn find_message_room_returns_single_room_for_known_message() {
        let (_dir, store) = fresh_store();
        let room_id = "room-a@wire.test";
        store
            .insert_timeline_event(&sample_event("m-1", room_id))
            .await
            .expect("insert");
        let rooms = store.find_message_room("m-1").await.expect("find");
        assert_eq!(rooms, vec![room_id.to_string()]);
    }

    #[tokio::test]
    async fn find_message_room_returns_empty_for_unknown_message() {
        let (_dir, store) = fresh_store();
        let rooms = store
            .find_message_room("never-was-sent")
            .await
            .expect("find");
        assert!(rooms.is_empty());
    }

    #[tokio::test]
    async fn find_message_room_resolves_edit_event_id() {
        // Wire's edit chain advances `current_edit_event_id`; the
        // resolver must surface the room when the lookup id matches
        // the chain pointer instead of the stable message_id.
        let (_dir, store) = fresh_store();
        let room_id = "room-b@wire.test";
        store
            .insert_timeline_event(&sample_event("m-original", room_id))
            .await
            .expect("insert");
        store
            .apply_message_edit(room_id, "m-original", "edit-1", "v1", 2000)
            .await
            .expect("edit");
        let rooms = store
            .find_message_room("edit-1")
            .await
            .expect("find via edit");
        assert_eq!(rooms, vec![room_id.to_string()]);
    }

    #[tokio::test]
    async fn find_message_room_returns_distinct_rooms_when_id_appears_in_multiple() {
        // The composite PK `(message_id, room_id)` permits the same
        // id in multiple rooms (UUIDv4 collisions astronomically
        // unlikely but the schema doesn't disallow it). The
        // resolver must list every match so the CLI can disambiguate.
        let (_dir, store) = fresh_store();
        store
            .insert_timeline_event(&sample_event("dup", "room-x@wire.test"))
            .await
            .expect("insert x");
        store
            .insert_timeline_event(&sample_event("dup", "room-y@wire.test"))
            .await
            .expect("insert y");
        let mut rooms = store.find_message_room("dup").await.expect("find");
        rooms.sort();
        assert_eq!(
            rooms,
            vec![
                "room-x@wire.test".to_string(),
                "room-y@wire.test".to_string()
            ]
        );
    }

    #[tokio::test]
    async fn load_timeline_event_returns_row_for_known_message_id() {
        let (_dir, store) = fresh_store();
        let room_id = "room-a@wire.test";
        store
            .insert_timeline_event(&sample_event("m-1", room_id))
            .await
            .expect("insert");
        let got = store
            .load_timeline_event("m-1")
            .await
            .expect("load")
            .expect("row present");
        assert_eq!(got.message_id, "m-1");
        assert_eq!(got.room_id, room_id);
        assert_eq!(got.sender_user_id, "u@wire.test");
        assert_eq!(got.body, "body");
    }

    #[tokio::test]
    async fn load_timeline_event_returns_none_for_unknown_id() {
        let (_dir, store) = fresh_store();
        let got = store
            .load_timeline_event("never-was-sent")
            .await
            .expect("load");
        assert!(got.is_none());
    }

    #[tokio::test]
    async fn load_timeline_event_resolves_edit_event_id() {
        // The lookup id may match the edit-chain pointer
        // (current_edit_event_id), not just the stable message_id.
        let (_dir, store) = fresh_store();
        let room_id = "room-b@wire.test";
        store
            .insert_timeline_event(&sample_event("m-original", room_id))
            .await
            .expect("insert");
        store
            .apply_message_edit(room_id, "m-original", "edit-1", "v1", 2000)
            .await
            .expect("edit");
        let got = store
            .load_timeline_event("edit-1")
            .await
            .expect("load via edit")
            .expect("row present");
        assert_eq!(got.message_id, "m-original", "PK stays stable across edits");
        assert_eq!(got.body, "v1", "body reflects the applied edit");
        assert_eq!(got.edited_at_ms, Some(2000));
    }

    // ---- Step 89: disappearing expiry sweep ----------------------------

    fn disappearing_event(
        message_id: &str,
        room_id: &str,
        expires_at_ms: Option<i64>,
    ) -> TimelineEventRecord {
        let mut event = sample_event(message_id, room_id);
        event.expires_at_ms = expires_at_ms;
        event
    }

    #[tokio::test]
    async fn insert_and_load_round_trip_expires_at_ms() {
        let (_dir, store) = fresh_store();
        let room_id = "room-e@wire.test";
        store
            .insert_timeline_event(&disappearing_event("m-eph", room_id, Some(9_000)))
            .await
            .expect("insert disappearing");
        store
            .insert_timeline_event(&disappearing_event("m-plain", room_id, None))
            .await
            .expect("insert plain");
        let rows = store.load_timeline_events(room_id).await.expect("load");
        let by_id = |id: &str| rows.iter().find(|r| r.message_id == id).expect(id).clone();
        assert_eq!(by_id("m-eph").expires_at_ms, Some(9_000));
        assert_eq!(by_id("m-plain").expires_at_ms, None);
        let single = store
            .load_timeline_event("m-eph")
            .await
            .expect("load one")
            .expect("row");
        assert_eq!(
            single.expires_at_ms,
            Some(9_000),
            "single-row read carries the stamp too"
        );
    }

    /// The record a sweep reports for a row of `disappearing_event`.
    fn expired(room_id: &str, message_id: &str) -> ExpiredMessageRecord {
        ExpiredMessageRecord {
            room_id: room_id.into(),
            message_id: message_id.into(),
            sender_user_id: "u@wire.test".into(),
        }
    }

    #[tokio::test]
    async fn expire_disappearing_messages_tombstones_only_due_rows_and_is_idempotent() {
        let (_dir, store) = fresh_store();
        let room_id = "room-e@wire.test";
        for (id, expires_at_ms) in [
            ("due", Some(5_000)),
            ("exact", Some(6_000)),
            ("future", Some(7_000)),
            ("plain", None),
        ] {
            store
                .insert_timeline_event(&disappearing_event(id, room_id, expires_at_ms))
                .await
                .expect(id);
        }

        let mut retired = store
            .expire_disappearing_messages(6_000)
            .await
            .expect("sweep");
        retired.sort_by(|a, b| a.message_id.cmp(&b.message_id));
        assert_eq!(
            retired,
            vec![expired(room_id, "due"), expired(room_id, "exact")],
            "rows at or before now are retired (<=), the future and plain rows are not"
        );

        let rows = store.load_timeline_events(room_id).await.expect("load");
        let by_id = |id: &str| rows.iter().find(|r| r.message_id == id).expect(id).clone();
        for id in ["due", "exact"] {
            let row = by_id(id);
            // Stamped with the sweep instant, the body cleared like a
            // MessageDelete tombstone, the expiry stamp kept.
            let retired = (
                row.redacted_at_ms,
                row.body.as_str(),
                row.expires_at_ms.is_some(),
            );
            assert_eq!(retired, (Some(6_000), "", true), "{id}");
        }
        for id in ["future", "plain"] {
            let row = by_id(id);
            assert_eq!(
                (row.redacted_at_ms, row.body.as_str()),
                (None, "body"),
                "{id}"
            );
        }

        let again = store
            .expire_disappearing_messages(6_000)
            .await
            .expect("second sweep");
        assert!(
            again.is_empty(),
            "already-tombstoned rows are not retired twice"
        );
    }

    #[tokio::test]
    async fn expire_disappearing_messages_skips_rows_already_deleted() {
        // A row the peer already deleted for everyone keeps its
        // original redaction stamp — the sweep must not rewrite it.
        let (_dir, store) = fresh_store();
        let room_id = "room-e@wire.test";
        store
            .insert_timeline_event(&disappearing_event("deleted", room_id, Some(5_000)))
            .await
            .expect("insert");
        assert!(
            store
                .apply_message_delete(room_id, "deleted", 4_000)
                .await
                .expect("delete")
        );
        let retired = store
            .expire_disappearing_messages(6_000)
            .await
            .expect("sweep");
        assert!(retired.is_empty());
        let row = store
            .load_timeline_event("deleted")
            .await
            .expect("load")
            .expect("row");
        assert_eq!(
            row.redacted_at_ms,
            Some(4_000),
            "the earlier delete stamp is kept"
        );
    }

    #[tokio::test]
    async fn pending_disappearing_expiries_lists_distinct_future_instants_ascending() {
        let (_dir, store) = fresh_store();
        let room_id = "room-e@wire.test";
        for (id, at) in [("a", 9_000), ("b", 7_000), ("c", 9_000), ("past", 1_000)] {
            store
                .insert_timeline_event(&disappearing_event(id, room_id, Some(at)))
                .await
                .expect("insert");
        }
        store
            .insert_timeline_event(&disappearing_event("plain", room_id, None))
            .await
            .expect("insert plain");
        let pending = store
            .pending_disappearing_expiries(5_000)
            .await
            .expect("pending");
        assert_eq!(
            pending,
            vec![7_000, 9_000],
            "distinct, ascending, strictly in the future"
        );
        assert!(
            store
                .expire_disappearing_messages(1_000)
                .await
                .expect("sweep")
                .iter()
                .any(|r| r.message_id == "past")
        );
        let after = store
            .pending_disappearing_expiries(9_500)
            .await
            .expect("pending");
        assert!(
            after.is_empty(),
            "nothing pending once every instant is in the past"
        );
    }
}
