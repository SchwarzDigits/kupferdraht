// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Full-state LWW folding (Step 60b): patch application and the
//! per-room fold over `timeline_events` rows.
use super::{
    FoldedTask, TaskItem, TaskPatch, TimelineEventRecord, TimelineItemKind, task_kind_from_body,
};

/// Apply a [`TaskPatch`] onto the current folded state, yielding the
/// next full state to send (create == update, ADR-0014).
pub(crate) fn apply_task_patch(current: &TaskItem, patch: &TaskPatch) -> TaskItem {
    TaskItem {
        task_id: current.task_id.clone(),
        title: patch.title.clone().unwrap_or_else(|| current.title.clone()),
        description: patch
            .description
            .clone()
            .or_else(|| current.description.clone()),
        assignee: patch.assignee.clone().or_else(|| current.assignee.clone()),
        due_at_ms: patch.due_at_ms.or(current.due_at_ms),
        status: patch.status.unwrap_or(current.status),
        priority: patch.priority.or(current.priority),
        sort_key: patch.sort_key.clone().or_else(|| current.sort_key.clone()),
    }
}

/// Fold a room's timeline rows into one entry per `task_id`: last
/// full state wins, in slice order (the caller supplies rows in
/// arrival order — the local proxy for the event stream's server
/// order, ADR-0014). Non-task rows, redacted rows (a `MessageDelete`
/// wipes the body), and malformed bodies contribute nothing — a
/// deleted or garbled message simply drops out of the fold.
pub(crate) fn fold_task_rows(rows: &[TimelineEventRecord]) -> Vec<FoldedTask> {
    let mut order: Vec<String> = Vec::new();
    let mut by_id: std::collections::HashMap<String, FoldedTask> = std::collections::HashMap::new();
    for row in rows {
        if row.kind != "task" || row.redacted_at_ms.is_some() {
            continue;
        }
        let Some(TimelineItemKind::Task(item)) = task_kind_from_body(&row.body) else {
            continue;
        };
        let item = *item;
        // Step 95c: rows stored before the decode-boundary check can
        // carry an empty id — it addresses no task.
        if item.task_id.is_empty() {
            continue;
        }
        if let Some(existing) = by_id.get_mut(&item.task_id) {
            // Full-state LWW: the later row replaces wholesale.
            existing.item = item;
            existing.updated_at_ms = row.sent_at_ms;
            existing.latest_message_id.clone_from(&row.message_id);
        } else {
            order.push(item.task_id.clone());
            by_id.insert(
                item.task_id.clone(),
                FoldedTask {
                    item,
                    creator_user_id: row.sender_user_id.clone(),
                    created_at_ms: row.sent_at_ms,
                    updated_at_ms: row.sent_at_ms,
                    latest_message_id: row.message_id.clone(),
                },
            );
        }
    }
    order
        .into_iter()
        .filter_map(|task_id| by_id.remove(&task_id))
        .collect()
}
