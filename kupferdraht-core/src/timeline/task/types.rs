// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Task value types (ADR-0014): drafts, items, patches, folded
//! views. Pure data — logic lives in the sibling modules.

/// Consumer-facing input for a new task. The library generates the
/// stable `task_id` (UUID) and stamps `status: Open` — a fresh task
/// is always open; state changes travel as later full-state updates
/// (Step 60b).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskDraft {
    pub title: String,
    /// Free text; links are plain text inside it (ADR-0014).
    pub description: Option<String>,
    /// Qualified user id (`<uuid>@<domain>`) of the assignee.
    pub assignee: Option<String>,
    pub due_at_ms: Option<i64>,
    pub priority: Option<TaskPriority>,
}

/// Task lifecycle state. Unknown wire values decode as `Open` — the
/// task stays visible and the next full-state update heals it
/// (ADR-0014).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStatus {
    Open,
    InProgress,
    Done,
    Cancelled,
}

/// Optional task priority; absent = no priority.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskPriority {
    Low,
    Medium,
    High,
    Urgent,
}

/// The folded, consumer-facing state of one task — carried by
/// [`TimelineItemKind::Task`] (boxed to keep the enum stack-compact
/// next to the cheap `Text` variant, like `Asset`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskItem {
    /// Stable identity (UUID) — deliberately NOT the `message_id`.
    pub task_id: String,
    pub title: String,
    pub description: Option<String>,
    /// Qualified user id (`<uuid>@<domain>`).
    pub assignee: Option<String>,
    pub due_at_ms: Option<i64>,
    pub status: TaskStatus,
    pub priority: Option<TaskPriority>,
    /// Lexicographic fractional index — consumed in Step 60c.
    pub sort_key: Option<String>,
}

/// A partial change to one task (Step 60b). Every `Some` field is
/// applied onto the current folded state; `None` keeps the current
/// value. The patch is set-only — *clearing* an optional field (e.g. removing
/// a due date) is not expressible through the patch (a nested
/// `Option<Option<T>>` does not cross the FFI boundary); the wire
/// format supports it, so a clearing API can follow without a schema
/// change.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskPatch {
    pub title: Option<String>,
    pub description: Option<String>,
    /// Qualified user id (`<uuid>@<domain>`).
    pub assignee: Option<String>,
    pub due_at_ms: Option<i64>,
    pub priority: Option<TaskPriority>,
    /// Any transition is legal — `Done → Open` (reopen) is just
    /// another full-state update (ADR-0014).
    pub status: Option<TaskStatus>,
    pub sort_key: Option<String>,
}

/// One task after folding its event log (Step 60b, ADR-0014): the
/// latest full state (LWW in arrival order) plus the fold metadata
/// the cross-room views need (creator for `delegated_tasks`,
/// timestamps for the default ordering).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoldedTask {
    pub item: TaskItem,
    /// Sender of the task's first message (qualified user id).
    pub creator_user_id: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    /// `message_id` of the message that carried the current state.
    pub latest_message_id: String,
}

/// One task with the room it lives in — the cross-room view element
/// (Step 60c): `Client::my_tasks()` / `Client::delegated_tasks()`
/// aggregate over every conversation the client replicates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoomTask {
    /// Qualified room id (`<uuid>@<domain>`).
    pub room_id: String,
    pub task: FoldedTask,
}
