// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Member-remove commit (Step 6d).
//!
//! `MlsClient::remove_members_commit` builds an MLS Remove-commit
//! against a locally persisted `MlsGroup`, given a list of qualified
//! Wire `user_id`s (`<uuid>@<domain>`). Every leaf belonging to one
//! of the listed users is collected from the local group state and
//! fed into [`openmls::group::MlsGroup::remove_members`]. The
//! resulting bundle is ready for `POST /v14/mls/commit-bundles`.
//!
//! Bundle layout (Wire's parser): `commit:MlsMessage ||
//! group_info:MlsMessage`. Remove-commits do not produce a Welcome
//! — no new members are joining — so the layout matches an
//! external-commit's two-segment shape (see [`super::external_commit`]).
//!
//! The same invariants 6b/6c discovered apply here: the persisted
//! group is back-filled to `MIXED_PLAINTEXT` before staging
//! ([`MlsClient::add_members_commit`] documents why), and any peer-
//! originated proposals deposited by sync-fetch are cleared so the
//! commit only carries the Removes the caller asked for.
//!
//! Pending-commit dance: after `MlsGroup::remove_members` succeeds
//! the commit is **staged** in openmls's local storage. The caller
//! follows up with [`MlsClient::merge_pending_commit`] once Wire's
//! `/v14/mls/commit-bundles` has accepted the upload, or
//! [`MlsClient::clear_pending_commit`] on rejection — same lifecycle
//! as `add_members_commit`.

#![cfg_attr(not(test), allow(dead_code))]

use std::collections::HashSet;

use openmls::group::MlsGroup;
use openmls::prelude::LeafNodeIndex;
use tracing::debug;

use crate::encryption::external_commit::serialize_bundle;
use crate::encryption::mls_client::MlsClient;
use crate::error::KupferdrahtError;

impl MlsClient {
    /// Build a Remove-commit on the locally persisted MLS group
    /// keyed by `group_id_bytes` that removes every leaf whose
    /// credential identity matches one of the qualified
    /// `user_ids` (`<uuid>@<domain>`). Returns the TLS-serialized
    /// commit bundle ready for `POST /v14/mls/commit-bundles`.
    ///
    /// One Wire user can own up to 7 devices and therefore up to 7
    /// leaves in any given group; a single
    /// `remove_members_commit(["alice@example.org"])` removes **all**
    /// of Alice's leaves in one commit, matching Wire's "every
    /// device of every member-user has a leaf" invariant
    /// (observations.md §27, the inverse direction).
    ///
    /// On success the local MLS group has a **staged** pending
    /// commit; the caller must call [`Self::merge_pending_commit`]
    /// after Wire accepts the upload (or
    /// [`Self::clear_pending_commit`] on rejection) to keep the
    /// local epoch in lockstep with the server.
    ///
    /// # Errors
    /// * `KupferdrahtError::InvalidInput` if the group is not in
    ///   storage, `user_ids` is empty, the caller's own user is in
    ///   the list (Kupferdraht does not yet support self-kick), or no
    ///   leaf in the group corresponds to any of the listed users.
    /// * `KupferdrahtError::Storage` for openmls storage failures.
    /// * `KupferdrahtError::Generic` for openmls Remove-commit
    ///   construction failures.
    pub fn remove_members_commit(
        &self,
        group_id_bytes: &[u8],
        user_ids: &[String],
    ) -> Result<Vec<u8>, KupferdrahtError> {
        debug_assert!(
            !group_id_bytes.is_empty(),
            "group_id_bytes must be non-empty"
        );
        if user_ids.is_empty() {
            return Err(KupferdrahtError::InvalidInput(
                "remove_members_commit: at least one user_id is required".into(),
            ));
        }
        self.provider().with_write_tx(|provider| {
            let group = super::group_invite::load_group_for_commit(
                provider,
                group_id_bytes,
                "remove_members_commit: group not in storage (call create_group / join_room first)",
            )?;
            let self_user =
                user_id_of(&self.credential_with_key().credential).ok_or_else(|| {
                    KupferdrahtError::Generic(
                        "remove_members_commit: own credential identity is malformed".into(),
                    )
                })?;
            let indices = resolve_target_leaf_indices(&group, user_ids, &self_user)?;
            debug!(
                group_id_len = group_id_bytes.len(),
                removes = indices.len(),
                "build remove-members commit"
            );
            let mut group = group;
            let (commit, _welcome_none_for_remove, group_info) = group
                .remove_members(provider, self.signature_keys(), &indices)
                .map_err(|e| {
                    KupferdrahtError::Generic(format!("MlsGroup::remove_members: {e:?}"))
                })?;
            serialize_bundle(&commit, group_info)
        })
    }
}

/// Map the caller-supplied user-ids to leaf indices in `group`.
/// Rejects an attempt to remove `self_user` (Kupferdraht does not yet
/// support self-kick) and rejects the call when no leaf matches.
fn resolve_target_leaf_indices(
    group: &MlsGroup,
    user_ids: &[String],
    self_user: &str,
) -> Result<Vec<LeafNodeIndex>, KupferdrahtError> {
    let target_users: HashSet<&str> = user_ids.iter().map(String::as_str).collect();
    if target_users.contains(self_user) {
        return Err(KupferdrahtError::InvalidInput(
            "remove_members_commit: removing self is not supported in Step 6d \
             (use logout / device-revocation flows once they exist)"
                .into(),
        ));
    }
    let indices: Vec<LeafNodeIndex> = group
        .members()
        .filter_map(|m| {
            let cred_user = user_id_of(&m.credential)?;
            target_users.contains(cred_user.as_str()).then_some(m.index)
        })
        .collect();
    if indices.is_empty() {
        return Err(KupferdrahtError::InvalidInput(format!(
            "remove_members_commit: no leaf in this group belongs to any of {user_ids:?}",
        )));
    }
    Ok(indices)
}

/// The qualified `<uuid>@<domain>` of a leaf credential, `None` for a
/// non-W-4 leaf (Step 95a: the one parser, `leaf_identity`).
fn user_id_of(credential: &openmls::prelude::Credential) -> Option<String> {
    super::leaf_identity::parse_leaf_identity(credential)
        .ok()
        .map(|leaf| leaf.user_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use openmls::prelude::{MlsMessageBodyIn, MlsMessageIn, WireFormat};
    use tls_codec::DeserializeBytes as _;

    use crate::encryption::test_support as ts;

    /// Step 71 (gap-B.20): the kick path shares
    /// `load_group_for_commit` — a queued external Remove targeting a
    /// third member must ride the user's Remove-commit.
    #[test]
    fn kick_commit_carries_pending_peer_remove() {
        const CHARLIE_USER: &str = "9e8d7c6b-1111-2222-3333-444455556666@wire.com";
        let (alice, group_id, es) = ts::group_with_alice_and_bob();
        ts::add_member(&alice, &group_id, CHARLIE_USER, "aabbccdd00112233");
        let charlie_idx = ts::leaf_index_of(&alice, &group_id, CHARLIE_USER);
        let proposal = ts::external_remove_proposal(&alice, &group_id, &es, charlie_idx);
        alice
            .process_inbound_message(&group_id, &proposal)
            .expect("queue the external remove via the real inbound path");

        alice
            .remove_members_commit(&group_id, &[ts::BOB_USER.to_string()])
            .expect("kick-commit with a queued peer remove");
        alice.merge_pending_commit(&group_id).expect("merge");

        let users: std::collections::HashSet<String> = alice
            .group_leaves(&group_id)
            .expect("group_leaves")
            .into_iter()
            .map(|l| l.user_id)
            .collect();
        assert!(!users.contains(ts::BOB_USER), "bob was kicked");
        assert!(
            !users.contains(CHARLIE_USER),
            "the queued external Remove must ride the kick-commit \
             (Step 28 obligation) — not be silently cleared"
        );
        assert!(users.contains(ts::ALICE_USER), "the committer remains");
    }

    /// Step 71 edge: the user kicks the SAME leaf a queued external
    /// Remove targets — the commit carries both proposals for one
    /// leaf and must still build (openmls dedups per-leaf Removes in
    /// the proposal queue) with Bob gone exactly once.
    #[test]
    fn kick_commit_tolerates_external_remove_of_the_same_leaf() {
        let (alice, group_id, es) = ts::group_with_alice_and_bob();
        let bob_idx = ts::peer_leaf_index(&alice, &group_id);
        let proposal = ts::external_remove_proposal(&alice, &group_id, &es, bob_idx);
        alice
            .process_inbound_message(&group_id, &proposal)
            .expect("queue the external remove");

        alice
            .remove_members_commit(&group_id, &[ts::BOB_USER.to_string()])
            .expect("kick of the externally-removed leaf must not error");
        alice.merge_pending_commit(&group_id).expect("merge");

        let users: std::collections::HashSet<String> = alice
            .group_leaves(&group_id)
            .expect("group_leaves")
            .into_iter()
            .map(|l| l.user_id)
            .collect();
        assert!(!users.contains(ts::BOB_USER), "bob is gone exactly once");
        assert!(users.contains(ts::ALICE_USER), "the committer remains");
    }

    /// Step 71 guard, kick side: the `CannotRemoveSelf` protection
    /// must hold on THIS path too — it lives in the shared
    /// `load_group_for_commit`, and this test keeps it there (a
    /// refactor moving the selection into the add path only would
    /// slip past the peer-remove tests).
    #[test]
    fn kick_commit_still_clears_pending_self_remove() {
        let (alice, group_id, es) = ts::group_with_alice_and_bob();
        let own_idx = ts::own_leaf_index(&alice, &group_id);
        let proposal = ts::external_remove_proposal(&alice, &group_id, &es, own_idx);
        alice
            .process_inbound_message(&group_id, &proposal)
            .expect("queue the self-targeting external remove");

        alice
            .remove_members_commit(&group_id, &[ts::BOB_USER.to_string()])
            .expect("kick-commit must not trip CannotRemoveSelf");
        alice.merge_pending_commit(&group_id).expect("merge");

        let users: std::collections::HashSet<String> = alice
            .group_leaves(&group_id)
            .expect("group_leaves")
            .into_iter()
            .map(|l| l.user_id)
            .collect();
        assert!(!users.contains(ts::BOB_USER), "bob was kicked");
        assert!(
            users.contains(ts::ALICE_USER),
            "the self-remove was cleared, not committed — alice stays"
        );
    }

    /// Standard P-256 backend-removal-key placeholder used by every
    /// `create_group` call in this module's tests.
    const REMOVAL_KEY_PLACEHOLDER: [u8; 65] = [0x04u8; 65];

    fn alice_client() -> MlsClient {
        MlsClient::new_fresh(
            "a11ce000-0000-4000-8000-000000000001@wire.com",
            "a11ce00000000001",
        )
        .expect("alice MlsClient")
    }

    fn bob_client() -> MlsClient {
        MlsClient::new_fresh(
            "8d1f2c34-aaaa-bbbb-cccc-dddddddddddd@wire.com",
            "deadbeefcafef00d",
        )
        .expect("bob MlsClient")
    }

    /// Drive Alice through `create_group` + an Add of Bob so the
    /// group has someone to kick. Returns `(group_id, bob_user_id)`.
    fn alice_group_with_bob(alice: &MlsClient) -> (Vec<u8>, String) {
        let group_id = b"kupferdraht-test-group-id-6d".to_vec();
        alice
            .create_group(&group_id, &REMOVAL_KEY_PLACEHOLDER)
            .expect("create_group");
        let bob = bob_client();
        let bob_kp = bob
            .generate_key_packages(1)
            .expect("generate_key_packages")
            .into_iter()
            .next()
            .expect("at least one KeyPackage");
        alice
            .add_members_commit(&group_id, &[bob_kp])
            .expect("add bob");
        alice.merge_pending_commit(&group_id).expect("merge add");
        (
            group_id,
            "8d1f2c34-aaaa-bbbb-cccc-dddddddddddd@wire.com".into(),
        )
    }

    #[test]
    fn remove_members_commit_errors_on_empty_user_ids() {
        let alice = alice_client();
        let (group_id, _) = alice_group_with_bob(&alice);
        let err = alice
            .remove_members_commit(&group_id, &[])
            .expect_err("empty list must fail");
        assert!(
            matches!(err, KupferdrahtError::InvalidInput(_)),
            "expected InvalidInput, got {err:?}"
        );
    }

    #[test]
    fn remove_members_commit_errors_when_group_not_in_storage() {
        let alice = alice_client();
        let err = alice
            .remove_members_commit(b"never-stored", &["bob@wire.com".into()])
            .expect_err("must fail without prior create_group");
        assert!(
            matches!(err, KupferdrahtError::InvalidInput(_)),
            "expected InvalidInput, got {err:?}"
        );
    }

    #[test]
    fn remove_members_commit_rejects_self_kick() {
        let alice = alice_client();
        let (group_id, _) = alice_group_with_bob(&alice);
        let err = alice
            .remove_members_commit(
                &group_id,
                &["a11ce000-0000-4000-8000-000000000001@wire.com".into()],
            )
            .expect_err("self-kick must fail");
        assert!(
            matches!(err, KupferdrahtError::InvalidInput(_)),
            "expected InvalidInput, got {err:?}"
        );
    }

    #[test]
    fn remove_members_commit_errors_when_no_leaf_matches() {
        let alice = alice_client();
        let (group_id, _) = alice_group_with_bob(&alice);
        let err = alice
            .remove_members_commit(
                &group_id,
                &["00000000-0000-0000-0000-000000000000@wire.com".into()],
            )
            .expect_err("non-member kick must fail");
        assert!(
            matches!(err, KupferdrahtError::InvalidInput(_)),
            "expected InvalidInput, got {err:?}"
        );
    }

    #[test]
    fn remove_members_commit_returns_two_segment_bundle() {
        let alice = alice_client();
        let (group_id, bob_user_id) = alice_group_with_bob(&alice);
        let bundle = alice
            .remove_members_commit(&group_id, &[bob_user_id])
            .expect("remove_members_commit must succeed");
        // Segment 1: commit (PublicMessage, MIXED_PLAINTEXT).
        let (commit_msg, rest) =
            MlsMessageIn::tls_deserialize_bytes(&bundle).expect("commit segment must parse");
        assert_eq!(commit_msg.wire_format(), WireFormat::PublicMessage);
        // Segment 2: group_info envelope.
        let (gi_msg, trailing) =
            MlsMessageIn::tls_deserialize_bytes(rest).expect("group_info segment must parse");
        assert_eq!(gi_msg.wire_format(), WireFormat::GroupInfo);
        assert!(matches!(gi_msg.extract(), MlsMessageBodyIn::GroupInfo(_)));
        assert!(
            trailing.is_empty(),
            "remove-bundle must contain exactly two segments — no Welcome"
        );
    }

    #[test]
    fn remove_members_commit_drops_target_leaf_after_merge() {
        let alice = alice_client();
        let (group_id, bob_user_id) = alice_group_with_bob(&alice);
        // Pre-merge: alice + bob.
        let pre = alice.group_leaves(&group_id).expect("pre");
        let pre_users: std::collections::HashSet<&str> =
            pre.iter().map(|l| l.user_id.as_str()).collect();
        assert!(pre_users.contains("8d1f2c34-aaaa-bbbb-cccc-dddddddddddd@wire.com"));
        // Stage + merge the kick.
        let _bundle = alice
            .remove_members_commit(&group_id, &[bob_user_id])
            .expect("remove_members_commit");
        alice.merge_pending_commit(&group_id).expect("merge remove");
        let post = alice.group_leaves(&group_id).expect("post");
        let post_users: std::collections::HashSet<&str> =
            post.iter().map(|l| l.user_id.as_str()).collect();
        assert!(
            !post_users.contains("8d1f2c34-aaaa-bbbb-cccc-dddddddddddd@wire.com"),
            "post-merge tree must no longer enumerate bob"
        );
        assert!(
            post_users.contains("a11ce000-0000-4000-8000-000000000001@wire.com"),
            "alice (the committer) must still be present"
        );
    }
}
