// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Case sends (Step 72c, ADR-0018): patch → full-snapshot send with
//! the client-side admin gate, and the invite re-broadcast tail.
//! Every mutation leaves the device as a complete `CasePayload`
//! without a text fallback (pinned in `wire/proto.rs`).

use crate::client::Client;
use crate::error::KupferdrahtError;
use crate::rooms::RoomMemberRole;

use super::{CasePatch, CaseSnapshot, CaseState, case_body, payload_from_snapshot};

/// The empty snapshot a create starts from — revision 0, the patch's
/// `case_ref` or a generated one.
fn fresh_case(patch: &CasePatch) -> CaseSnapshot {
    CaseSnapshot {
        revision: 0,
        // An empty/whitespace patch ref counts as absent — user input
        // must never reach the builder's non-empty check.
        case_ref: patch
            .case_ref
            .as_deref()
            .map(str::trim)
            .filter(|r| !r.is_empty())
            .map_or_else(|| uuid::Uuid::new_v4().to_string(), str::to_string),
        external_ref: None,
        title: None,
        case_type: None,
        state: CaseState::Open,
        state_detail: None,
        assignee: None,
        due_at_ms: None,
        priority: None,
        opened_at_ms: None,
        closed_at_ms: None,
        authority_ref: None,
        extensions: std::collections::HashMap::new(),
    }
}

/// Apply a set-only [`CasePatch`] onto the current fold, yielding the
/// next full snapshot to send. Create (no current) generates the
/// `case_ref` unless the patch names one and starts at revision 1;
/// an update keeps identity and bumps the revision.
///
/// # Errors
/// `KupferdrahtError::InvalidInput` when the current revision is `u64::MAX`
/// (a peer admin's snapshot can carry it) — there is no next revision
/// every receiver's fold would accept (Step 95c).
fn apply_case_patch(
    current: Option<&CaseSnapshot>,
    patch: &CasePatch,
) -> Result<CaseSnapshot, KupferdrahtError> {
    let base = current.cloned().unwrap_or_else(|| fresh_case(patch));
    let revision = base.revision.checked_add(1).ok_or_else(|| {
        KupferdrahtError::InvalidInput(
            "set_case: the case is at the maximum revision and cannot be updated".into(),
        )
    })?;
    Ok(CaseSnapshot {
        revision,
        case_ref: base.case_ref,
        external_ref: patch.external_ref.clone().or(base.external_ref),
        title: patch.title.clone().or(base.title),
        case_type: patch.case_type.clone().or(base.case_type),
        state: patch.state.unwrap_or(base.state),
        state_detail: patch.state_detail.clone().or(base.state_detail),
        assignee: patch.assignee.clone().or(base.assignee),
        due_at_ms: patch.due_at_ms.or(base.due_at_ms),
        priority: patch.priority.or(base.priority),
        opened_at_ms: patch.opened_at_ms.or(base.opened_at_ms),
        closed_at_ms: patch.closed_at_ms.or(base.closed_at_ms),
        authority_ref: patch.authority_ref.clone().or(base.authority_ref),
        // Wholesale replace when patched — one display-only unit,
        // matching the fold's rule.
        extensions: patch.extensions.clone().unwrap_or(base.extensions),
    })
}

impl Client {
    /// Create or update a room's case (Step 72c) — see
    /// [`crate::rooms::Room::set_case`] for the caller-facing docs.
    ///
    /// # Errors
    /// * `KupferdrahtError::InvalidInput` if the caller is not a
    ///   conversation administrator (client-side mirror of the
    ///   receive-side fold gate), the room is unknown, or the room
    ///   has no local MLS group.
    /// * Encode / MLS / transport / storage errors from the send.
    pub(crate) async fn set_case(
        &self,
        room_id: &str,
        patch: CasePatch,
    ) -> Result<CaseSnapshot, KupferdrahtError> {
        debug_assert!(!room_id.is_empty(), "set_case: room_id must be non-empty");
        let room = self.room(room_id).await.ok_or_else(|| {
            KupferdrahtError::InvalidInput(format!("set_case: {room_id} not in cache"))
        })?;
        let self_is_admin = room
            .members()
            .await?
            .iter()
            .any(|m| m.is_self && m.role == RoomMemberRole::Administrator);
        if !self_is_admin {
            return Err(KupferdrahtError::InvalidInput(
                "set_case: only conversation administrators may change case state \
                 (the receive-side fold would ignore this snapshot)"
                    .into(),
            ));
        }
        let current = self.folded_case(room_id).await?;
        let snapshot = apply_case_patch(current.as_ref(), &patch)?;
        self.send_case_snapshot(room_id, &snapshot).await?;
        Ok(snapshot)
    }

    /// Shared send tail for `set_case` and the invite re-broadcast:
    /// encode the full snapshot (no fallback), MLS-encrypt + upload,
    /// persist the `kind='case'` row (`sender_was_admin: true` — the
    /// caller enforced the gate), emit the diff.
    pub(crate) async fn send_case_snapshot(
        &self,
        room_id: &str,
        snapshot: &CaseSnapshot,
    ) -> Result<(), KupferdrahtError> {
        let group_id = self.resolve_room_group_id(room_id, "set_case").await?;
        let payload = payload_from_snapshot(snapshot);
        let (message_id, plaintext) = crate::wire::proto::build_case_generic_message(payload)?;
        // Step 97a: the row is written as `Sending` before the upload.
        self.send_content_row(
            room_id,
            &group_id,
            "set_case",
            crate::timeline::ContentRow {
                message_id: &message_id,
                plaintext: &plaintext,
                kind: "case",
                body: &case_body(snapshot, true),
            },
        )
        .await
        .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Some("")`/whitespace `case_ref` on create
    /// must generate an id instead of panicking downstream at the
    /// builder's non-empty assert (user input is never assert fuel).
    #[test]
    fn empty_patch_case_ref_generates_an_id_on_create() {
        for raw in ["", "   "] {
            let patch = CasePatch {
                case_ref: Some(raw.to_string()),
                title: Some("Fall".into()),
                ..CasePatch::default()
            };
            let snapshot = apply_case_patch(None, &patch).expect("create");
            assert!(
                !snapshot.case_ref.trim().is_empty(),
                "an id was generated for {raw:?}"
            );
            assert_eq!(snapshot.revision, 1);
        }
    }

    /// The named ref is honored on create and immutable on update.
    #[test]
    fn named_case_ref_is_honored_and_identity_stable() {
        let patch = CasePatch {
            case_ref: Some("case-2026-1".to_string()),
            ..CasePatch::default()
        };
        let created = apply_case_patch(None, &patch).expect("create");
        assert_eq!(created.case_ref, "case-2026-1");
        let update = CasePatch {
            case_ref: Some("other".to_string()),
            ..CasePatch::default()
        };
        let updated = apply_case_patch(Some(&created), &update).expect("update");
        assert_eq!(updated.case_ref, "case-2026-1", "identity never changes");
        assert_eq!(updated.revision, 2);
    }
}
