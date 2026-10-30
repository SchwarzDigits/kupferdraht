// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `kupferdraht-cli` task command handlers (Step 60a, ADR-0014).

use kupferdraht::timeline::{
    FoldedTask, RoomTask, TaskDraft, TaskItem, TaskPatch, TaskPriority, TaskStatus,
};
use kupferdraht::{KupferdrahtError, SyncSettings};

use super::{fmt_ms, resume_client, resume_with_room};

/// Clap-facing priority argument; mapped onto the library enum at the
/// dispatch boundary so the core stays clap-free.
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub(crate) enum PriorityArg {
    Low,
    Medium,
    High,
    Urgent,
}

impl PriorityArg {
    pub(crate) fn into_core(self) -> TaskPriority {
        match self {
            Self::Low => TaskPriority::Low,
            Self::Medium => TaskPriority::Medium,
            Self::High => TaskPriority::High,
            Self::Urgent => TaskPriority::Urgent,
        }
    }
}

/// Clap-facing status argument for `task edit --status`. Step 60b.
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub(crate) enum StatusArg {
    Open,
    InProgress,
    Done,
    Cancelled,
}

impl StatusArg {
    pub(crate) fn into_core(self) -> TaskStatus {
        match self {
            Self::Open => TaskStatus::Open,
            Self::InProgress => TaskStatus::InProgress,
            Self::Done => TaskStatus::Done,
            Self::Cancelled => TaskStatus::Cancelled,
        }
    }
}

/// Human-readable status label for CLI output.
pub(crate) fn status_label(item: &TaskItem) -> &'static str {
    match item.status {
        kupferdraht::timeline::TaskStatus::Open => "open",
        kupferdraht::timeline::TaskStatus::InProgress => "in_progress",
        kupferdraht::timeline::TaskStatus::Done => "done",
        kupferdraht::timeline::TaskStatus::Cancelled => "cancelled",
    }
}

/// Human-readable priority label for CLI output.
pub(crate) fn priority_label(priority: TaskPriority) -> &'static str {
    match priority {
        TaskPriority::Low => "low",
        TaskPriority::Medium => "medium",
        TaskPriority::High => "high",
        TaskPriority::Urgent => "urgent",
    }
}

/// One-line task rendering shared by `task list` and the timeline view.
pub(crate) fn task_preview(item: &TaskItem) -> String {
    let checkbox = match item.status {
        kupferdraht::timeline::TaskStatus::Done => "\u{2611}",
        kupferdraht::timeline::TaskStatus::Cancelled => "\u{2612}",
        _ => "\u{2610}",
    };
    format!(
        "{checkbox} \"{}\" [{}]{}{}{}",
        item.title,
        status_label(item),
        item.assignee
            .as_deref()
            .map(|a| format!(" @{a}"))
            .unwrap_or_default(),
        item.due_at_ms
            .map(|d| format!(" due={}", fmt_ms(d)))
            .unwrap_or_default(),
        item.priority
            .map(|p| format!(" prio={}", priority_label(p)))
            .unwrap_or_default(),
    )
}

/// Map the CLI's `--fallback` / `--no-fallback` pair onto the core
/// tri-state (Step 75, ADR-0020): either flag is an explicit choice,
/// neither means "derive from the room's task-list marker".
pub(crate) fn fallback_mode(fallback: bool, no_fallback: bool) -> Option<bool> {
    match (fallback, no_fallback) {
        (true, _) => Some(true),
        (_, true) => Some(false),
        (false, false) => None,
    }
}

fn fallback_label(with_fallback: Option<bool>) -> &'static str {
    match with_fallback {
        Some(true) => "on (mode c)",
        Some(false) => "off (mode a)",
        None => "auto (from the room's task-list marker)",
    }
}

/// Step 60a — create and send a new task (status starts at `open`).
pub(crate) async fn add(
    room_id: String,
    draft: TaskDraft,
    with_fallback: Option<bool>,
) -> Result<(), KupferdrahtError> {
    let (_client, room, room_id) = resume_with_room(room_id).await?;
    let timeline = room.timeline()?;
    let title = draft.title.clone();
    let assignee = draft.assignee.clone();
    let due_ms = draft.due_at_ms;
    let priority = draft.priority;
    let handle = timeline.send_task(draft, with_fallback).await?;
    println!("task add ok");
    println!("  room_id:    {room_id}");
    println!("  title:      {title}");
    if let Some(assignee) = assignee {
        println!("  assignee:   {assignee}");
    }
    if let Some(due_ms) = due_ms {
        println!("  due:        {}", fmt_ms(due_ms));
    }
    if let Some(priority) = priority {
        println!("  priority:   {}", priority_label(priority));
    }
    println!("  fallback:   {}", fallback_label(with_fallback));
    println!("  message_id: {}", handle.message_id);
    println!("  sent_at:    {}", fmt_ms(handle.sent_at_ms));
    Ok(())
}

/// Step 60b — the folded task list of a room: one entry per `task_id`,
/// latest full state wins (LWW in arrival order), sorted by due date,
/// then priority, then first-seen. Drains pending notifications first
/// (via `resume_with_room`'s sync) so fresh updates are folded in.
pub(crate) async fn list(room_id: String) -> Result<(), KupferdrahtError> {
    let (_client, room, room_id) = resume_with_room(room_id).await?;
    let tasks: Vec<FoldedTask> = room.tasks().await?;
    println!("task list ok");
    println!("  room_id: {room_id}");
    if room.is_task_list().await? {
        println!("  marker:  dedicated task list (ADR-0020)");
    }
    println!("  tasks:   {}", tasks.len());
    for f in &tasks {
        println!(
            "  {}  (task_id={}, updated={}, by={})",
            task_preview(&f.item),
            f.item.task_id,
            fmt_ms(f.updated_at_ms),
            f.creator_user_id,
        );
    }
    Ok(())
}

/// Step 75 — set or clear the room's task-list marker (ADR-0020):
/// any member may flip it; official clients see nothing.
pub(crate) async fn mark(room_id: String, clear: bool) -> Result<(), KupferdrahtError> {
    let (_client, room, room_id) = resume_with_room(room_id).await?;
    room.set_task_list(!clear).await?;
    println!("task mark ok");
    println!("  room_id:   {room_id}");
    println!("  task_list: {}", if clear { "cleared" } else { "marked" });
    Ok(())
}

/// Render one cross-room view entry (`task list --mine/--delegated`).
fn print_room_task(t: &RoomTask) {
    println!(
        "  {}  (room={}, task_id={}, updated={})",
        task_preview(&t.task.item),
        t.room_id,
        t.task.item.task_id,
        fmt_ms(t.task.updated_at_ms),
    );
}

/// Step 60c — cross-room "my open work": tasks assigned to this
/// account with status open / in-progress, across every room.
pub(crate) async fn list_mine() -> Result<(), KupferdrahtError> {
    let client = resume_client().await?;
    client.sync_once(SyncSettings::default()).await?;
    let tasks = client.my_tasks().await?;
    println!("task list --mine ok");
    println!("  tasks: {}", tasks.len());
    for t in &tasks {
        print_room_task(t);
    }
    Ok(())
}

/// Step 60c — cross-room "what I delegated": tasks this account
/// created and assigned to someone else, any status.
pub(crate) async fn list_delegated() -> Result<(), KupferdrahtError> {
    let client = resume_client().await?;
    client.sync_once(SyncSettings::default()).await?;
    let tasks = client.delegated_tasks().await?;
    println!("task list --delegated ok");
    println!("  tasks: {}", tasks.len());
    for t in &tasks {
        print_room_task(t);
    }
    Ok(())
}

/// Step 60c — move a task in the room's manual order: a fractional
/// `sort_key` between the `--after`/`--before` neighbors goes out as
/// an ordinary full-state update.
pub(crate) async fn move_task(
    room_id: String,
    task_id: String,
    after: Option<String>,
    before: Option<String>,
    with_fallback: Option<bool>,
) -> Result<(), KupferdrahtError> {
    let (_client, room, room_id) = resume_with_room(room_id).await?;
    let timeline = room.timeline()?;
    let handle = timeline
        .move_task(&task_id, after, before, with_fallback)
        .await?;
    println!("task move ok");
    println!("  room_id:    {room_id}");
    println!("  task_id:    {task_id}");
    println!("  message_id: {}", handle.message_id);
    println!("  sent_at:    {}", fmt_ms(handle.sent_at_ms));
    Ok(())
}

/// Shared Step 60b update handler: apply `patch` onto the current
/// folded state and send the resulting full state.
async fn send_update(
    verb: &str,
    room_id: String,
    task_id: String,
    patch: TaskPatch,
    with_fallback: Option<bool>,
) -> Result<(), KupferdrahtError> {
    let (_client, room, room_id) = resume_with_room(room_id).await?;
    let timeline = room.timeline()?;
    let handle = timeline.update_task(&task_id, patch, with_fallback).await?;
    println!("task {verb} ok");
    println!("  room_id:    {room_id}");
    println!("  task_id:    {task_id}");
    println!("  message_id: {}", handle.message_id);
    println!("  sent_at:    {}", fmt_ms(handle.sent_at_ms));
    Ok(())
}

/// Step 60b — mark a task done (`status = done`).
pub(crate) async fn done(
    room_id: String,
    task_id: String,
    with_fallback: Option<bool>,
) -> Result<(), KupferdrahtError> {
    let patch = TaskPatch {
        status: Some(TaskStatus::Done),
        ..TaskPatch::default()
    };
    send_update("done", room_id, task_id, patch, with_fallback).await
}

/// Step 60b — re-assign a task to another member.
pub(crate) async fn assign(
    room_id: String,
    task_id: String,
    user: String,
    with_fallback: Option<bool>,
) -> Result<(), KupferdrahtError> {
    let patch = TaskPatch {
        assignee: Some(user),
        ..TaskPatch::default()
    };
    send_update("assign", room_id, task_id, patch, with_fallback).await
}

/// Step 60b — edit any fields of a task via a set-only patch.
pub(crate) async fn edit(
    room_id: String,
    task_id: String,
    patch: TaskPatch,
    with_fallback: Option<bool>,
) -> Result<(), KupferdrahtError> {
    if patch == TaskPatch::default() {
        return Err(KupferdrahtError::InvalidInput(
            "task edit: pass at least one field to change".into(),
        ));
    }
    send_update("edit", room_id, task_id, patch, with_fallback).await
}
