// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Task read views (Step 60c): per-room and cross-room lists over
//! the materialized cache, plus the record conversions they share
//! with the cache-maintenance path.
use super::{
    Client, FoldedTask, KupferdrahtError, RoomTask, TaskItem, TaskRecord, TaskStatus,
    priority_as_str, priority_from_str, sort_folded_tasks, sort_room_tasks, status_as_str,
    status_from_str,
};

/// Cross-room "my open work" view (Step 60c, ADR-0014): tasks
/// assigned to `self_user_id` that still need doing (`Open` or
/// `InProgress`), across all rooms.
pub(crate) fn filter_my_tasks(tasks: &[RoomTask], self_user_id: &str) -> Vec<RoomTask> {
    tasks
        .iter()
        .filter(|t| {
            t.task.item.assignee.as_deref() == Some(self_user_id)
                && matches!(
                    t.task.item.status,
                    TaskStatus::Open | TaskStatus::InProgress
                )
        })
        .cloned()
        .collect()
}

/// Cross-room "what I delegated" view (Step 60c, ADR-0014): tasks
/// `self_user_id` created and assigned to someone else — any status,
/// so the creator sees progress (`done` included). Unassigned tasks
/// are not delegated.
pub(crate) fn filter_delegated_tasks(tasks: &[RoomTask], self_user_id: &str) -> Vec<RoomTask> {
    tasks
        .iter()
        .filter(|t| {
            t.task.creator_user_id == self_user_id
                && t.task
                    .item
                    .assignee
                    .as_deref()
                    .is_some_and(|assignee| assignee != self_user_id)
        })
        .cloned()
        .collect()
}

/// Project one folded task into its materialized `tasks`-table row.
/// Step 60c.
pub(crate) fn task_record_from_folded(room_id: &str, f: &FoldedTask) -> TaskRecord {
    TaskRecord {
        room_id: room_id.to_string(),
        task_id: f.item.task_id.clone(),
        title: f.item.title.clone(),
        description: f.item.description.clone(),
        assignee: f.item.assignee.clone(),
        due_at_ms: f.item.due_at_ms,
        status: status_as_str(f.item.status).to_string(),
        priority: f.item.priority.map(|p| priority_as_str(p).to_string()),
        sort_key: f.item.sort_key.clone(),
        creator_user_id: f.creator_user_id.clone(),
        created_at_ms: f.created_at_ms,
        updated_at_ms: f.updated_at_ms,
        latest_message_id: f.latest_message_id.clone(),
    }
}

/// Rehydrate a materialized row into the cross-room view element.
/// Unknown `status` strings degrade to `Open`, unknown `priority`
/// strings to `None` — the same tolerance as the JSON body parse.
/// Step 60c.
pub(crate) fn folded_from_task_record(rec: TaskRecord) -> RoomTask {
    RoomTask {
        room_id: rec.room_id,
        task: FoldedTask {
            item: TaskItem {
                task_id: rec.task_id,
                title: rec.title,
                description: rec.description,
                assignee: rec.assignee,
                due_at_ms: rec.due_at_ms,
                status: status_from_str(&rec.status),
                priority: rec.priority.as_deref().and_then(priority_from_str),
                sort_key: rec.sort_key,
            },
            creator_user_id: rec.creator_user_id,
            created_at_ms: rec.created_at_ms,
            updated_at_ms: rec.updated_at_ms,
            latest_message_id: rec.latest_message_id,
        },
    }
}

impl Client {
    /// Cross-room "my open work" view (Step 60c): tasks assigned to
    /// this account with status `Open` or `InProgress`, across every
    /// room in the local replica, in the ADR-0014 ordering.
    ///
    /// # Errors
    /// `InvalidInput` without an attached `Store`; `Storage` from the
    /// SQL read.
    pub async fn my_tasks(&self) -> Result<Vec<RoomTask>, KupferdrahtError> {
        let me = self.own_qualified_user_id();
        let store = self.store_handle().ok_or_else(|| {
            KupferdrahtError::InvalidInput(
                "my_tasks: no Store attached — set ClientBuilder::base_dir(...) first".into(),
            )
        })?;
        // Index-backed narrowing in SQL; the pure filter stays the
        // semantic authority on top.
        let all: Vec<RoomTask> = store
            .load_open_tasks_for_assignee(&me)
            .await?
            .into_iter()
            .map(folded_from_task_record)
            .collect();
        let mut mine = filter_my_tasks(&all, &me);
        sort_room_tasks(&mut mine);
        Ok(mine)
    }

    /// Cross-room "what I delegated" view (Step 60c): tasks this
    /// account created and assigned to someone else — any status, so
    /// progress (`done`) stays visible.
    ///
    /// # Errors
    /// Same as [`Self::my_tasks`].
    pub async fn delegated_tasks(&self) -> Result<Vec<RoomTask>, KupferdrahtError> {
        let me = self.own_qualified_user_id();
        let store = self.store_handle().ok_or_else(|| {
            KupferdrahtError::InvalidInput(
                "delegated_tasks: no Store attached — set ClientBuilder::base_dir(...) first"
                    .into(),
            )
        })?;
        let all: Vec<RoomTask> = store
            .load_tasks_for_creator(&me)
            .await?
            .into_iter()
            .map(folded_from_task_record)
            .collect();
        let mut delegated = filter_delegated_tasks(&all, &me);
        sort_room_tasks(&mut delegated);
        Ok(delegated)
    }

    /// Folded task list of one room (Step 60b): one entry per
    /// `task_id`, latest full state wins in arrival order, sorted by
    /// the ADR-0014 default (due, then priority, then first-seen).
    pub(crate) async fn room_tasks(
        &self,
        room_id: &str,
    ) -> Result<Vec<FoldedTask>, KupferdrahtError> {
        debug_assert!(!room_id.is_empty(), "room_tasks: room_id must be non-empty");
        let store = self.store_handle().ok_or_else(|| {
            KupferdrahtError::InvalidInput(
                "room_tasks: no Store attached — set ClientBuilder::base_dir(...) first".into(),
            )
        })?;
        // Step 60c: read the materialized fold cache instead of
        // re-folding timeline_events on every call.
        let mut folded: Vec<FoldedTask> = store
            .load_tasks_for_room(room_id)
            .await?
            .into_iter()
            .map(|rec| folded_from_task_record(rec).task)
            .collect();
        sort_folded_tasks(&mut folded);
        Ok(folded)
    }
}
