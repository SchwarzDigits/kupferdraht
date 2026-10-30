// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 74 — end-of-cycle flush of queued external-removal commits.
//!
//! The Step-28 receive path used to commit **per received Remove
//! proposal**, which storms an N-deep backlog with N−1 guaranteed
//! `mls-commit-missing-references` rejections (each surfaced to
//! consumers via `on_sync_error`) and leaves a no-re-trigger window
//! when the final commit fails transiently — observations §69/§70.
//! Since Step 74 the dispatcher only *records* the room
//! (`Client::queue_removal_flush`); this module commits once per
//! room at the end of the dispatch cycle, covering every proposal
//! that arrived in it.

use std::sync::Arc;

use tracing::{debug, warn};

use crate::client::Client;
use crate::error::KupferdrahtError;
use crate::sync::service::SyncService;

impl SyncService {
    /// Commit queued external-sender Removes, once per room (plan
    /// 74). Rooms leave the queue only on success — failures retry
    /// on the next cycle:
    ///
    /// * `mls-commit-missing-references` → debug only. On the push
    ///   path proposals arrive one frame (= one cycle) at a time, so
    ///   a mid-backlog rejection is the expected, self-resolving
    ///   outcome — not consumer-visible error material.
    /// * anything else → `on_sync_error` + retry, the same surface
    ///   the pre-74 inline commit had.
    ///
    /// Infallible by design: a flush failure must never abort the
    /// drain or stream cycle that triggered it (the proposals are
    /// safe in openmls storage either way).
    pub(crate) async fn flush_removal_commits(&self, client: &Arc<Client>) {
        for room_id in client.pending_removal_flushes() {
            debug_assert!(
                !room_id.is_empty(),
                "flush_removal_commits: queued room_id must be non-empty"
            );
            match client.commit_pending_external_removal(&room_id).await {
                Ok(()) => client.ack_removal_flush(&room_id),
                Err(e) if is_missing_references(&e) => {
                    debug!(
                        room = %room_id,
                        "removal flush: commit not covering all pending proposals \
                         (backlog still arriving); retrying next cycle"
                    );
                }
                Err(e) => {
                    self.notify_error(&format!("removal flush ({room_id}): {e}"));
                    warn!(room = %room_id, error = %e, "removal flush failed; retrying next cycle");
                }
            }
        }
    }
}

/// Wire's "commit does not reference all pending proposals" rejection
/// — the expected mid-backlog outcome, matched on the stable error
/// label (`BackendRejected` carries it verbatim, api-notes
/// conventions).
fn is_missing_references(e: &KupferdrahtError) -> bool {
    matches!(
        e,
        KupferdrahtError::BackendRejected(msg) if msg.contains("mls-commit-missing-references")
    )
}

#[cfg(test)]
mod tests {
    use super::is_missing_references;
    use crate::error::KupferdrahtError;

    #[test]
    fn matches_only_the_missing_references_label() {
        assert!(is_missing_references(&KupferdrahtError::BackendRejected(
            "upload_commit_bundle 400 Bad Request: {\"code\":400,\
             \"label\":\"mls-commit-missing-references\",\"message\":\"…\"}"
                .into()
        )));
        assert!(!is_missing_references(&KupferdrahtError::BackendRejected(
            "upload_commit_bundle 409 Conflict: {\"label\":\"mls-stale-message\"}".into()
        )));
        assert!(!is_missing_references(&KupferdrahtError::Network(
            "mls-commit-missing-references mentioned, wrong class".into()
        )));
    }
}
