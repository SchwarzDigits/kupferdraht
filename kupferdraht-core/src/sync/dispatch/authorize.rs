// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 95b (ADR-0028) — who may edit or delete an inbound target.
//!
//! Wire's rule, checked against the ADR-0027 (MLS-authenticated) sender
//! and the target row resolved **in the same room**:
//! - an edit applies only when its sender's user is the target's sender
//!   (any device of that user);
//! - a delete applies when its sender's user is the target's sender, or
//!   the target is ephemeral and either its sender is the self user
//!   (another own device syncing a receiver-side expiry) or the target
//!   is the self user's own message (receivers enforce the sender's
//!   timer by deleting it) — observations §74, §91. A self-sent delete
//!   of another member's ordinary message is rejected, as the official
//!   webapp does (§91).
//!
//! No admin override. A rejected edit or delete is dropped with a `warn!`.

use std::sync::Arc;

use tracing::warn;

use crate::client::Client;
use crate::error::KupferdrahtError;
use crate::storage::TimelineEventRecord;

/// Whether an edit by `sender_user_id` may change `target`.
fn may_edit(target: &TimelineEventRecord, sender_user_id: &str) -> bool {
    target.sender_user_id == sender_user_id
}

/// Whether a delete by `sender_user_id` may redact `target`.
fn may_delete(
    target: &TimelineEventRecord,
    sender_user_id: &str,
    self_user_id: Option<&str>,
) -> bool {
    let is_self = |user: &str| self_user_id == Some(user);
    let is_ephemeral = target.expires_at_ms.is_some() || target.ephemeral_ttl_ms.is_some();
    target.sender_user_id == sender_user_id
        || (is_ephemeral && (is_self(sender_user_id) || is_self(&target.sender_user_id)))
}

/// The operation an inbound message performs on an earlier one.
#[derive(Clone, Copy)]
pub(super) enum TargetOp {
    Edit,
    Delete,
}

/// Resolve `target_id` in `room_id` and check ADR-0028 for `op` by
/// `sender_user_id`. `Ok(true)` — apply it; `Ok(false)` — the target is
/// unknown in this room or the sender may not touch it (logged).
///
/// # Errors
/// `KupferdrahtError::Storage` / `Encoding` from the target lookup.
pub(super) async fn target_op_is_authorized(
    client: &Arc<Client>,
    room_id: &str,
    target_id: &str,
    sender_user_id: &str,
    op: TargetOp,
) -> Result<bool, KupferdrahtError> {
    let Some(store) = client.store_handle() else {
        return Ok(false);
    };
    let Some(target) = store
        .load_timeline_event_in_room(room_id, target_id)
        .await?
    else {
        warn!(
            room_id,
            target_id, "edit/delete target not found in this room — no effect"
        );
        return Ok(false);
    };
    let self_user = client.user_id().map(crate::identity::UserId::as_str);
    let allowed = match op {
        TargetOp::Edit => may_edit(&target, sender_user_id),
        TargetOp::Delete => may_delete(&target, sender_user_id, self_user),
    };
    if !allowed {
        warn!(
            room_id,
            target_id,
            sender = sender_user_id,
            target_sender = %target.sender_user_id,
            "edit/delete of another user's message rejected (ADR-0028)"
        );
    }
    Ok(allowed)
}

#[cfg(test)]
mod tests {
    use super::{may_delete, may_edit};
    use crate::storage::TimelineEventRecord;

    const BOB: &str = "bob@example.test";
    const CAROL: &str = "carol@example.test";
    const ME: &str = "me@example.test";

    fn row(sender: &str, ephemeral: bool) -> TimelineEventRecord {
        TimelineEventRecord {
            sender_user_id: sender.into(),
            expires_at_ms: ephemeral.then_some(1_000),
            ..TimelineEventRecord::default()
        }
    }

    #[test]
    fn only_the_original_user_may_edit() {
        assert!(may_edit(&row(BOB, false), BOB));
        assert!(!may_edit(&row(BOB, false), CAROL));
        assert!(!may_edit(&row(BOB, false), ME), "not even the self user");
    }

    #[test]
    fn the_original_user_may_delete() {
        assert!(may_delete(&row(BOB, false), BOB, Some(ME)));
    }

    #[test]
    fn a_third_party_may_not_delete() {
        assert!(!may_delete(&row(BOB, false), CAROL, Some(ME)));
        assert!(
            !may_delete(&row(BOB, true), CAROL, Some(ME)),
            "not someone else's ephemeral either"
        );
    }

    #[test]
    fn the_self_user_may_delete_a_foreign_ephemeral_only() {
        assert!(
            may_delete(&row(BOB, true), ME, Some(ME)),
            "own-device sync of a receiver-side expiry"
        );
        assert!(
            !may_delete(&row(BOB, false), ME, Some(ME)),
            "not another member's ordinary message (observations §91)"
        );
    }

    #[test]
    fn anyone_may_delete_the_self_users_own_ephemeral() {
        assert!(may_delete(&row(ME, true), CAROL, Some(ME)));
        assert!(
            !may_delete(&row(ME, false), CAROL, Some(ME)),
            "a non-ephemeral own message stays protected"
        );
    }

    #[test]
    fn an_own_ephemeral_is_recognised_by_its_ttl_alone() {
        // A failed own send stamps no `expires_at_ms`, only the TTL.
        let mut failed_own = row(ME, false);
        failed_own.ephemeral_ttl_ms = Some(60_000);
        assert!(may_delete(&failed_own, CAROL, Some(ME)));
    }

    #[test]
    fn without_a_known_self_user_only_the_original_sender_may_delete() {
        assert!(!may_delete(&row(ME, true), CAROL, None));
        assert!(may_delete(&row(BOB, false), BOB, None));
    }
}
