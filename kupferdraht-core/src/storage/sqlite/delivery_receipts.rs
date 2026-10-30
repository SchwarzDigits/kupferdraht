// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `delivery_receipts` table CRUD. Step 25b. One row per
//! `(message_id, confirmer_user_id)` ack from a Wire
//! `Confirmation { type: DELIVERED }` event. Composite PK lets a
//! redelivered confirmation `INSERT OR IGNORE` cleanly. Mirror of
//! [`super::read_receipts`].

use rusqlite::params;

use crate::error::KupferdrahtError;
use crate::storage::records::DeliveryReceiptRecord;

use super::SqliteStore;

pub(super) fn apply_delivery_receipts(
    store: &SqliteStore,
    room_id: &str,
    confirmer_user_id: &str,
    message_ids: &[String],
    confirmed_at_ms: i64,
) -> Result<usize, KupferdrahtError> {
    if message_ids.is_empty() {
        return Ok(0);
    }
    let mut conn = store.lock()?;
    let tx = super::write_tx::begin(&mut conn)
        .map_err(|e| KupferdrahtError::Storage(format!("apply_delivery_receipts begin tx: {e}")))?;
    let mut inserted = 0usize;
    for message_id in message_ids {
        let n = tx
            .execute(
                "INSERT OR IGNORE INTO delivery_receipts (
                    message_id, room_id, confirmer_user_id, confirmed_at_ms
                 ) VALUES (?1, ?2, ?3, ?4)",
                params![message_id, room_id, confirmer_user_id, confirmed_at_ms],
            )
            .map_err(|e| {
                KupferdrahtError::Storage(format!(
                    "apply_delivery_receipts insert {message_id}: {e}"
                ))
            })?;
        inserted += n;
    }
    tx.commit()
        .map_err(|e| KupferdrahtError::Storage(format!("apply_delivery_receipts commit: {e}")))?;
    Ok(inserted)
}

pub(super) fn load_delivery_receipts_for_room(
    store: &SqliteStore,
    room_id: &str,
) -> Result<Vec<DeliveryReceiptRecord>, KupferdrahtError> {
    let conn = store.lock()?;
    let mut stmt = conn
        .prepare(
            "SELECT message_id, room_id, confirmer_user_id, confirmed_at_ms
             FROM delivery_receipts
             WHERE room_id = ?1
             ORDER BY confirmed_at_ms ASC",
        )
        .map_err(|e| {
            KupferdrahtError::Storage(format!("load_delivery_receipts_for_room prepare: {e}"))
        })?;
    let rows: Vec<DeliveryReceiptRecord> = stmt
        .query_map(params![room_id], |row| {
            Ok(DeliveryReceiptRecord {
                message_id: row.get(0)?,
                room_id: row.get(1)?,
                confirmer_user_id: row.get(2)?,
                confirmed_at_ms: row.get(3)?,
            })
        })
        .map_err(|e| {
            KupferdrahtError::Storage(format!("load_delivery_receipts_for_room query: {e}"))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| {
            KupferdrahtError::Storage(format!("load_delivery_receipts_for_room row: {e}"))
        })?;
    Ok(rows)
}
