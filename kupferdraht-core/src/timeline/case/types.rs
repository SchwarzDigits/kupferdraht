// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Case value types (ADR-0018, Step 72a): the public snapshot shape.
//! Pure data — codec and (from 72b) fold logic live in sibling
//! modules.

use std::collections::HashMap;

/// Lifecycle state of a case. Unknown wire values decode as `Open`
/// (the snapshot stays visible; the next one heals it — ADR-0014
/// precedent carried into ADR-0018).
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum CaseState {
    Open,
    Active,
    Waiting,
    Resolved,
    Closed,
    Cancelled,
}

/// Case priority; absent = not set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum CasePriority {
    Low,
    Normal,
    High,
    Urgent,
}

/// Full snapshot of one case as carried by every `CasePayload`
/// message and as folded locally (ADR-0018 §Decision 4). Every
/// update carries the complete state; `revision` is the fold key.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct CaseSnapshot {
    /// Monotonic per (room, `case_ref`).
    pub revision: u64,
    /// Stable per-room case identity (not the message id).
    pub case_ref: String,
    /// File reference / id in the leading external system.
    pub external_ref: Option<String>,
    pub title: Option<String>,
    /// Deployment-defined classification.
    pub case_type: Option<String>,
    pub state: CaseState,
    pub state_detail: Option<String>,
    /// Qualified user id (`<uuid>@<domain>`).
    pub assignee: Option<String>,
    pub due_at_ms: Option<i64>,
    pub priority: Option<CasePriority>,
    pub opened_at_ms: Option<i64>,
    pub closed_at_ms: Option<i64>,
    /// Tenant or public authority — deployment-defined identifier.
    pub authority_ref: Option<String>,
    /// Display-only extras — no semantics, no authorization weight.
    /// (`HashMap` for the `uniffi::Record` mirror; entry order never
    /// carries meaning — the fold keys off `revision`, not bytes.)
    pub extensions: HashMap<String, String>,
}

/// Set-only patch for [`CaseSnapshot`] — the input to
/// `Room::set_case` (Step 72c). `None` keeps the current value; the
/// library bumps `revision` and sends the resulting **full**
/// snapshot (ADR-0018 — create and update are the same message).
/// `case_ref` is honored only when no case exists yet (create);
/// `extensions` replaces wholesale when `Some` (one display-only
/// unit, the fold's rule).
#[derive(Debug, Clone, Default, PartialEq, Eq, uniffi::Record)]
pub struct CasePatch {
    pub case_ref: Option<String>,
    pub external_ref: Option<String>,
    pub title: Option<String>,
    pub case_type: Option<String>,
    pub state: Option<CaseState>,
    pub state_detail: Option<String>,
    pub assignee: Option<String>,
    pub due_at_ms: Option<i64>,
    pub priority: Option<CasePriority>,
    pub opened_at_ms: Option<i64>,
    pub closed_at_ms: Option<i64>,
    pub authority_ref: Option<String>,
    pub extensions: Option<HashMap<String, String>>,
}
