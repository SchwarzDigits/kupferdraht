// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! The `timeline_events.body` JSON codec for case rows (Step 72b;
//! split out of `encode.rs` in 72c). The body doubles as
//! small-payload storage (task/`Location` precedent) and carries the
//! receive-time `sender_was_admin` flag next to the snapshot.

use super::{CasePriority, CaseSnapshot, CaseState};

/// JSON shape packed into `timeline_events.body` for a case row —
/// the column doubles as small-payload storage (the task/`Location`
/// precedent). `state`/`priority` travel as strings so a newer
/// Kupferdraht's values stay readable; `sender_was_admin` captures the
/// Step-72a metadata role at receive time (ADR-0018 receive-time
/// authorization — the read-fold must not re-evaluate history with
/// today's roles).
#[derive(serde::Serialize, serde::Deserialize)]
struct CaseBodyPayload {
    revision: u64,
    case_ref: String,
    #[serde(default)]
    external_ref: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    case_type: Option<String>,
    state: String,
    #[serde(default)]
    state_detail: Option<String>,
    #[serde(default)]
    assignee: Option<String>,
    #[serde(default)]
    due_at_ms: Option<i64>,
    #[serde(default)]
    priority: Option<String>,
    #[serde(default)]
    opened_at_ms: Option<i64>,
    #[serde(default)]
    closed_at_ms: Option<i64>,
    #[serde(default)]
    authority_ref: Option<String>,
    #[serde(default)]
    extensions: std::collections::HashMap<String, String>,
    #[serde(default)]
    sender_was_admin: bool,
}

fn state_to_str(state: CaseState) -> &'static str {
    match state {
        CaseState::Open => "open",
        CaseState::Active => "active",
        CaseState::Waiting => "waiting",
        CaseState::Resolved => "resolved",
        CaseState::Closed => "closed",
        CaseState::Cancelled => "cancelled",
    }
}

fn state_from_str(raw: &str) -> CaseState {
    match raw {
        "active" => CaseState::Active,
        "waiting" => CaseState::Waiting,
        "resolved" => CaseState::Resolved,
        "closed" => CaseState::Closed,
        "cancelled" => CaseState::Cancelled,
        // Unknown strings (a newer Kupferdraht) degrade to Open, the
        // stays-visible default (ADR-0018 / ADR-0014).
        _ => CaseState::Open,
    }
}

fn priority_to_str(priority: CasePriority) -> &'static str {
    match priority {
        CasePriority::Low => "low",
        CasePriority::Normal => "normal",
        CasePriority::High => "high",
        CasePriority::Urgent => "urgent",
    }
}

fn priority_from_str(raw: &str) -> Option<CasePriority> {
    match raw {
        "low" => Some(CasePriority::Low),
        "normal" => Some(CasePriority::Normal),
        "high" => Some(CasePriority::High),
        "urgent" => Some(CasePriority::Urgent),
        _ => None,
    }
}

/// Pack a snapshot + the receive-time authorization flag into the
/// `timeline_events.body` JSON.
pub(crate) fn case_body(snapshot: &CaseSnapshot, sender_was_admin: bool) -> String {
    let payload = CaseBodyPayload {
        revision: snapshot.revision,
        case_ref: snapshot.case_ref.clone(),
        external_ref: snapshot.external_ref.clone(),
        title: snapshot.title.clone(),
        case_type: snapshot.case_type.clone(),
        state: state_to_str(snapshot.state).to_string(),
        state_detail: snapshot.state_detail.clone(),
        assignee: snapshot.assignee.clone(),
        due_at_ms: snapshot.due_at_ms,
        priority: snapshot.priority.map(|p| priority_to_str(p).to_string()),
        opened_at_ms: snapshot.opened_at_ms,
        closed_at_ms: snapshot.closed_at_ms,
        authority_ref: snapshot.authority_ref.clone(),
        extensions: snapshot.extensions.clone(),
        sender_was_admin,
    };
    serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_string())
}

/// Parse a persisted case body back into the snapshot + the
/// receive-time authorization flag. `None` on malformed JSON — the
/// row then degrades to Text in the timeline and contributes
/// nothing to the fold.
pub(crate) fn parse_case_body(body: &str) -> Option<(CaseSnapshot, bool)> {
    let p: CaseBodyPayload = serde_json::from_str(body).ok()?;
    let snapshot = CaseSnapshot {
        revision: p.revision,
        case_ref: p.case_ref,
        external_ref: p.external_ref,
        title: p.title,
        case_type: p.case_type,
        state: state_from_str(&p.state),
        state_detail: p.state_detail,
        assignee: p.assignee,
        due_at_ms: p.due_at_ms,
        priority: p.priority.as_deref().and_then(priority_from_str),
        opened_at_ms: p.opened_at_ms,
        closed_at_ms: p.closed_at_ms,
        authority_ref: p.authority_ref,
        extensions: p.extensions,
    };
    Some((snapshot, p.sender_was_admin))
}

/// `timeline_events.kind == "case"` rows viewed as timeline items.
pub(crate) fn case_kind_from_body(body: &str) -> Option<super::super::TimelineItemKind> {
    parse_case_body(body)
        .map(|(snapshot, _)| super::super::TimelineItemKind::Case(Box::new(snapshot)))
}
