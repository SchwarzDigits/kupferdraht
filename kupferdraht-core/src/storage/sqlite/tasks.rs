// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `tasks` table CRUD — the materialized task fold cache (Step 60c,
//! ADR-0014). One row per `(room_id, task_id)` carrying the latest
//! folded state; the upsert keeps `creator_user_id` / `created_at_ms`
//! first-writer-wins. Rebuild (redaction / V23 backfill) goes through
//! [`replace_room_tasks`].

use rusqlite::params;

use crate::error::KupferdrahtError;
use crate::storage::records::{TaskRecord, TimelineEventRecord};

use super::SqliteStore;
use super::timeline_events::{assemble_timeline_event, read_raw_timeline_row};

pub(super) fn upsert_task(store: &SqliteStore, task: &TaskRecord) -> Result<(), KupferdrahtError> {
    let conn = store.lock()?;
    upsert_task_on(&conn, task)
}

/// [`upsert_task`] on a given connection or transaction (Step 105).
pub(super) fn upsert_task_on(
    conn: &rusqlite::Connection,
    task: &TaskRecord,
) -> Result<(), KupferdrahtError> {
    if task.task_id.is_empty() || task.room_id.is_empty() {
        return Err(KupferdrahtError::InvalidInput(
            "upsert_task: task_id and room_id must be non-empty".into(),
        ));
    }
    conn.execute(
        "INSERT INTO tasks (
            room_id, task_id, title, description, assignee, due_at_ms,
            status, priority, sort_key, creator_user_id, created_at_ms,
            updated_at_ms, latest_message_id
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
         ON CONFLICT(room_id, task_id) DO UPDATE SET
            title             = excluded.title,
            description       = excluded.description,
            assignee          = excluded.assignee,
            due_at_ms         = excluded.due_at_ms,
            status            = excluded.status,
            priority          = excluded.priority,
            sort_key          = excluded.sort_key,
            updated_at_ms     = excluded.updated_at_ms,
            latest_message_id = excluded.latest_message_id",
        // creator_user_id + created_at_ms deliberately absent from the
        // UPDATE set: first writer wins (the task's creator).
        params![
            task.room_id,
            task.task_id,
            task.title,
            task.description,
            task.assignee,
            task.due_at_ms,
            task.status,
            task.priority,
            task.sort_key,
            task.creator_user_id,
            task.created_at_ms,
            task.updated_at_ms,
            task.latest_message_id,
        ],
    )
    .map_err(|e| {
        KupferdrahtError::Storage(format!(
            "upsert_task {}@{}: {e}",
            task.task_id, task.room_id
        ))
    })?;
    Ok(())
}

pub(super) fn replace_room_tasks(
    store: &SqliteStore,
    room_id: &str,
    tasks: &[TaskRecord],
) -> Result<(), KupferdrahtError> {
    let mut conn = store.lock()?;
    let tx = super::write_tx::begin(&mut conn)
        .map_err(|e| KupferdrahtError::Storage(format!("replace_room_tasks begin tx: {e}")))?;
    tx.execute("DELETE FROM tasks WHERE room_id = ?1", params![room_id])
        .map_err(|e| {
            KupferdrahtError::Storage(format!("replace_room_tasks delete {room_id}: {e}"))
        })?;
    for task in tasks {
        debug_assert!(
            task.room_id == room_id,
            "replace_room_tasks: record for foreign room"
        );
        tx.execute(
            "INSERT INTO tasks (
                room_id, task_id, title, description, assignee, due_at_ms,
                status, priority, sort_key, creator_user_id, created_at_ms,
                updated_at_ms, latest_message_id
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                task.room_id,
                task.task_id,
                task.title,
                task.description,
                task.assignee,
                task.due_at_ms,
                task.status,
                task.priority,
                task.sort_key,
                task.creator_user_id,
                task.created_at_ms,
                task.updated_at_ms,
                task.latest_message_id,
            ],
        )
        .map_err(|e| {
            KupferdrahtError::Storage(format!("replace_room_tasks insert {}: {e}", task.task_id))
        })?;
    }
    tx.commit()
        .map_err(|e| KupferdrahtError::Storage(format!("replace_room_tasks commit: {e}")))
}

fn read_task_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TaskRecord> {
    Ok(TaskRecord {
        room_id: row.get(0)?,
        task_id: row.get(1)?,
        title: row.get(2)?,
        description: row.get(3)?,
        assignee: row.get(4)?,
        due_at_ms: row.get(5)?,
        status: row.get(6)?,
        priority: row.get(7)?,
        sort_key: row.get(8)?,
        creator_user_id: row.get(9)?,
        created_at_ms: row.get(10)?,
        updated_at_ms: row.get(11)?,
        latest_message_id: row.get(12)?,
    })
}

const TASK_COLUMNS: &str = "room_id, task_id, title, description, assignee, due_at_ms,
     status, priority, sort_key, creator_user_id, created_at_ms,
     updated_at_ms, latest_message_id";

fn query_tasks(
    store: &SqliteStore,
    label: &str,
    where_clause: &str,
    args: &[&dyn rusqlite::types::ToSql],
) -> Result<Vec<TaskRecord>, KupferdrahtError> {
    let conn = store.lock()?;
    let sql = format!("SELECT {TASK_COLUMNS} FROM tasks WHERE {where_clause}");
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| KupferdrahtError::Storage(format!("{label} prepare: {e}")))?;
    stmt.query_map(args, read_task_row)
        .map_err(|e| KupferdrahtError::Storage(format!("{label} query: {e}")))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| KupferdrahtError::Storage(format!("{label} row: {e}")))
}

pub(super) fn load_tasks_for_room(
    store: &SqliteStore,
    room_id: &str,
) -> Result<Vec<TaskRecord>, KupferdrahtError> {
    query_tasks(store, "load_tasks_for_room", "room_id = ?1", &[&room_id])
}

pub(super) fn load_open_tasks_for_assignee(
    store: &SqliteStore,
    assignee: &str,
) -> Result<Vec<TaskRecord>, KupferdrahtError> {
    query_tasks(
        store,
        "load_open_tasks_for_assignee",
        "assignee = ?1 AND status IN ('open', 'in_progress')",
        &[&assignee],
    )
}

pub(super) fn load_tasks_for_creator(
    store: &SqliteStore,
    creator_user_id: &str,
) -> Result<Vec<TaskRecord>, KupferdrahtError> {
    query_tasks(
        store,
        "load_tasks_for_creator",
        "creator_user_id = ?1",
        &[&creator_user_id],
    )
}

pub(super) fn count_tasks(store: &SqliteStore) -> Result<u64, KupferdrahtError> {
    let conn = store.lock()?;
    conn.query_row("SELECT COUNT(*) FROM tasks", [], |row| row.get::<_, u64>(0))
        .map_err(|e| KupferdrahtError::Storage(format!("count_tasks: {e}")))
}

/// Every `kind = 'task'` timeline row across all rooms, arrival order
/// per room — the V23 backfill source.
pub(super) fn load_task_timeline_rows(
    store: &SqliteStore,
) -> Result<Vec<TimelineEventRecord>, KupferdrahtError> {
    let conn = store.lock()?;
    let mut stmt = conn
        .prepare(
            "SELECT message_id, room_id, sender_user_id, sender_device_id,
                    kind, body,
                    sent_at_ms, delivery_status, reply_to_id,
                    edited_at_ms, redacted_at_ms, expires_at_ms, ephemeral_ttl_ms
             FROM timeline_events
             WHERE kind = 'task'
             ORDER BY room_id ASC, sent_at_ms ASC, rowid ASC",
        )
        .map_err(|e| KupferdrahtError::Storage(format!("load_task_timeline_rows prepare: {e}")))?;
    let raw = stmt
        .query_map([], read_raw_timeline_row)
        .map_err(|e| KupferdrahtError::Storage(format!("load_task_timeline_rows query: {e}")))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| KupferdrahtError::Storage(format!("load_task_timeline_rows row: {e}")))?;
    raw.into_iter().map(assemble_timeline_event).collect()
}

#[cfg(test)]
mod tests {
    use super::super::fresh_store;
    use super::*;

    fn record(room: &str, task: &str, assignee: Option<&str>, status: &str) -> TaskRecord {
        TaskRecord {
            room_id: room.into(),
            task_id: task.into(),
            title: format!("title-{task}"),
            description: None,
            assignee: assignee.map(str::to_string),
            due_at_ms: None,
            status: status.into(),
            priority: None,
            sort_key: None,
            creator_user_id: "creator@example.org".into(),
            created_at_ms: 1000,
            updated_at_ms: 1000,
            latest_message_id: format!("m-{task}"),
        }
    }

    /// The V23 backfill query must read the full row shape — its
    /// column list had fallen behind `read_raw_timeline_row` when
    /// Step 89 appended `expires_at_ms` (caught while Step 79 appended
    /// `ephemeral_ttl_ms`): every backfill would have failed with an
    /// invalid column index the moment a task row existed.
    #[test]
    fn task_timeline_rows_backfill_query_reads_the_full_row() {
        let (_dir, store) = fresh_store();
        let row = crate::storage::records::TimelineEventRecord {
            message_id: "m-task".into(),
            room_id: "r@wire.com".into(),
            sender_user_id: "u@wire.com".into(),
            sender_device_id: "dev".into(),
            kind: "task".into(),
            body: "{}".into(),
            sent_at_ms: 1000,
            delivery_status: "submitted".into(),
            ..Default::default()
        };
        super::super::timeline_events::insert_timeline_event(&store, &row).expect("insert");
        let rows = load_task_timeline_rows(&store).expect("backfill query");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].message_id, "m-task");
        assert_eq!(rows[0].expires_at_ms, None);
        assert_eq!(rows[0].ephemeral_ttl_ms, None);
    }

    #[test]
    fn upsert_preserves_creator_and_created_at_on_conflict() {
        let (_dir, store) = fresh_store();
        upsert_task(&store, &record("r@wire.com", "t1", None, "open")).expect("insert");
        let mut update = record("r@wire.com", "t1", Some("anna@wire.com"), "done");
        update.creator_user_id = "someone-else@wire.com".into();
        update.created_at_ms = 9999;
        update.updated_at_ms = 2000;
        upsert_task(&store, &update).expect("upsert");
        let rows = load_tasks_for_room(&store, "r@wire.com").expect("load");
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row.status, "done", "state fields replaced");
        assert_eq!(row.assignee.as_deref(), Some("anna@wire.com"));
        assert_eq!(row.updated_at_ms, 2000);
        assert_eq!(
            row.creator_user_id, "creator@example.org",
            "creator is first-writer-wins"
        );
        assert_eq!(row.created_at_ms, 1000, "created_at is first-writer-wins");
    }

    #[test]
    fn assignee_and_creator_views_filter_as_indexed() {
        let (_dir, store) = fresh_store();
        upsert_task(
            &store,
            &record("r1@wire.com", "mine-open", Some("me@wire.com"), "open"),
        )
        .expect("a");
        upsert_task(
            &store,
            &record(
                "r2@wire.com",
                "mine-progress",
                Some("me@wire.com"),
                "in_progress",
            ),
        )
        .expect("b");
        upsert_task(
            &store,
            &record("r1@wire.com", "mine-done", Some("me@wire.com"), "done"),
        )
        .expect("c");
        upsert_task(
            &store,
            &record("r1@wire.com", "other", Some("anna@wire.com"), "open"),
        )
        .expect("d");
        let mine = load_open_tasks_for_assignee(&store, "me@wire.com").expect("mine");
        let mut ids: Vec<&str> = mine.iter().map(|t| t.task_id.as_str()).collect();
        ids.sort_unstable();
        assert_eq!(ids, vec!["mine-open", "mine-progress"]);
        let created = load_tasks_for_creator(&store, "creator@example.org").expect("created");
        assert_eq!(created.len(), 4, "creator view is status-agnostic");
        assert_eq!(count_tasks(&store).expect("count"), 4);
    }

    #[test]
    fn replace_room_tasks_rebuilds_only_that_room() {
        let (_dir, store) = fresh_store();
        upsert_task(&store, &record("r1@wire.com", "t1", None, "open")).expect("a");
        upsert_task(&store, &record("r2@wire.com", "t2", None, "open")).expect("b");
        replace_room_tasks(
            &store,
            "r1@wire.com",
            &[record("r1@wire.com", "t3", None, "open")],
        )
        .expect("replace");
        let r1 = load_tasks_for_room(&store, "r1@wire.com").expect("r1");
        assert_eq!(r1.len(), 1);
        assert_eq!(r1[0].task_id, "t3");
        let r2 = load_tasks_for_room(&store, "r2@wire.com").expect("r2");
        assert_eq!(r2.len(), 1, "other rooms untouched");
    }
}
