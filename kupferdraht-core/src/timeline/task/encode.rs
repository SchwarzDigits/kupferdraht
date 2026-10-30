// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Task serialization boundaries (Step 60a): the mode-c fallback
//! text, the JSON `timeline_events.body` codec, and the protobuf
//! `TaskPayload` conversions.
use super::{MentionInput, TaskItem, TaskPriority, TaskStatus, TimelineItemKind};

/// Render the mode-c fallback text for a task message (ADR-0014):
/// a human-readable line official Wire clients display verbatim,
/// e.g. `☐ Prepare deploy — @anna`. The checkbox reflects the task
/// state (`☑` done, `☒` cancelled, `☐` otherwise — Step 60b) so
/// update messages read as an activity log. When `assignee` is
/// `Some((handle, qualified_user_id))`, the returned mentions carry
/// the `@handle` span in **UTF-16 code units** (Wire's offset
/// convention — the checkbox glyph alone makes byte offsets diverge).
pub(crate) fn task_fallback(
    title: &str,
    status: TaskStatus,
    assignee: Option<(&str, &str)>,
) -> (String, Vec<MentionInput>) {
    debug_assert!(!title.is_empty(), "task_fallback: title must be non-empty");
    let checkbox = match status {
        TaskStatus::Done => "\u{2611}",
        TaskStatus::Cancelled => "\u{2612}",
        TaskStatus::Open | TaskStatus::InProgress => "\u{2610}",
    };
    let mut text = format!("{checkbox} {title}");
    let mut mentions = Vec::new();
    if let Some((handle, qualified_user_id)) = assignee {
        let mention_text = if handle.starts_with('@') {
            handle.to_string()
        } else {
            format!("@{handle}")
        };
        text.push_str(" \u{2014} ");
        // Offsets are UTF-16 code units (Wire's convention) — the
        // leading `☐` makes them diverge from byte offsets.
        let start = i32::try_from(text.encode_utf16().count()).unwrap_or(i32::MAX);
        let length = i32::try_from(mention_text.encode_utf16().count()).unwrap_or(i32::MAX);
        text.push_str(&mention_text);
        mentions.push(MentionInput {
            start,
            length,
            qualified_user_id: qualified_user_id.to_string(),
        });
    }
    (text, mentions)
}

/// JSON shape packed into `timeline_events.body` for a task item —
/// the column doubles as small-payload storage (the `Location`
/// precedent). `status`/`priority` travel as strings so a newer
/// Kupferdraht's values stay readable; optionals default so older rows
/// without later fields still parse.
#[derive(serde::Serialize, serde::Deserialize)]
struct TaskBodyPayload {
    task_id: String,
    title: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    assignee: Option<String>,
    #[serde(default)]
    due_at_ms: Option<i64>,
    status: String,
    #[serde(default)]
    priority: Option<String>,
    #[serde(default)]
    sort_key: Option<String>,
}

pub(crate) fn status_as_str(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Open => "open",
        TaskStatus::InProgress => "in_progress",
        TaskStatus::Done => "done",
        TaskStatus::Cancelled => "cancelled",
    }
}

/// Unknown strings map to `Open` — the task stays visible and the
/// next full-state update heals it (ADR-0014).
pub(crate) fn status_from_str(raw: &str) -> TaskStatus {
    match raw {
        "in_progress" => TaskStatus::InProgress,
        "done" => TaskStatus::Done,
        "cancelled" => TaskStatus::Cancelled,
        _ => TaskStatus::Open,
    }
}

pub(crate) fn priority_as_str(priority: TaskPriority) -> &'static str {
    match priority {
        TaskPriority::Low => "low",
        TaskPriority::Medium => "medium",
        TaskPriority::High => "high",
        TaskPriority::Urgent => "urgent",
    }
}

/// Unknown strings map to `None` — priority is optional, so an
/// unknown value degrades to "no priority" rather than a wrong one.
pub(crate) fn priority_from_str(raw: &str) -> Option<TaskPriority> {
    match raw {
        "low" => Some(TaskPriority::Low),
        "medium" => Some(TaskPriority::Medium),
        "high" => Some(TaskPriority::High),
        "urgent" => Some(TaskPriority::Urgent),
        _ => None,
    }
}

/// Serialize a task into its `timeline_events.body` JSON form
/// (kind `task`). Symmetric with [`task_kind_from_body`].
pub(crate) fn task_body(item: &TaskItem) -> String {
    serde_json::to_string(&TaskBodyPayload {
        task_id: item.task_id.clone(),
        title: item.title.clone(),
        description: item.description.clone(),
        assignee: item.assignee.clone(),
        due_at_ms: item.due_at_ms,
        status: status_as_str(item.status).to_string(),
        priority: item.priority.map(|p| priority_as_str(p).to_string()),
        sort_key: item.sort_key.clone(),
    })
    .unwrap_or_default()
}

/// Parse a packed task body back into [`TimelineItemKind::Task`];
/// `None` for malformed JSON (caller degrades to a Text fallback,
/// the `Location` precedent). An unknown `status` string maps to
/// `Open` (ADR-0014).
pub(crate) fn task_kind_from_body(body: &str) -> Option<TimelineItemKind> {
    let p: TaskBodyPayload = serde_json::from_str(body).ok()?;
    Some(TimelineItemKind::Task(Box::new(TaskItem {
        task_id: p.task_id,
        title: p.title,
        description: p.description,
        assignee: p.assignee,
        due_at_ms: p.due_at_ms,
        status: status_from_str(&p.status),
        priority: p.priority.as_deref().and_then(priority_from_str),
        sort_key: p.sort_key,
    })))
}

/// Map a [`TaskItem`] onto the wire-layer `TaskPayload` protobuf.
pub(crate) fn payload_from_item(item: &TaskItem) -> crate::wire::proto::TaskPayload {
    use crate::wire::proto;
    let status = match item.status {
        TaskStatus::Open => proto::TaskStatus::Open,
        TaskStatus::InProgress => proto::TaskStatus::InProgress,
        TaskStatus::Done => proto::TaskStatus::Done,
        TaskStatus::Cancelled => proto::TaskStatus::Cancelled,
    };
    let priority = item.priority.map(|p| match p {
        TaskPriority::Low => proto::TaskPriority::Low as i32,
        TaskPriority::Medium => proto::TaskPriority::Medium as i32,
        TaskPriority::High => proto::TaskPriority::High as i32,
        TaskPriority::Urgent => proto::TaskPriority::Urgent as i32,
    });
    proto::TaskPayload {
        task_id: item.task_id.clone(),
        title: item.title.clone(),
        status: status as i32,
        description: item.description.clone(),
        assignee: item.assignee.as_deref().map(|qid| {
            let (id, domain) = qid.split_once('@').unwrap_or((qid, ""));
            proto::QualifiedUserId {
                id: id.to_string(),
                domain: domain.to_string(),
            }
        }),
        due_at_ms: item.due_at_ms,
        priority,
        sort_key: item.sort_key.clone(),
    }
}

/// Map an inbound wire-layer `TaskPayload` onto the folded item shape.
/// Unknown status wire values decode as `Open` (prost accessor
/// default, ADR-0014); unknown priority values degrade to `None`.
pub(crate) fn item_from_payload(payload: &crate::wire::proto::TaskPayload) -> TaskItem {
    use crate::wire::proto;
    let status = match payload.status() {
        proto::TaskStatus::Open => TaskStatus::Open,
        proto::TaskStatus::InProgress => TaskStatus::InProgress,
        proto::TaskStatus::Done => TaskStatus::Done,
        proto::TaskStatus::Cancelled => TaskStatus::Cancelled,
    };
    let priority = payload
        .priority
        .and_then(|raw| proto::TaskPriority::try_from(raw).ok())
        .map(|p| match p {
            proto::TaskPriority::Low => TaskPriority::Low,
            proto::TaskPriority::Medium => TaskPriority::Medium,
            proto::TaskPriority::High => TaskPriority::High,
            proto::TaskPriority::Urgent => TaskPriority::Urgent,
        });
    TaskItem {
        task_id: payload.task_id.clone(),
        title: payload.title.clone(),
        description: payload.description.clone(),
        assignee: payload.assignee.as_ref().map(|q| {
            if q.domain.is_empty() {
                q.id.clone()
            } else {
                format!("{}@{}", q.id, q.domain)
            }
        }),
        due_at_ms: payload.due_at_ms,
        status,
        priority,
        sort_key: payload.sort_key.clone(),
    }
}
