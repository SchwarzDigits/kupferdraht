// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Materialized `tasks`-table maintenance (Step 60c): per-room
//! rebuild after a carrier redaction and the one-time V23 backfill.
use super::{Client, KupferdrahtError, TaskRecord, fold_task_rows, task_record_from_folded};

/// Re-fold one room's task rows and replace its materialized cache —
/// after a task message was redacted (the wiped body drops out of the
/// fold), or during the V23 backfill. Step 60c.
pub(crate) async fn rebuild_room_task_cache(
    store: &std::sync::Arc<dyn crate::storage::Store>,
    room_id: &str,
) -> Result<(), KupferdrahtError> {
    let rows = store.load_timeline_events(room_id).await?;
    let folded = fold_task_rows(&rows);
    let records: Vec<TaskRecord> = folded
        .iter()
        .map(|f| task_record_from_folded(room_id, f))
        .collect();
    store.replace_room_tasks(room_id, &records).await
}

/// Rebuild the room's task cache iff the redacted message was a task
/// message (`kind` survives the body wipe). Called from both delete
/// paths after `apply_message_delete`. Step 60c.
pub(crate) async fn rebuild_tasks_if_redacted_task(
    store: &std::sync::Arc<dyn crate::storage::Store>,
    room_id: &str,
    target_message_id: &str,
) -> Result<(), KupferdrahtError> {
    let Some(row) = store.load_timeline_event(target_message_id).await? else {
        return Ok(());
    };
    if row.kind != "task" {
        return Ok(());
    }
    rebuild_room_task_cache(store, room_id).await
}

/// One-time backfill of the materialized `tasks` table after the V23
/// migration (Step 60c): when the cache is empty but task rows exist,
/// re-fold every room. Called once per resume from
/// `Client::load_rooms_from_store`; with a populated cache this is a
/// single COUNT query.
pub(crate) async fn backfill_task_cache(client: &Client) -> Result<(), KupferdrahtError> {
    let Some(store) = client.store_handle() else {
        return Ok(());
    };
    if store.count_tasks().await? > 0 {
        return Ok(());
    }
    let rows = store.load_task_timeline_rows().await?;
    let mut i = 0usize;
    while i < rows.len() {
        let room_id = rows[i].room_id.clone();
        let mut j = i;
        while j < rows.len() && rows[j].room_id == room_id {
            j += 1;
        }
        let folded = fold_task_rows(&rows[i..j]);
        let records: Vec<TaskRecord> = folded
            .iter()
            .map(|f| task_record_from_folded(&room_id, f))
            .collect();
        store.replace_room_tasks(&room_id, &records).await?;
        i = j;
    }
    Ok(())
}
