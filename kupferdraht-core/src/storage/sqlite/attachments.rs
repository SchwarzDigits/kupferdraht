// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `attachments` table CRUD. One row per `Asset` `GenericMessage`
//! Kupferdraht sends or receives. Step 11d (ADR-0011 §Storage shape).
//!
//! Anchored to the parent `timeline_events` row by
//! `(message_id, room_id)` composite primary key + FK. The
//! FK uses the `SQLite` default `ON DELETE NO ACTION` (CHANGELOG.md
//! §32 lesson — a CASCADE in this direction would let a future
//! cleanup pass silently drop asset metadata; an explicit two-
//! step delete is the safer contract).
//!
//! `otr_key_ct` is the AES-256-GCM ciphertext of the 32-byte raw
//! AES key the asset was encrypted with — encrypted via the
//! per-account DEK from Step 7c. The plaintext key never lives on
//! disk; callers that need it (send re-cache, receive decrypt)
//! unwrap on demand inside the `_ct` decrypt envelope. **This
//! module stores `otr_key_ct` verbatim** — the encrypt-on-write
//! step is the caller's responsibility (mirrors how
//! `session.access_token_ct` etc. are handled by their callers).

use rusqlite::{OptionalExtension, params};

use crate::error::KupferdrahtError;
use crate::storage::records::AttachmentRecord;

use super::SqliteStore;

pub(super) fn insert_attachment(
    store: &SqliteStore,
    record: &AttachmentRecord,
) -> Result<(), KupferdrahtError> {
    let conn = store.lock()?;
    insert_attachment_on(&conn, record)
}

/// [`insert_attachment`] on a given connection or transaction (Step 105).
pub(super) fn insert_attachment_on(
    conn: &rusqlite::Connection,
    record: &AttachmentRecord,
) -> Result<(), KupferdrahtError> {
    let size_bytes = i64::try_from(record.size_bytes).map_err(|_| {
        KupferdrahtError::Storage(format!(
            "insert_attachment: size_bytes {} overflows i64",
            record.size_bytes
        ))
    })?;
    conn.execute(
        "INSERT INTO attachments (
            message_id, room_id, mime_type, filename, size_bytes,
            sha256, otr_key, asset_id, asset_token, asset_domain,
            encryption_algo, cache_path, created_at_ms
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        params![
            record.message_id,
            record.room_id,
            record.mime_type,
            record.filename,
            size_bytes,
            record.sha256,
            record.otr_key_ct,
            record.asset_id,
            record.asset_token,
            record.asset_domain,
            record.encryption_algo,
            record.cache_path,
            record.created_at_ms,
        ],
    )
    .map_err(|e| {
        KupferdrahtError::Storage(format!(
            "insert_attachment ({}, {}): {e}",
            record.message_id, record.room_id
        ))
    })?;
    Ok(())
}

pub(super) fn get_attachment(
    store: &SqliteStore,
    message_id: &str,
    room_id: &str,
) -> Result<Option<AttachmentRecord>, KupferdrahtError> {
    let conn = store.lock()?;
    conn.query_row(
        "SELECT message_id, room_id, mime_type, filename, size_bytes,
                sha256, otr_key, asset_id, asset_token, asset_domain,
                encryption_algo, cache_path, created_at_ms
         FROM attachments WHERE message_id = ?1 AND room_id = ?2",
        params![message_id, room_id],
        record_from_row,
    )
    .optional()
    .map_err(|e| {
        KupferdrahtError::Storage(format!("get_attachment ({message_id}, {room_id}): {e}"))
    })
}

pub(super) fn list_attachments_for_room(
    store: &SqliteStore,
    room_id: &str,
) -> Result<Vec<AttachmentRecord>, KupferdrahtError> {
    let conn = store.lock()?;
    let mut stmt = conn
        .prepare(
            "SELECT message_id, room_id, mime_type, filename, size_bytes,
                    sha256, otr_key, asset_id, asset_token, asset_domain,
                    encryption_algo, cache_path, created_at_ms
             FROM attachments WHERE room_id = ?1
             ORDER BY created_at_ms ASC",
        )
        .map_err(|e| {
            KupferdrahtError::Storage(format!("list_attachments_for_room prepare: {e}"))
        })?;
    let rows: Vec<AttachmentRecord> = stmt
        .query_map(params![room_id], record_from_row)
        .map_err(|e| KupferdrahtError::Storage(format!("list_attachments_for_room query: {e}")))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| KupferdrahtError::Storage(format!("list_attachments_for_room row: {e}")))?;
    Ok(rows)
}

pub(super) fn update_attachment_cache_path(
    store: &SqliteStore,
    message_id: &str,
    room_id: &str,
    cache_path: Option<&str>,
) -> Result<(), KupferdrahtError> {
    let conn = store.lock()?;
    conn.execute(
        "UPDATE attachments SET cache_path = ?1
         WHERE message_id = ?2 AND room_id = ?3",
        params![cache_path, message_id, room_id],
    )
    .map_err(|e| {
        KupferdrahtError::Storage(format!(
            "update_attachment_cache_path ({message_id}, {room_id}): {e}"
        ))
    })?;
    Ok(())
}

pub(super) fn delete_attachment(
    store: &SqliteStore,
    message_id: &str,
    room_id: &str,
) -> Result<(), KupferdrahtError> {
    let conn = store.lock()?;
    conn.execute(
        "DELETE FROM attachments WHERE message_id = ?1 AND room_id = ?2",
        params![message_id, room_id],
    )
    .map_err(|e| {
        KupferdrahtError::Storage(format!("delete_attachment ({message_id}, {room_id}): {e}"))
    })?;
    Ok(())
}

fn record_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AttachmentRecord> {
    let size_bytes_i64: i64 = row.get(4)?;
    Ok(AttachmentRecord {
        message_id: row.get(0)?,
        room_id: row.get(1)?,
        mime_type: row.get(2)?,
        filename: row.get(3)?,
        size_bytes: u64::try_from(size_bytes_i64).unwrap_or(0),
        sha256: row.get(5)?,
        otr_key_ct: row.get(6)?,
        asset_id: row.get(7)?,
        asset_token: row.get(8)?,
        asset_domain: row.get(9)?,
        encryption_algo: row.get(10)?,
        cache_path: row.get(11)?,
        created_at_ms: row.get(12)?,
    })
}

#[cfg(test)]
mod tests {
    use crate::storage::Store as _;
    use crate::storage::records::{AttachmentRecord, TimelineEventRecord};
    use crate::storage::sqlite::fresh_store;

    fn sample_timeline(message_id: &str, room_id: &str) -> TimelineEventRecord {
        TimelineEventRecord {
            message_id: message_id.into(),
            room_id: room_id.into(),
            sender_user_id: "u@wire.test".into(),
            sender_device_id: "dev".into(),
            kind: "asset".into(),
            body: String::new(),
            sent_at_ms: 1_000,
            delivery_status: "received".into(),
            reply_to_id: None,
            edited_at_ms: None,
            redacted_at_ms: None,
            expires_at_ms: None,
            ephemeral_ttl_ms: None,
        }
    }

    fn sample_attachment(message_id: &str, room_id: &str) -> AttachmentRecord {
        AttachmentRecord {
            message_id: message_id.into(),
            room_id: room_id.into(),
            mime_type: "image/png".into(),
            filename: Some("photo.png".into()),
            size_bytes: 1_024,
            sha256: vec![0xABu8; 32],
            otr_key_ct: vec![0xCDu8; 48], // simulated AES-GCM ciphertext (32B key + 12B nonce + 16B tag — 60B but Kupferdraht's envelope shape is opaque here)
            asset_id: "asset-key-uuid".into(),
            asset_token: Some("private-token-jwt".into()),
            asset_domain: "prod.wire.com".into(),
            encryption_algo: "AES_CBC".into(),
            cache_path: None,
            created_at_ms: 2_000,
        }
    }

    #[tokio::test]
    async fn insert_then_get_round_trips_every_column() {
        let (_dir, store) = fresh_store();
        let room_id = "room-a@wire.test";
        let message_id = "msg-1";
        store
            .insert_timeline_event(&sample_timeline(message_id, room_id))
            .await
            .expect("parent timeline_events");
        let attachment = sample_attachment(message_id, room_id);
        store.insert_attachment(&attachment).await.expect("insert");
        let fetched = store
            .get_attachment(message_id, room_id)
            .await
            .expect("get")
            .expect("present");
        assert_eq!(fetched, attachment);
    }

    #[tokio::test]
    async fn get_returns_none_when_absent() {
        let (_dir, store) = fresh_store();
        let found = store
            .get_attachment("nope", "nope@nope")
            .await
            .expect("get");
        assert!(found.is_none());
    }

    #[tokio::test]
    async fn insert_without_parent_timeline_event_fails_fk() {
        // FK constraint must reject orphan attachments. Step 11f's
        // orchestrator inserts the timeline_events row first.
        let (_dir, store) = fresh_store();
        let err = store
            .insert_attachment(&sample_attachment("ghost", "room-z@wire.test"))
            .await
            .expect_err("orphan insert must fail FK");
        assert!(
            matches!(err, crate::error::KupferdrahtError::Storage(_)),
            "got {err:?}"
        );
    }

    #[tokio::test]
    async fn list_for_room_orders_by_created_at_ms_ascending() {
        let (_dir, store) = fresh_store();
        let room_id = "room-x@wire.test";
        for (i, ts) in [(1, 3_000_i64), (2, 1_000_i64), (3, 2_000_i64)]
            .iter()
            .enumerate()
        {
            let message_id = format!("msg-{}", i + 1);
            store
                .insert_timeline_event(&sample_timeline(&message_id, room_id))
                .await
                .expect("parent");
            let mut a = sample_attachment(&message_id, room_id);
            a.created_at_ms = ts.1;
            // Per the schema, message_id is informational beyond
            // the FK PK; use the (i + 1) suffix above to keep
            // each row's PK unique.
            let _ = i; // silence unused
            store.insert_attachment(&a).await.expect("insert");
        }
        let list = store
            .list_attachments_for_room(room_id)
            .await
            .expect("list");
        let ts: Vec<i64> = list.iter().map(|a| a.created_at_ms).collect();
        assert_eq!(ts, vec![1_000, 2_000, 3_000], "ascending by created_at_ms");
    }

    #[tokio::test]
    async fn list_for_room_filters_by_room() {
        let (_dir, store) = fresh_store();
        let room_a = "room-a@wire.test";
        let room_b = "room-b@wire.test";
        store
            .insert_timeline_event(&sample_timeline("msg-a", room_a))
            .await
            .expect("parent a");
        store
            .insert_timeline_event(&sample_timeline("msg-b", room_b))
            .await
            .expect("parent b");
        store
            .insert_attachment(&sample_attachment("msg-a", room_a))
            .await
            .expect("insert a");
        store
            .insert_attachment(&sample_attachment("msg-b", room_b))
            .await
            .expect("insert b");
        let only_a = store
            .list_attachments_for_room(room_a)
            .await
            .expect("list a");
        assert_eq!(only_a.len(), 1);
        assert_eq!(only_a[0].room_id, room_a);
    }

    #[tokio::test]
    async fn update_cache_path_sets_and_clears() {
        let (_dir, store) = fresh_store();
        let room_id = "room-c@wire.test";
        let message_id = "msg-c";
        store
            .insert_timeline_event(&sample_timeline(message_id, room_id))
            .await
            .expect("parent");
        store
            .insert_attachment(&sample_attachment(message_id, room_id))
            .await
            .expect("insert");
        // Set.
        store
            .update_attachment_cache_path(message_id, room_id, Some("/var/kupferdraht/cache/x.enc"))
            .await
            .expect("set path");
        let after_set = store
            .get_attachment(message_id, room_id)
            .await
            .expect("get")
            .expect("present");
        assert_eq!(
            after_set.cache_path.as_deref(),
            Some("/var/kupferdraht/cache/x.enc")
        );
        // Clear.
        store
            .update_attachment_cache_path(message_id, room_id, None)
            .await
            .expect("clear path");
        let after_clear = store
            .get_attachment(message_id, room_id)
            .await
            .expect("get")
            .expect("present");
        assert!(after_clear.cache_path.is_none(), "cache_path cleared");
    }

    #[tokio::test]
    async fn update_cache_path_is_silent_when_row_absent() {
        // Idempotent / no-op: setting a path on a non-existent row
        // succeeds without error and does nothing.
        let (_dir, store) = fresh_store();
        store
            .update_attachment_cache_path("ghost", "ghost@nope", Some("/tmp/x"))
            .await
            .expect("no-op for missing row");
    }

    #[tokio::test]
    async fn delete_attachment_removes_row() {
        let (_dir, store) = fresh_store();
        let room_id = "room-d@wire.test";
        let message_id = "msg-d";
        store
            .insert_timeline_event(&sample_timeline(message_id, room_id))
            .await
            .expect("parent");
        store
            .insert_attachment(&sample_attachment(message_id, room_id))
            .await
            .expect("insert");
        store
            .delete_attachment(message_id, room_id)
            .await
            .expect("delete");
        let after = store
            .get_attachment(message_id, room_id)
            .await
            .expect("get");
        assert!(after.is_none(), "row gone after delete");
    }

    #[tokio::test]
    async fn delete_attachment_is_idempotent_for_missing_row() {
        let (_dir, store) = fresh_store();
        store
            .delete_attachment("ghost", "ghost@nope")
            .await
            .expect("idempotent delete");
    }

    #[tokio::test]
    async fn otr_key_ct_persists_verbatim_blob() {
        // The Store layer treats `otr_key_ct` as an opaque blob —
        // the per-account DEK envelope is applied by the caller,
        // not this module. Round-trip preserves the bytes exactly.
        let (_dir, store) = fresh_store();
        let room_id = "room-e@wire.test";
        let message_id = "msg-e";
        store
            .insert_timeline_event(&sample_timeline(message_id, room_id))
            .await
            .expect("parent");
        let mut a = sample_attachment(message_id, room_id);
        a.otr_key_ct = (0u8..=255u8).collect();
        store.insert_attachment(&a).await.expect("insert");
        let fetched = store
            .get_attachment(message_id, room_id)
            .await
            .expect("get")
            .expect("present");
        assert_eq!(fetched.otr_key_ct, a.otr_key_ct);
        assert_eq!(fetched.otr_key_ct.len(), 256);
    }
}
