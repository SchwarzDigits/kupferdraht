// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! The combined writes of one event — a timeline row and its dependent
//! row (mentions, attachment, task fold) — as one transaction (Step 105).
//! Since Step 97e an inbound notification is processed at least once; a
//! half-written event would be skipped as a duplicate on the replay, so
//! both writes land together or not at all.

use rusqlite::Connection;

use crate::error::KupferdrahtError;
use crate::storage::{
    AttachmentRecord, MentionRecord, MessageEditRecord, TaskRecord, TimelineEventRecord,
};

use super::SqliteStore;
use super::attachments::insert_attachment_on;
use super::mentions::replace_mentions_on;
use super::tasks::upsert_task_on;
use super::timeline_events::{apply_message_edit_on, try_insert_timeline_event_on};

/// Run `write` in one `IMMEDIATE` transaction; an error rolls all of it
/// back.
fn in_write_tx<T>(
    store: &SqliteStore,
    op: &str,
    write: impl FnOnce(&Connection) -> Result<T, KupferdrahtError>,
) -> Result<T, KupferdrahtError> {
    let mut conn = store.lock()?;
    let tx = super::write_tx::begin(&mut conn)
        .map_err(|e| KupferdrahtError::Storage(format!("{op} begin tx: {e}")))?;
    let out = write(&tx)?;
    tx.commit()
        .map_err(|e| KupferdrahtError::Storage(format!("{op} commit: {e}")))?;
    Ok(out)
}

pub(super) fn insert_timeline_event_with_mentions(
    store: &SqliteStore,
    event: &TimelineEventRecord,
    mentions: &[MentionRecord],
) -> Result<bool, KupferdrahtError> {
    in_write_tx(store, "insert_timeline_event_with_mentions", |tx| {
        if !try_insert_timeline_event_on(tx, event)? {
            return Ok(false);
        }
        replace_mentions_on(tx, &event.room_id, &event.message_id, mentions)?;
        Ok(true)
    })
}

pub(super) fn insert_timeline_event_with_attachment(
    store: &SqliteStore,
    event: &TimelineEventRecord,
    attachment: &AttachmentRecord,
) -> Result<bool, KupferdrahtError> {
    debug_assert_eq!(
        event.message_id, attachment.message_id,
        "one event's two rows"
    );
    in_write_tx(store, "insert_timeline_event_with_attachment", |tx| {
        if !try_insert_timeline_event_on(tx, event)? {
            return Ok(false);
        }
        insert_attachment_on(tx, attachment)?;
        Ok(true)
    })
}

pub(super) fn insert_timeline_event_with_task(
    store: &SqliteStore,
    event: &TimelineEventRecord,
    task: &TaskRecord,
) -> Result<bool, KupferdrahtError> {
    in_write_tx(store, "insert_timeline_event_with_task", |tx| {
        if !try_insert_timeline_event_on(tx, event)? {
            return Ok(false);
        }
        upsert_task_on(tx, task)?;
        Ok(true)
    })
}

pub(super) fn apply_message_edit_with_mentions(
    store: &SqliteStore,
    edit: &MessageEditRecord,
    mentions: &[MentionRecord],
) -> Result<bool, KupferdrahtError> {
    in_write_tx(store, "apply_message_edit_with_mentions", |tx| {
        let applied = apply_message_edit_on(
            tx,
            &edit.room_id,
            &edit.replacing_message_id,
            &edit.edit_event_id,
            &edit.new_body,
            edit.edited_at_ms,
        )?;
        if applied {
            replace_mentions_on(tx, &edit.room_id, &edit.message_id, mentions)?;
        }
        Ok(applied)
    })
}

#[cfg(test)]
mod tests {
    use crate::storage::sqlite::{SqliteStore, fresh_store};
    use crate::storage::{
        AttachmentRecord, MentionRecord, MessageEditRecord, Store, TaskRecord, TimelineEventRecord,
    };

    const ROOM: &str = "room@wire.test";

    fn row(message_id: &str, kind: &str, body: &str) -> TimelineEventRecord {
        TimelineEventRecord {
            message_id: message_id.into(),
            room_id: ROOM.into(),
            sender_user_id: "u@wire.test".into(),
            sender_device_id: "dev".into(),
            kind: kind.into(),
            body: body.into(),
            sent_at_ms: 1_000,
            delivery_status: "received".into(),
            reply_to_id: None,
            edited_at_ms: None,
            redacted_at_ms: None,
            expires_at_ms: None,
            ephemeral_ttl_ms: None,
        }
    }

    fn mention(message_id: &str, user_id: &str) -> MentionRecord {
        MentionRecord {
            message_id: message_id.into(),
            room_id: ROOM.into(),
            user_id: user_id.into(),
            start: 0,
            length: 4,
        }
    }

    fn attachment(message_id: &str) -> AttachmentRecord {
        AttachmentRecord {
            message_id: message_id.into(),
            room_id: ROOM.into(),
            mime_type: "image/png".into(),
            filename: Some("cat.png".into()),
            size_bytes: 3,
            sha256: vec![1; 32],
            otr_key_ct: vec![2; 32],
            asset_id: "3-1-asset".into(),
            asset_token: None,
            asset_domain: "wire.test".into(),
            encryption_algo: "AES_CBC".into(),
            cache_path: None,
            created_at_ms: 1_000,
        }
    }

    fn task(message_id: &str) -> TaskRecord {
        TaskRecord {
            room_id: ROOM.into(),
            task_id: "task-1".into(),
            title: "Buy milk".into(),
            description: None,
            assignee: None,
            due_at_ms: None,
            status: "open".into(),
            priority: None,
            sort_key: None,
            creator_user_id: "u@wire.test".into(),
            created_at_ms: 1_000,
            updated_at_ms: 1_000,
            latest_message_id: message_id.into(),
        }
    }

    /// Abort every insert into `table` — the second write of an event.
    fn fail_inserts_into(store: &SqliteStore, table: &str) {
        store
            .lock()
            .expect("lock")
            .execute_batch(&format!(
                "CREATE TRIGGER fail_{table} BEFORE INSERT ON {table} \
                 BEGIN SELECT RAISE(ABORT, 'injected {table} failure'); END;"
            ))
            .expect("trigger");
    }

    fn heal(store: &SqliteStore, table: &str) {
        store
            .lock()
            .expect("lock")
            .execute_batch(&format!("DROP TRIGGER fail_{table};"))
            .expect("drop trigger");
    }

    async fn row_exists(store: &SqliteStore, message_id: &str) -> bool {
        store
            .load_timeline_events(ROOM)
            .await
            .expect("load")
            .iter()
            .any(|r| r.message_id == message_id)
    }

    async fn mention_users(store: &SqliteStore) -> Vec<String> {
        store
            .load_mentions_for_room(ROOM)
            .await
            .expect("mentions")
            .into_iter()
            .map(|m| m.user_id)
            .collect()
    }

    #[tokio::test]
    async fn a_text_row_and_its_mentions_are_one_write() {
        let (_dir, store) = fresh_store();
        let text = row("m-text", "text", "@bob hi");
        fail_inserts_into(&store, "mentions");

        let failed = store
            .insert_timeline_event_with_mentions(&text, &[mention("m-text", "bob@wire.test")])
            .await;
        assert!(failed.is_err(), "the injected failure surfaces: {failed:?}");
        assert!(
            !row_exists(&store, "m-text").await,
            "the row is rolled back"
        );

        heal(&store, "mentions");
        let fresh = store
            .insert_timeline_event_with_mentions(&text, &[mention("m-text", "bob@wire.test")])
            .await
            .expect("retry");
        assert!(fresh, "the retry writes the row");
        assert_eq!(
            mention_users(&store).await,
            vec!["bob@wire.test".to_string()]
        );
    }

    #[tokio::test]
    async fn an_asset_row_and_its_attachment_are_one_write() {
        let (_dir, store) = fresh_store();
        let asset = row("m-asset", "asset", "cat.png");
        fail_inserts_into(&store, "attachments");

        let failed = store
            .insert_timeline_event_with_attachment(&asset, &attachment("m-asset"))
            .await;
        assert!(failed.is_err(), "the injected failure surfaces: {failed:?}");
        assert!(
            !row_exists(&store, "m-asset").await,
            "the row is rolled back"
        );

        heal(&store, "attachments");
        assert!(
            store
                .insert_timeline_event_with_attachment(&asset, &attachment("m-asset"))
                .await
                .expect("retry")
        );
        assert!(
            store
                .get_attachment("m-asset", ROOM)
                .await
                .expect("load")
                .is_some(),
            "the retry writes the attachment"
        );
    }

    #[tokio::test]
    async fn a_task_row_and_its_fold_are_one_write() {
        let (_dir, store) = fresh_store();
        let task_row = row("m-task", "task", "{}");
        fail_inserts_into(&store, "tasks");

        let failed = store
            .insert_timeline_event_with_task(&task_row, &task("m-task"))
            .await;
        assert!(failed.is_err(), "the injected failure surfaces: {failed:?}");
        assert!(
            !row_exists(&store, "m-task").await,
            "the row is rolled back"
        );

        heal(&store, "tasks");
        assert!(
            store
                .insert_timeline_event_with_task(&task_row, &task("m-task"))
                .await
                .expect("retry")
        );
        assert_eq!(
            store.load_tasks_for_room(ROOM).await.expect("tasks").len(),
            1,
            "the retry writes the fold"
        );
    }

    #[tokio::test]
    async fn an_edit_and_its_mentions_are_one_write() {
        let (_dir, store) = fresh_store();
        store
            .insert_timeline_event(&row("m-orig", "text", "hi"))
            .await
            .expect("original");
        let edit = MessageEditRecord {
            room_id: ROOM.into(),
            message_id: "m-orig".into(),
            replacing_message_id: "m-orig".into(),
            edit_event_id: "edit-1".into(),
            new_body: "@bob hi".into(),
            edited_at_ms: 2_000,
        };
        fail_inserts_into(&store, "mentions");

        let failed = store
            .apply_message_edit_with_mentions(&edit, &[mention("m-orig", "bob@wire.test")])
            .await;
        assert!(failed.is_err(), "the injected failure surfaces: {failed:?}");
        let rows = store.load_timeline_events(ROOM).await.expect("load");
        assert_eq!(rows[0].body, "hi", "the edit is rolled back");

        heal(&store, "mentions");
        assert!(
            store
                .apply_message_edit_with_mentions(&edit, &[mention("m-orig", "bob@wire.test")])
                .await
                .expect("retry"),
            "the retry applies the edit"
        );
        assert_eq!(
            mention_users(&store).await,
            vec!["bob@wire.test".to_string()]
        );
    }

    #[tokio::test]
    async fn an_existing_row_is_reported_and_left_alone() {
        let (_dir, store) = fresh_store();
        let text = row("m-dup", "text", "@bob hi");
        assert!(
            store
                .insert_timeline_event_with_mentions(&text, &[mention("m-dup", "bob@wire.test")])
                .await
                .expect("first")
        );

        let again = store
            .insert_timeline_event_with_mentions(&text, &[mention("m-dup", "eve@wire.test")])
            .await
            .expect("an existing row is not an error");

        assert!(!again, "reported as already present");
        assert_eq!(
            mention_users(&store).await,
            vec!["bob@wire.test".to_string()],
            "the stored mentions are untouched"
        );
    }
}
