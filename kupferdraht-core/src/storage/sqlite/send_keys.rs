// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `send_keys` — the idempotency key of a keyed send and the outcome it
//! answers (Step 130a, V32). A key is written in the same transaction as
//! what it answers for — the outgoing row with its mentions or attachment,
//! or the reaction set — and that transaction first forgets the keys older
//! than [`SEND_KEY_RETENTION_MS`].

use rusqlite::{Connection, OptionalExtension, params};

use crate::error::KupferdrahtError;
use crate::storage::records::{
    AttachmentRecord, MentionRecord, SEND_KEY_RETENTION_MS, SendKeyRecord, TimelineEventRecord,
};

use super::SqliteStore;
use super::attachments::insert_attachment_on;
use super::event_writes::in_write_tx;
use super::mentions::replace_mentions_on;
use super::reactions::replace_user_reactions_on;
use super::timeline_events::try_insert_timeline_event_on;

/// The stored key `idempotency_key`, `None` when it was never written or
/// has been swept.
pub(super) fn send_key(
    store: &SqliteStore,
    idempotency_key: &str,
) -> Result<Option<SendKeyRecord>, KupferdrahtError> {
    debug_assert!(!idempotency_key.is_empty(), "send_key: key must be set");
    let conn = store.lock()?;
    conn.query_row(
        "SELECT idempotency_key, room_id, kind, message_id, sent_at_ms, created_at_ms
         FROM send_keys
         WHERE idempotency_key = ?1",
        params![idempotency_key],
        |row| {
            Ok(SendKeyRecord {
                idempotency_key: row.get(0)?,
                room_id: row.get(1)?,
                kind: row.get(2)?,
                message_id: row.get(3)?,
                sent_at_ms: row.get(4)?,
                created_at_ms: row.get(5)?,
            })
        },
    )
    .optional()
    .map_err(|e| KupferdrahtError::Storage(format!("send_key {idempotency_key}: {e}")))
}

/// The outgoing row, its attachment (or else its mentions) and its key as
/// one transaction. `Ok(false)` when the row already exists — nothing is
/// written then, neither the sweep nor the key.
pub(super) fn insert_timeline_event_keyed(
    store: &SqliteStore,
    event: &TimelineEventRecord,
    mentions: &[MentionRecord],
    attachment: Option<&AttachmentRecord>,
    key: &SendKeyRecord,
) -> Result<bool, KupferdrahtError> {
    debug_assert_eq!(
        event.message_id, key.message_id,
        "a message key answers its own row"
    );
    in_write_tx(store, "insert_timeline_event_keyed", |tx| {
        if !try_insert_timeline_event_on(tx, event)? {
            return Ok(false);
        }
        match attachment {
            Some(attachment) => insert_attachment_on(tx, attachment)?,
            None => replace_mentions_on(tx, &event.room_id, &event.message_id, mentions)?,
        }
        record_send_key_on(tx, key)?;
        Ok(true)
    })
}

/// The own reaction set on `canonical_message_id` and the toggle's key as
/// one transaction; the set is stamped with the key's `sent_at_ms`.
pub(super) fn replace_user_reactions_keyed(
    store: &SqliteStore,
    room_id: &str,
    canonical_message_id: &str,
    sender_user_id: &str,
    emojis: &[String],
    key: &SendKeyRecord,
) -> Result<(), KupferdrahtError> {
    in_write_tx(store, "replace_user_reactions_keyed", |tx| {
        replace_user_reactions_on(
            tx,
            room_id,
            canonical_message_id,
            sender_user_id,
            emojis,
            key.sent_at_ms,
        )?;
        record_send_key_on(tx, key)
    })
}

/// Forget the keys older than the retention, measured from `key`'s
/// `created_at_ms`, then insert `key`. A key already stored fails the
/// insert (and with it the caller's transaction): a key answers one call.
fn record_send_key_on(conn: &Connection, key: &SendKeyRecord) -> Result<(), KupferdrahtError> {
    debug_assert!(
        !key.idempotency_key.is_empty() && !key.message_id.is_empty(),
        "record_send_key: key and message id must be set"
    );
    let cutoff_ms = key.created_at_ms.saturating_sub(SEND_KEY_RETENTION_MS);
    conn.execute(
        "DELETE FROM send_keys WHERE created_at_ms < ?1",
        params![cutoff_ms],
    )
    .map_err(|e| KupferdrahtError::Storage(format!("sweep send_keys: {e}")))?;
    conn.execute(
        "INSERT INTO send_keys (
            idempotency_key, room_id, kind, message_id, sent_at_ms, created_at_ms
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            key.idempotency_key,
            key.room_id,
            key.kind,
            key.message_id,
            key.sent_at_ms,
            key.created_at_ms,
        ],
    )
    .map_err(|e| {
        KupferdrahtError::Storage(format!("insert send_key {}: {e}", key.idempotency_key))
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::fresh_store;
    use crate::storage::Store as _;
    use crate::storage::records::{SendKeyRecord, TimelineEventRecord};

    const ROOM: &str = "room@wire.test";

    fn row(message_id: &str) -> TimelineEventRecord {
        TimelineEventRecord {
            message_id: message_id.into(),
            room_id: ROOM.into(),
            sender_user_id: "u@wire.test".into(),
            sender_device_id: "dev".into(),
            kind: "text".into(),
            body: "hi".into(),
            sent_at_ms: 1,
            delivery_status: "sending".into(),
            ..TimelineEventRecord::default()
        }
    }

    fn key(idempotency_key: &str, message_id: &str) -> SendKeyRecord {
        SendKeyRecord {
            idempotency_key: idempotency_key.into(),
            room_id: ROOM.into(),
            kind: "message".into(),
            message_id: message_id.into(),
            sent_at_ms: 1,
            created_at_ms: 1,
        }
    }

    #[tokio::test]
    async fn an_existing_row_writes_no_key() {
        let (_dir, store) = fresh_store();
        assert!(
            store
                .insert_timeline_event_keyed(&row("m1"), &[], None, &key("k1", "m1"))
                .await
                .expect("first write")
        );
        let again = store
            .insert_timeline_event_keyed(&row("m1"), &[], None, &key("k2", "m1"))
            .await
            .expect("second write");
        assert!(!again, "the row already exists");
        assert_eq!(store.send_key("k2").await.expect("read"), None);
    }

    #[tokio::test]
    async fn a_reused_key_rolls_its_row_back() {
        let (_dir, store) = fresh_store();
        store
            .insert_timeline_event_keyed(&row("m1"), &[], None, &key("k1", "m1"))
            .await
            .expect("first write");
        store
            .insert_timeline_event_keyed(&row("m2"), &[], None, &key("k1", "m2"))
            .await
            .expect_err("a key answers one call");
        assert_eq!(store.load_timeline_event("m2").await.expect("read"), None);
        assert_eq!(
            store
                .send_key("k1")
                .await
                .expect("read")
                .map(|k| k.message_id),
            Some("m1".to_string())
        );
    }

    #[tokio::test]
    async fn a_keyed_reaction_set_and_its_key_land_together() {
        let (_dir, store) = fresh_store();
        let mut reaction_key = key("kr", "reaction-event");
        reaction_key.kind = "reaction".into();
        reaction_key.sent_at_ms = 7;
        store
            .replace_user_reactions_keyed(ROOM, "m1", "u@wire.test", &["👍".into()], &reaction_key)
            .await
            .expect("keyed replace");
        let reactions = store.load_reactions_for_room(ROOM).await.expect("read");
        assert_eq!(reactions.len(), 1);
        assert_eq!(reactions[0].sent_at_ms, 7, "stamped with the key's time");
        assert_eq!(
            store.send_key("kr").await.expect("read"),
            Some(reaction_key)
        );
    }
}
