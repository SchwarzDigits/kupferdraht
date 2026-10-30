// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Case ↔ protobuf `CasePayload` conversions (Step 72a; consumed by
//! the 72b dispatch and the 72c send path). The persisted-body JSON
//! codec lives in the sibling `body` module (split in 72c per the
//! 72b review — this file had crossed 200 lines).

use super::{CasePriority, CaseSnapshot, CaseState};
use crate::wire::proto;

/// Map a [`CaseSnapshot`] onto the wire-layer `CasePayload` protobuf.
/// prost encodes map entries in the backing `HashMap`'s iteration
/// order — acceptable: snapshot equality and the LWW fold key off
/// `revision`, never off byte comparison, so non-deterministic
/// `extensions` bytes carry no semantics.
pub(crate) fn payload_from_snapshot(snapshot: &CaseSnapshot) -> proto::CasePayload {
    let state = match snapshot.state {
        CaseState::Open => proto::CaseState::Open,
        CaseState::Active => proto::CaseState::Active,
        CaseState::Waiting => proto::CaseState::Waiting,
        CaseState::Resolved => proto::CaseState::Resolved,
        CaseState::Closed => proto::CaseState::Closed,
        CaseState::Cancelled => proto::CaseState::Cancelled,
    };
    let priority = snapshot.priority.map(|p| match p {
        CasePriority::Low => proto::CasePriority::Low as i32,
        CasePriority::Normal => proto::CasePriority::Normal as i32,
        CasePriority::High => proto::CasePriority::High as i32,
        CasePriority::Urgent => proto::CasePriority::Urgent as i32,
    });
    proto::CasePayload {
        revision: snapshot.revision,
        case_ref: snapshot.case_ref.clone(),
        external_ref: snapshot.external_ref.clone(),
        title: snapshot.title.clone(),
        case_type: snapshot.case_type.clone(),
        state: state as i32,
        state_detail: snapshot.state_detail.clone(),
        assignee: snapshot.assignee.as_deref().map(|qid| {
            let (id, domain) = qid.split_once('@').unwrap_or((qid, ""));
            proto::QualifiedUserId {
                id: id.to_string(),
                domain: domain.to_string(),
            }
        }),
        due_at_ms: snapshot.due_at_ms,
        priority,
        opened_at_ms: snapshot.opened_at_ms,
        closed_at_ms: snapshot.closed_at_ms,
        authority_ref: snapshot.authority_ref.clone(),
        extensions: snapshot.extensions.clone().into_iter().collect(),
    }
}

/// Map an inbound wire-layer `CasePayload` onto the snapshot shape.
/// Unknown `state` wire values decode as `Open` (prost accessor
/// default); unknown `priority` values degrade to `None` (ADR-0018 /
/// ADR-0014 precedent — the snapshot stays visible, the next one
/// heals it).
pub(crate) fn snapshot_from_payload(payload: &proto::CasePayload) -> CaseSnapshot {
    let state = match payload.state() {
        proto::CaseState::Open => CaseState::Open,
        proto::CaseState::Active => CaseState::Active,
        proto::CaseState::Waiting => CaseState::Waiting,
        proto::CaseState::Resolved => CaseState::Resolved,
        proto::CaseState::Closed => CaseState::Closed,
        proto::CaseState::Cancelled => CaseState::Cancelled,
    };
    let priority = payload
        .priority
        .and_then(|raw| proto::CasePriority::try_from(raw).ok())
        .map(|p| match p {
            proto::CasePriority::Low => CasePriority::Low,
            proto::CasePriority::Normal => CasePriority::Normal,
            proto::CasePriority::High => CasePriority::High,
            proto::CasePriority::Urgent => CasePriority::Urgent,
        });
    CaseSnapshot {
        revision: payload.revision,
        case_ref: payload.case_ref.clone(),
        external_ref: payload.external_ref.clone(),
        title: payload.title.clone(),
        case_type: payload.case_type.clone(),
        state,
        state_detail: payload.state_detail.clone(),
        assignee: payload.assignee.as_ref().map(|q| {
            if q.domain.is_empty() {
                q.id.clone()
            } else {
                format!("{}@{}", q.id, q.domain)
            }
        }),
        due_at_ms: payload.due_at_ms,
        priority,
        opened_at_ms: payload.opened_at_ms,
        closed_at_ms: payload.closed_at_ms,
        authority_ref: payload.authority_ref.clone(),
        extensions: payload.extensions.clone().into_iter().collect(),
    }
}
