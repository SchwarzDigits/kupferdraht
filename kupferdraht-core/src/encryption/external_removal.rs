// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Auto-commit of external-sender Remove proposals (Step 28).
//!
//! Wire's backend acts as the group's MLS *external sender* (W-5 /
//! ADR-0004). When it removes a member it cannot commit itself — RFC
//! 9420 forbids a non-member external sender from authoring a commit —
//! so it signs a Remove **proposal** and relies on a remaining
//! **member** to commit it. Kupferdraht must be that committing member.
//!
//! The inbound receive path ([`MlsClient::process_inbound_message`])
//! already stores such a proposal via `store_pending_proposal`. This
//! module adds the missing half: [`MlsClient::commit_external_removals`]
//! drains a pending external Remove into a commit bundle ready for
//! `POST /v14/mls/commit-bundles`, mirroring the member-kick bundle
//! shape (`commit:MlsMessage || group_info:MlsMessage`, no Welcome —
//! see [`super::group_kick`]).
//!
//! Three outcomes, modelled by [`ExternalRemovalOutcome`]:
//! - a pending Remove targeting a **peer** leaf → stage a commit and
//!   return the bundle (caller uploads, then `merge_pending_commit`);
//! - a pending Remove targeting **our own** leaf → we cannot commit our
//!   own removal (`CannotRemoveSelf`); the caller discards the group
//!   locally (a different remaining member commits it for the rest);
//! - **no** pending external Remove → nothing to do.
//!
//! Crypto/transport split (ADR-0009): this module only *produces* the
//! bundle bytes; the actual `POST /v14/mls/commit-bundles` upload and
//! the `409 mls-stale-message` retry live in the sync/client layer.

#![cfg_attr(not(test), allow(dead_code))]

use openmls::framing::Sender;
use openmls::group::{GroupId, MlsGroup};
use openmls::messages::proposals::Proposal;
use openmls_traits::OpenMlsProvider;

use crate::encryption::external_commit::serialize_bundle;
use crate::encryption::mls_client::MlsClient;
use crate::error::KupferdrahtError;

/// Result of [`MlsClient::commit_external_removals`].
#[derive(Debug)]
pub(crate) enum ExternalRemovalOutcome {
    /// A commit removing one or more **peer** leaves was staged. The
    /// carried bytes are the TLS-serialized commit bundle ready for
    /// `POST /v14/mls/commit-bundles`; the caller uploads it and then
    /// calls [`MlsClient::merge_pending_commit`] on success (or
    /// [`MlsClient::clear_pending_commit`] on rejection).
    Committed(Vec<u8>),
    /// The pending external Remove targets our own leaf. We must not
    /// attempt to commit it (`CannotRemoveSelf`); the caller discards
    /// the group locally via [`MlsClient::discard_group`].
    SelfRemoved,
    /// No pending external-sender Remove proposal is queued — no-op.
    Nothing,
}

impl MlsClient {
    /// Commit a pending external-sender Remove proposal on the group
    /// keyed by `group_id_bytes`, returning the resulting commit bundle
    /// (or a signal that no commit is appropriate — see
    /// [`ExternalRemovalOutcome`]).
    ///
    /// # Errors
    /// * `KupferdrahtError::MlsGroupMissing` / `InvalidInput` if the group
    ///   is not in local storage.
    /// * `KupferdrahtError::Storage` for openmls storage failures.
    /// * `KupferdrahtError::Generic` if openmls rejects the commit build.
    /// * `KupferdrahtError::Encoding` if bundle serialization fails.
    pub fn commit_external_removals(
        &self,
        group_id_bytes: &[u8],
    ) -> Result<ExternalRemovalOutcome, KupferdrahtError> {
        if group_id_bytes.is_empty() {
            return Err(KupferdrahtError::InvalidInput(
                "commit_external_removals: group_id_bytes must be non-empty".into(),
            ));
        }
        self.provider().with_write_tx(|provider| {
            let mut group =
                MlsGroup::load(provider.storage(), &GroupId::from_slice(group_id_bytes))
                    .map_err(|e| KupferdrahtError::Storage(format!("load MlsGroup: {e}")))?
                    .ok_or(KupferdrahtError::MlsGroupMissing)?;

            match classify_pending_removes(&group) {
                PendingRemove::None => return Ok(ExternalRemovalOutcome::Nothing),
                // A pending Remove targets our own leaf: openmls would
                // reject the commit with `CannotRemoveSelf`. A different
                // remaining member commits it for the rest of the group; we
                // surface `SelfRemoved` so the caller discards locally.
                PendingRemove::Own => return Ok(ExternalRemovalOutcome::SelfRemoved),
                PendingRemove::Peer => {}
            }

            super::group_invite::ensure_commit_wire_format(&group)?;
            // `commit_to_pending_proposals` consumes the proposal store and
            // stages the commit; the caller uploads the bundle and then
            // `merge_pending_commit`s (or `clear_pending_commit`s on a
            // `409 mls-stale-message`).
            let (commit, _welcome_none_for_remove, group_info) = group
                .commit_to_pending_proposals(provider, self.signature_keys())
                .map_err(|e| {
                    KupferdrahtError::Generic(format!("commit_to_pending_proposals: {e:?}"))
                })?;
            let bundle = serialize_bundle(&commit, group_info)?;
            Ok(ExternalRemovalOutcome::Committed(bundle))
        })
    }
}

/// Whether the group's queued proposals contain a Remove, and if so
/// whether any targets our own leaf (the self-removal case).
enum PendingRemove {
    /// No Remove proposal is queued.
    None,
    /// A Remove targets our own leaf — must not be self-committed.
    Own,
    /// One or more Removes target peer leaves only.
    Peer,
}

/// Scan the queued proposals once, classifying the Remove situation.
/// The external sender's Remove of our own leaf takes precedence (`Own`).
/// A member's Remove of our leaf is only a proposal (Step 106, ADR-0034
/// d.3): it drops nothing, and since a batch that includes it would trip
/// `CannotRemoveSelf`, we commit nothing while it is queued — another
/// member commits it.
fn classify_pending_removes(group: &MlsGroup) -> PendingRemove {
    let own_index = group.own_leaf_index();
    let mut saw_peer_remove = false;
    let mut member_removes_us = false;
    for queued in group.pending_proposals() {
        if let Proposal::Remove(remove) = queued.proposal() {
            if remove.removed() != own_index {
                saw_peer_remove = true;
            } else if matches!(queued.sender(), Sender::External(_)) {
                return PendingRemove::Own;
            } else {
                member_removes_us = true;
            }
        }
    }
    if saw_peer_remove && !member_removes_us {
        PendingRemove::Peer
    } else {
        PendingRemove::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openmls::prelude::{MlsMessageIn, WireFormat};
    use tls_codec::DeserializeBytes as _;

    use crate::encryption::test_support::{
        ALICE_USER, BOB_USER, current_epoch, external_remove_proposal, group_with_alice_and_bob,
        group_with_alice_and_joined_bob, member_remove_proposal, own_leaf_index, peer_leaf_index,
    };

    #[test]
    fn process_inbound_message_reports_proposal_queued_for_external_remove() {
        // The sync dispatcher keys off this signal to drive the commit
        // (Step 28). An external Remove proposal must surface as
        // `ProposalQueued`, not `Handshake`.
        let (alice, group_id, es) = group_with_alice_and_bob();
        let bob_idx = peer_leaf_index(&alice, &group_id);
        let proposal = external_remove_proposal(&alice, &group_id, &es, bob_idx);
        let decoded = alice
            .process_inbound_message(&group_id, &proposal)
            .expect("process external proposal");
        assert!(
            matches!(decoded, crate::encryption::DecodedMessage::ProposalQueued),
            "external Remove proposal must signal ProposalQueued, got {decoded:?}"
        );
    }

    #[test]
    fn commit_external_removals_commits_peer_proposal_and_drops_leaf() {
        let (alice, group_id, es) = group_with_alice_and_bob();
        let bob_idx = peer_leaf_index(&alice, &group_id);
        let proposal = external_remove_proposal(&alice, &group_id, &es, bob_idx);
        // Real inbound receive path queues the pending proposal.
        alice
            .process_inbound_message(&group_id, &proposal)
            .expect("process external proposal");

        let outcome = alice
            .commit_external_removals(&group_id)
            .expect("commit_external_removals must succeed for a peer remove");
        let ExternalRemovalOutcome::Committed(bundle) = outcome else {
            panic!("expected Committed, got {outcome:?}");
        };
        assert!(!bundle.is_empty(), "committed bundle must carry bytes");

        // Apply the staged commit; Bob gone, Alice remains.
        alice.merge_pending_commit(&group_id).expect("merge remove");
        let users: std::collections::HashSet<String> = alice
            .group_leaves(&group_id)
            .expect("group_leaves")
            .into_iter()
            .map(|l| l.user_id)
            .collect();
        assert!(
            !users.contains(BOB_USER),
            "bob's leaf must be gone after the external-remove commit merges"
        );
        assert!(
            users.contains(ALICE_USER),
            "alice (the committer) must remain"
        );
    }

    #[test]
    fn commit_external_removals_produces_two_segment_public_message_bundle() {
        let (alice, group_id, es) = group_with_alice_and_bob();
        let bob_idx = peer_leaf_index(&alice, &group_id);
        let proposal = external_remove_proposal(&alice, &group_id, &es, bob_idx);
        alice
            .process_inbound_message(&group_id, &proposal)
            .expect("process external proposal");

        let ExternalRemovalOutcome::Committed(bundle) = alice
            .commit_external_removals(&group_id)
            .expect("commit_external_removals")
        else {
            panic!("expected Committed");
        };
        // Segment 1: commit, PublicMessage (MIXED_PLAINTEXT).
        let (commit_msg, rest) =
            MlsMessageIn::tls_deserialize_bytes(&bundle).expect("commit segment parse");
        assert_eq!(commit_msg.wire_format(), WireFormat::PublicMessage);
        // Segment 2: group_info; no Welcome (a remove adds nobody).
        let (gi_msg, trailing) =
            MlsMessageIn::tls_deserialize_bytes(rest).expect("group_info segment parse");
        assert_eq!(gi_msg.wire_format(), WireFormat::GroupInfo);
        assert!(
            trailing.is_empty(),
            "external-remove bundle must be exactly two segments"
        );
    }

    #[test]
    fn commit_external_removals_advances_epoch_after_merge() {
        let (alice, group_id, es) = group_with_alice_and_bob();
        let epoch_before = current_epoch(&alice, &group_id);
        let bob_idx = peer_leaf_index(&alice, &group_id);
        let proposal = external_remove_proposal(&alice, &group_id, &es, bob_idx);
        alice
            .process_inbound_message(&group_id, &proposal)
            .expect("process external proposal");
        let _ = alice
            .commit_external_removals(&group_id)
            .expect("commit_external_removals");
        alice.merge_pending_commit(&group_id).expect("merge");
        assert_eq!(
            current_epoch(&alice, &group_id),
            epoch_before + 1,
            "committing the external remove must advance the epoch by one"
        );
    }

    #[test]
    fn commit_external_removals_returns_self_removed_for_own_leaf() {
        let (alice, group_id, es) = group_with_alice_and_bob();
        let own = own_leaf_index(&alice, &group_id);
        let proposal = external_remove_proposal(&alice, &group_id, &es, own);
        alice
            .process_inbound_message(&group_id, &proposal)
            .expect("process self-targeting external proposal");

        let outcome = alice
            .commit_external_removals(&group_id)
            .expect("self-remove must not surface as an error (no CannotRemoveSelf)");
        assert!(
            matches!(outcome, ExternalRemovalOutcome::SelfRemoved),
            "expected SelfRemoved, got {outcome:?}"
        );
        // No commit was staged: the group is still intact locally with
        // Alice present (the caller decides to discard_group).
        let users: std::collections::HashSet<String> = alice
            .group_leaves(&group_id)
            .expect("group_leaves")
            .into_iter()
            .map(|l| l.user_id)
            .collect();
        assert!(
            users.contains(ALICE_USER),
            "self-remove must not locally evict Alice before the caller discards the group"
        );
    }

    /// Step 106 (ADR-0034 d.3): a member's Remove proposal for our own
    /// leaf is only a proposal — we neither discard the group nor can we
    /// commit it (another member does).
    #[test]
    fn a_members_remove_of_our_leaf_leaves_the_group_alone() {
        let (alice, bob, group_id) = group_with_alice_and_joined_bob();
        let own = own_leaf_index(&alice, &group_id);
        let proposal = member_remove_proposal(&bob, &group_id, own);
        alice
            .process_inbound_message(&group_id, &proposal)
            .expect("process member proposal");

        let outcome = alice
            .commit_external_removals(&group_id)
            .expect("a member proposal is not an error");
        assert!(
            matches!(outcome, ExternalRemovalOutcome::Nothing),
            "expected Nothing, got {outcome:?}"
        );
    }

    #[test]
    fn commit_external_removals_returns_nothing_without_pending_proposal() {
        let (alice, group_id, _es) = group_with_alice_and_bob();
        let outcome = alice
            .commit_external_removals(&group_id)
            .expect("commit_external_removals with no pending proposal");
        assert!(
            matches!(outcome, ExternalRemovalOutcome::Nothing),
            "expected Nothing, got {outcome:?}"
        );
    }

    #[test]
    fn commit_external_removals_rejects_empty_group_id() {
        let alice = MlsClient::new_fresh(ALICE_USER, "a11ce00000000001").expect("alice MlsClient");
        let err = alice
            .commit_external_removals(&[])
            .expect_err("empty group_id must be rejected");
        assert!(
            matches!(err, KupferdrahtError::InvalidInput(_)),
            "expected InvalidInput for empty group_id, got {err:?}"
        );
    }

    #[test]
    fn commit_external_removals_errors_on_unknown_group() {
        let alice = MlsClient::new_fresh(ALICE_USER, "a11ce00000000001").expect("alice MlsClient");
        let err = alice
            .commit_external_removals(b"never-stored-group")
            .expect_err("unknown group must fail");
        assert!(
            matches!(
                err,
                KupferdrahtError::MlsGroupMissing | KupferdrahtError::InvalidInput(_)
            ),
            "expected MlsGroupMissing/InvalidInput, got {err:?}"
        );
    }
}
