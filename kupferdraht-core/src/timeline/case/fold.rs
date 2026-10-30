// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Case fold rules (Step 72b, ADR-0018): last-writer-wins by
//! `revision` for the admin-gated core, arrival-ordered display-only
//! `extensions` from any member.

use super::CaseSnapshot;

/// Apply one inbound case event onto the current folded state
/// (arrival order = the caller's iteration order — the local proxy
/// for the event stream's server order, ADR-0014/0018).
///
/// Rules (ADR-0018 §Decision):
/// * `sender_was_admin` (captured at receive time): the snapshot
///   replaces the fold when `incoming.revision >=` the current one
///   (equal → later arrival wins); an older revision is ignored.
///   No current state → the snapshot creates it.
/// * non-admin: the core never changes. Only `extensions` replace
///   (wholesale — they are one display-only unit), and only when a
///   folded case already exists; `revision` carries no weight on
///   this track. Without current state there is nothing to attach
///   display extras to — the event contributes nothing.
pub(crate) fn apply_case_event(
    current: Option<CaseSnapshot>,
    incoming: &CaseSnapshot,
    sender_was_admin: bool,
) -> Option<CaseSnapshot> {
    if sender_was_admin {
        return match current {
            Some(state) if incoming.revision < state.revision => Some(state),
            _ => Some(incoming.clone()),
        };
    }
    // Non-admin: display-only extensions replace wholesale on an
    // existing case; the core (incl. revision) never moves.
    current.map(|mut state| {
        state.extensions.clone_from(&incoming.extensions);
        state
    })
}

#[cfg(test)]
mod tests {
    use super::super::{CasePriority, CaseState};
    use super::*;

    fn snap(revision: u64, title: &str) -> CaseSnapshot {
        CaseSnapshot {
            revision,
            case_ref: "case-1".into(),
            external_ref: None,
            title: Some(title.into()),
            case_type: None,
            state: CaseState::Open,
            state_detail: None,
            assignee: None,
            due_at_ms: None,
            priority: Some(CasePriority::Normal),
            opened_at_ms: None,
            closed_at_ms: None,
            authority_ref: None,
            extensions: std::collections::HashMap::new(),
        }
    }

    #[test]
    fn admin_snapshot_creates_and_replaces_by_revision() {
        let first = apply_case_event(None, &snap(1, "one"), true);
        assert_eq!(first.as_ref().and_then(|s| s.title.as_deref()), Some("one"));
        // Higher revision replaces.
        let second = apply_case_event(first.clone(), &snap(3, "three"), true);
        assert_eq!(
            second.as_ref().and_then(|s| s.title.as_deref()),
            Some("three")
        );
        // Older revision is ignored.
        let third = apply_case_event(second.clone(), &snap(2, "two"), true);
        assert_eq!(third, second, "an older admin revision never regresses");
        // Equal revision: later arrival wins (server-order tie-break).
        let fourth = apply_case_event(third, &snap(3, "three-b"), true);
        assert_eq!(
            fourth.and_then(|s| s.title),
            Some("three-b".to_string()),
            "equal revision → the later arrival replaces"
        );
    }

    #[test]
    fn non_admin_touches_extensions_only_and_needs_existing_state() {
        // No folded case yet → a non-admin event contributes nothing.
        assert_eq!(apply_case_event(None, &snap(1, "one"), false), None);

        let mut with_ext = snap(5, "five");
        with_ext
            .extensions
            .insert("stale".into(), "by the admin".into());
        let current = apply_case_event(None, &with_ext, true);
        // Non-admin snapshot: lower revision, different core, one
        // extension — only the extensions may land.
        let mut incoming = snap(1, "hijack");
        incoming.state = CaseState::Closed;
        incoming
            .extensions
            .insert("note".into(), "resolved on site".into());
        let folded =
            apply_case_event(current, &incoming, false).expect("state survives a non-admin event");
        assert_eq!(
            folded.title.as_deref(),
            Some("five"),
            "core fields never fold from non-admins"
        );
        assert_eq!(folded.state, CaseState::Open, "state is core");
        assert_eq!(folded.revision, 5, "revision is core");
        assert_eq!(
            folded.extensions.get("note").map(String::as_str),
            Some("resolved on site"),
            "extensions fold from any member (ADR-0018 §Decision 3)"
        );
        assert_eq!(
            folded.extensions.get("stale"),
            None,
            "extensions replace WHOLESALE - one display-only unit, not a \
             per-key merge (a merge would leak deleted extras forever)"
        );
    }
}
