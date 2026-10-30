// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Member-add commit (Step 6b).
//!
//! `MlsClient::add_members_commit` builds an MLS Add-commit against a
//! locally persisted `MlsGroup`, given one or more TLS-serialized
//! `KeyPackage`s claimed via Wire's
//! `POST /v14/mls/key-packages/claim/{user_domain}/{user}` endpoint.
//! On success it returns the TLS-serialized commit-bundle bytes ready
//! for `POST /v14/mls/commit-bundles`.
//!
//! Bundle layout (Wire's parser): `commit:MlsMessage ||
//! group_info:MlsMessage || welcome:MlsMessage`. Add-commits always
//! produce a Welcome (the new members' epoch keys); zero-add commits
//! and external-commits don't, see [`super::external_commit`].
//!
//! Pending-commit dance: after `MlsGroup::add_members` succeeds the
//! commit is **staged** in openmls's local storage but not merged.
//! The caller must follow up with [`MlsClient::merge_pending_commit`]
//! once Wire's `/v14/mls/commit-bundles` has accepted the upload, or
//! [`MlsClient::clear_pending_commit`] on a rejected upload, so the
//! local epoch does not drift ahead of the server.

#![cfg_attr(not(test), allow(dead_code))]

use std::collections::HashSet;

use openmls::ciphersuite::hash_ref::ProposalRef;
use openmls::group::{GroupId, MIXED_PLAINTEXT_WIRE_FORMAT_POLICY, MlsGroup};
use openmls::messages::proposals::Proposal;
use openmls::prelude::{KeyPackage, KeyPackageIn, ProtocolVersion};
use openmls_traits::OpenMlsProvider;
use tls_codec::DeserializeBytes as _;
use tracing::debug;

use crate::encryption::external_commit::serialize_bundle_with_welcome;
use crate::encryption::mls_client::MlsClient;
use crate::error::KupferdrahtError;

impl MlsClient {
    /// Build an Add-commit on the locally persisted MLS group keyed
    /// by `group_id_bytes` that adds every `KeyPackage` in
    /// `key_package_bytes_list`. Returns the TLS-serialized commit
    /// bundle ready for `POST /v14/mls/commit-bundles`.
    ///
    /// On success the local MLS group has a **staged** pending
    /// commit; the caller must call [`Self::merge_pending_commit`]
    /// after Wire accepts the upload (or
    /// [`Self::clear_pending_commit`] on rejection) to keep the local
    /// epoch in lockstep with the server.
    ///
    /// `KeyPackage` validation: each entry is deserialized through
    /// `KeyPackageIn::tls_deserialize` and validated under
    /// `ProtocolVersion::Mls10`. A claim that fails validation
    /// surfaces as `KupferdrahtError::Encoding` carrying openmls's reason.
    ///
    /// # Errors
    /// * `KupferdrahtError::InvalidInput` if the group id is not in
    ///   storage or `key_package_bytes_list` is empty.
    /// * `KupferdrahtError::Encoding` for malformed / invalid `KeyPackage`
    ///   bytes.
    /// * `KupferdrahtError::Storage` for openmls storage failures.
    /// * `KupferdrahtError::Generic` for openmls Add-commit construction
    ///   failures (e.g. an existing pending commit, missing PSK, …).
    pub fn add_members_commit(
        &self,
        group_id_bytes: &[u8],
        key_package_bytes_list: &[Vec<u8>],
    ) -> Result<Vec<u8>, KupferdrahtError> {
        debug_assert!(
            !group_id_bytes.is_empty(),
            "group_id_bytes must be non-empty"
        );
        if key_package_bytes_list.is_empty() {
            return Err(KupferdrahtError::InvalidInput(
                "add_members_commit: at least one KeyPackage is required".into(),
            ));
        }
        self.provider().with_write_tx(|provider| {
            let mut group = load_group_for_commit(
                provider,
                group_id_bytes,
                "add_members_commit: group not in storage (call create_group first)",
            )?;
            let key_packages = filter_new_key_packages(provider, &group, key_package_bytes_list)?;
            debug!(
                group_id_len = group_id_bytes.len(),
                adds = key_packages.len(),
                "build add-members commit"
            );
            // Use `add_members_without_update`, not `add_members`: Wire
            // rejects a path-update on the committer's leaf
            // (`force_self_update=true`, the openmls default) with
            // `mls-protocol-error: "Tried to add invalid LeafNode: ..."`
            // (observations.md §26). RFC 9420 §17.4 permits a path-less Add — the
            // new members' KeyPackages carry their own signed leaves.
            let (commit, welcome, group_info) = group
                .add_members_without_update(provider, self.signature_keys(), &key_packages)
                .map_err(|e| {
                    KupferdrahtError::Generic(format!(
                        "MlsGroup::add_members_without_update: {e:?}"
                    ))
                })?;
            serialize_bundle_with_welcome(&commit, group_info, Some(welcome))
        })
    }
}

fn validate_key_packages<P: OpenMlsProvider>(
    provider: &P,
    key_package_bytes_list: &[Vec<u8>],
) -> Result<Vec<KeyPackage>, KupferdrahtError> {
    let mut out = Vec::with_capacity(key_package_bytes_list.len());
    for bytes in key_package_bytes_list {
        let (kp_in, rest) = KeyPackageIn::tls_deserialize_bytes(bytes).map_err(|e| {
            KupferdrahtError::Encoding(format!("KeyPackageIn TLS deserialize: {e}"))
        })?;
        if !rest.is_empty() {
            return Err(KupferdrahtError::Encoding(
                "KeyPackage bytes had trailing data after the envelope".into(),
            ));
        }
        let kp = kp_in
            .validate(provider.crypto(), ProtocolVersion::Mls10)
            .map_err(|e| KupferdrahtError::Encoding(format!("KeyPackage validation: {e:?}")))?;
        out.push(kp);
    }
    Ok(out)
}

/// Load `group_id_bytes` from openmls storage and clear queued
/// peer-proposals so the next commit is atomic in the caller-requested
/// set only. `not_in_storage_msg` is the `InvalidInput` text returned
/// when the id has no group on disk yet — kept op-specific for clarity.
///
/// Precondition: the group's handshake wire-format is `MIXED_PLAINTEXT`
/// (Wire's commit-bundle parser requirement, observations.md §28). This is an
/// invariant established at group entry — `create_group` and
/// `join_by_external_commit` both set it — so it is asserted here, not
/// re-established. (Groups from pre-`external_commit` Kupferdraht builds
/// defaulted to `PURE_CIPHERTEXT`; that legacy path is no longer
/// supported.)
pub(super) fn load_group_for_commit<P: OpenMlsProvider>(
    provider: &P,
    group_id_bytes: &[u8],
    not_in_storage_msg: &'static str,
) -> Result<MlsGroup, KupferdrahtError> {
    let group_id = GroupId::from_slice(group_id_bytes);
    let mut group = MlsGroup::load(provider.storage(), &group_id)
        .map_err(|e| KupferdrahtError::Storage(format!("load MlsGroup: {e}")))?
        .ok_or_else(|| KupferdrahtError::InvalidInput(not_in_storage_msg.into()))?;
    ensure_commit_wire_format(&group)?;
    drop_non_obligation_proposals(provider, &mut group)?;
    Ok(group)
}

/// Step 95c: Wire's commit-bundle parser needs plaintext handshake
/// messages — the `MIXED_PLAINTEXT` policy every current entry path
/// (`create_group` / `join_by_external_commit`) sets. A group an older
/// build persisted with openmls's `PURE_CIPHERTEXT` default is state of
/// an existing `mls.db`, not a code invariant.
///
/// # Errors
/// `KupferdrahtError::InvalidInput` for any other policy — re-join the room.
pub(super) fn ensure_commit_wire_format(group: &MlsGroup) -> Result<(), KupferdrahtError> {
    if group.configuration().wire_format_policy() != MIXED_PLAINTEXT_WIRE_FORMAT_POLICY {
        return Err(KupferdrahtError::InvalidInput(
            "group predates the MIXED_PLAINTEXT wire format Wire requires for commits — \
             re-join the room"
                .into(),
        ));
    }
    Ok(())
}

/// Selective pending-proposal hygiene before a user commit (Step 71,
/// gap-B.20). A queued backend external Remove targeting a **peer**
/// leaf is a Step-28 commit obligation — it stays queued and rides
/// this commit (a commit covers the pending-proposal list, RFC 9420
/// §12.4). Everything else is dropped:
///
/// * a Remove targeting **our own** leaf — committing it trips
///   `CreateCommitError::CannotRemoveSelf` (live-confirmed); if the
///   backend is removing us, it re-issues the
///   proposal at the next epoch, so dropping loses nothing;
/// * every non-Remove class (Update / Add) — Kupferdraht's commits stay
///   atomic in the caller-requested set, and Wire's external sender
///   emits only Removes.
fn drop_non_obligation_proposals<P: OpenMlsProvider>(
    provider: &P,
    group: &mut MlsGroup,
) -> Result<(), KupferdrahtError> {
    let own_index = group.own_leaf_index();
    let dropped: Vec<ProposalRef> = group
        .pending_proposals()
        .filter(|queued| match queued.proposal() {
            Proposal::Remove(remove) => remove.removed() == own_index,
            _ => true,
        })
        .map(|queued| queued.proposal_reference_ref().clone())
        .collect();
    debug!(
        dropped = dropped.len(),
        "pre-commit proposal hygiene (peer Removes kept)"
    );
    for proposal_ref in &dropped {
        group
            .remove_pending_proposal(provider.storage(), proposal_ref)
            .map_err(|e| KupferdrahtError::Storage(format!("remove_pending_proposal: {e}")))?;
    }
    Ok(())
}

/// Validate the supplied `KeyPackage` bytes and drop entries whose
/// signature key is already a leaf in `group`. Wire's
/// `mls/key-packages/claim` sometimes returns KPs whose owner is
/// already a member; openmls would reject duplicates with
/// `DuplicateSignatureKey`. Returns `InvalidInput` when nothing is
/// left to add.
fn filter_new_key_packages<P: OpenMlsProvider>(
    provider: &P,
    group: &MlsGroup,
    key_package_bytes_list: &[Vec<u8>],
) -> Result<Vec<KeyPackage>, KupferdrahtError> {
    let validated = validate_key_packages(provider, key_package_bytes_list)?;
    let existing_keys: HashSet<Vec<u8>> = group.members().map(|m| m.signature_key).collect();
    let key_packages: Vec<KeyPackage> = validated
        .into_iter()
        .filter(|kp| !existing_keys.contains(kp.leaf_node().signature_key().as_slice()))
        .collect();
    if key_packages.is_empty() {
        return Err(KupferdrahtError::InvalidInput(
            "add_members_commit: every claimed KeyPackage already corresponds to a \
             current group member; nothing to add"
                .into(),
        ));
    }
    Ok(key_packages)
}

#[cfg(test)]
mod tests {
    use super::*;
    use openmls::prelude::{MlsMessageBodyIn, MlsMessageIn, WireFormat};

    use crate::encryption::mls_client::INITIAL_KEY_PACKAGE_COUNT;
    use crate::encryption::test_support as ts;

    const DAVE_USER: &str = "1a2b3c4d-eeee-ffff-0000-111122223333@wire.com";

    fn dave_key_package() -> Vec<u8> {
        let dave = MlsClient::new_fresh(DAVE_USER, "0011223344556677").expect("dave MlsClient");
        dave.generate_key_packages(1)
            .expect("generate_key_packages")
            .into_iter()
            .next()
            .expect("one KeyPackage")
    }

    fn member_user_ids(client: &MlsClient, group_id: &[u8]) -> std::collections::HashSet<String> {
        client
            .group_leaves(group_id)
            .expect("group_leaves")
            .into_iter()
            .map(|l| l.user_id)
            .collect()
    }

    /// Step 95c: a group an old build persisted with openmls's default
    /// `PURE_CIPHERTEXT` policy is runtime state of an existing
    /// `mls.db` — committing to it used to hit a `debug_assert!`. Wire
    /// rejects `PrivateMessage` commits, so it is refused as
    /// `InvalidInput` (re-join the room).
    #[test]
    fn a_pure_ciphertext_group_refuses_a_commit() {
        use openmls::group::{MlsGroupCreateConfig, PURE_CIPHERTEXT_WIRE_FORMAT_POLICY};

        let alice = MlsClient::new_fresh(
            "a11ce000-0000-4000-8000-000000000001@wire.com",
            "a11ce00000000001",
        )
        .expect("alice MlsClient");
        let group_id = b"legacy-pure-ciphertext";
        let config = MlsGroupCreateConfig::builder()
            .ciphersuite(crate::encryption::mls_client::MLS_CIPHERSUITE)
            .use_ratchet_tree_extension(true)
            .wire_format_policy(PURE_CIPHERTEXT_WIRE_FORMAT_POLICY)
            .build();
        MlsGroup::new_with_group_id(
            &alice.provider().autocommit_provider(),
            alice.signature_keys(),
            &config,
            GroupId::from_slice(group_id),
            alice.credential_with_key().clone(),
        )
        .expect("legacy group");

        let err = alice
            .add_members_commit(group_id, &[dave_key_package()])
            .expect_err("no commit into a PURE_CIPHERTEXT group");

        assert!(
            matches!(err, KupferdrahtError::InvalidInput(_)),
            "got {err:?}"
        );
    }

    /// Step 71 (gap-B.20): a queued backend external Remove targeting
    /// a peer must ride the user's Add-commit — clearing it silently
    /// drops Step 28's commit obligation.
    #[test]
    fn add_commit_carries_pending_peer_remove() {
        let (alice, group_id, es) = ts::group_with_alice_and_bob();
        let bob_idx = ts::peer_leaf_index(&alice, &group_id);
        let proposal = ts::external_remove_proposal(&alice, &group_id, &es, bob_idx);
        alice
            .process_inbound_message(&group_id, &proposal)
            .expect("queue the external remove via the real inbound path");

        alice
            .add_members_commit(&group_id, &[dave_key_package()])
            .expect("add-commit with a queued peer remove");
        alice.merge_pending_commit(&group_id).expect("merge");

        let users = member_user_ids(&alice, &group_id);
        assert!(users.contains(DAVE_USER), "dave must be added");
        assert!(
            !users.contains(ts::BOB_USER),
            "the queued external Remove must ride the add-commit \
             (Step 28 obligation) — not be silently cleared"
        );
        assert!(users.contains(ts::ALICE_USER), "the committer remains");
    }

    /// Step 71 guard: a queued external Remove targeting OUR OWN leaf
    /// must still be cleared before the commit — committing it trips
    /// `CannotRemoveSelf` (live-confirmed). Pins the
    /// pre-Step-71 behavior the selective clear must preserve.
    #[test]
    fn add_commit_still_clears_pending_self_remove() {
        let (alice, group_id, es) = ts::group_with_alice_and_bob();
        let own_idx = ts::own_leaf_index(&alice, &group_id);
        let proposal = ts::external_remove_proposal(&alice, &group_id, &es, own_idx);
        alice
            .process_inbound_message(&group_id, &proposal)
            .expect("queue the self-targeting external remove");

        alice
            .add_members_commit(&group_id, &[dave_key_package()])
            .expect("add-commit must not trip CannotRemoveSelf");
        alice.merge_pending_commit(&group_id).expect("merge");

        let users = member_user_ids(&alice, &group_id);
        assert!(users.contains(DAVE_USER), "dave must be added");
        assert!(
            users.contains(ts::ALICE_USER),
            "the self-remove was cleared, not committed — alice stays \
             until the backend re-issues at the next epoch"
        );
        assert!(
            users.contains(ts::BOB_USER),
            "an uninvolved peer must survive the self-remove clear — \
             an over-aggressive selection that evicts bob would \
             otherwise stay green"
        );
    }

    /// Standard P-256 backend-removal-key placeholder used by every
    /// `create_group` call in this module's tests. Group construction
    /// only checks length; signature verification only triggers if a
    /// Remove proposal arrives — the tests don't exercise that path.
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

    /// Drive Alice through `create_group` so the group is in her
    /// openmls storage, then return its raw `group_id` bytes.
    fn alice_group_id(alice: &MlsClient) -> Vec<u8> {
        let group_id = b"kupferdraht-test-group-id-6b".to_vec();
        alice
            .create_group(&group_id, &REMOVAL_KEY_PLACEHOLDER)
            .expect("alice create_group");
        group_id
    }

    /// Generate one TLS-serialized `KeyPackage` for `client`.
    fn one_key_package_bytes(client: &MlsClient) -> Vec<u8> {
        client
            .generate_key_packages(1)
            .expect("generate_key_packages")
            .into_iter()
            .next()
            .expect("at least one KeyPackage")
    }

    #[test]
    fn add_members_commit_errors_when_group_not_in_storage() {
        let alice = alice_client();
        let bob = bob_client();
        let bob_kp = one_key_package_bytes(&bob);
        // No `create_group` was called on Alice — the group id is
        // unknown to her storage.
        let err = alice
            .add_members_commit(b"never-stored", &[bob_kp])
            .expect_err("must fail without prior create_group");
        assert!(
            matches!(err, KupferdrahtError::InvalidInput(_)),
            "expected InvalidInput, got {err:?}"
        );
    }

    #[test]
    fn add_members_commit_errors_on_empty_key_package_list() {
        let alice = alice_client();
        let group_id = alice_group_id(&alice);
        let err = alice
            .add_members_commit(&group_id, &[])
            .expect_err("empty list must fail");
        assert!(
            matches!(err, KupferdrahtError::InvalidInput(_)),
            "expected InvalidInput, got {err:?}"
        );
    }

    #[test]
    fn add_members_commit_errors_on_malformed_key_package_bytes() {
        let alice = alice_client();
        let group_id = alice_group_id(&alice);
        let err = alice
            .add_members_commit(&group_id, &[vec![0xFFu8; 4]])
            .expect_err("garbage KeyPackage must fail");
        assert!(
            matches!(err, KupferdrahtError::Encoding(_)),
            "expected Encoding, got {err:?}"
        );
    }

    #[test]
    fn add_members_commit_returns_three_segment_bundle() {
        let alice = alice_client();
        let group_id = alice_group_id(&alice);
        let bob = bob_client();
        let bob_kp = one_key_package_bytes(&bob);

        let bundle = alice
            .add_members_commit(&group_id, &[bob_kp])
            .expect("add_members_commit must succeed");

        // Segment 1: commit, must be PublicMessage (Wire's parser
        // requires this — group_create.rs sets MIXED_PLAINTEXT, so
        // handshake commits are PublicMessage).
        let (commit_msg, rest) =
            MlsMessageIn::tls_deserialize_bytes(&bundle).expect("commit segment must parse");
        assert_eq!(
            commit_msg.wire_format(),
            WireFormat::PublicMessage,
            "add-commit must be PublicMessage under MIXED_PLAINTEXT"
        );

        // Segment 2: group_info envelope.
        let (gi_msg, rest) =
            MlsMessageIn::tls_deserialize_bytes(rest).expect("group_info segment must parse");
        assert_eq!(
            gi_msg.wire_format(),
            WireFormat::GroupInfo,
            "second segment must be MlsMessage(GroupInfo)"
        );
        assert!(matches!(gi_msg.extract(), MlsMessageBodyIn::GroupInfo(_)));

        // Segment 3: welcome envelope.
        let (welcome_msg, trailing) =
            MlsMessageIn::tls_deserialize_bytes(rest).expect("welcome segment must parse");
        assert_eq!(
            welcome_msg.wire_format(),
            WireFormat::Welcome,
            "third segment must be MlsMessage(Welcome)"
        );
        assert!(
            trailing.is_empty(),
            "bundle must contain exactly three MlsMessage segments"
        );
    }

    #[test]
    fn add_members_commit_stages_pending_commit_until_merge() {
        // After add_members_commit the local group has a *pending*
        // commit; merging it advances the local epoch from 0 to 1
        // and the leaf count from 1 to 2.
        let alice = alice_client();
        let group_id = alice_group_id(&alice);
        let bob = bob_client();
        let bob_kp = one_key_package_bytes(&bob);

        let _ = alice
            .add_members_commit(&group_id, &[bob_kp])
            .expect("add_members_commit");

        // Before merge: alice is the only leaf the group enumerates.
        let leaves_before = alice.group_leaves(&group_id).expect("leaves before merge");
        assert_eq!(
            leaves_before.len(),
            1,
            "before merge_pending_commit alice's tree still shows only herself"
        );

        alice
            .merge_pending_commit(&group_id)
            .expect("merge_pending_commit");

        let leaves_after = alice.group_leaves(&group_id).expect("leaves after merge");
        let user_ids: std::collections::HashSet<&str> =
            leaves_after.iter().map(|l| l.user_id.as_str()).collect();
        assert!(
            user_ids.contains("a11ce000-0000-4000-8000-000000000001@wire.com"),
            "post-merge tree must include alice"
        );
        assert!(
            user_ids.contains("8d1f2c34-aaaa-bbbb-cccc-dddddddddddd@wire.com"),
            "post-merge tree must include bob"
        );
    }

    #[test]
    fn add_members_commit_supports_multiple_key_packages_in_one_commit() {
        // A single Add-commit can carry multiple Add proposals — the
        // wire-shape Kupferdraht uses when `claim_key_packages` returns
        // one KeyPackage per *device* of the invitee (each device
        // has its own signing key). Two KPs from the same MlsClient
        // share the same signature key and trip MLS's
        // `DuplicateSignatureKey` validation, so the fixture below
        // builds two distinct MlsClients to mirror "two devices".
        let alice = alice_client();
        let group_id = alice_group_id(&alice);
        let bob_device_1 = MlsClient::new_fresh(
            "8d1f2c34-aaaa-bbbb-cccc-dddddddddddd@wire.com",
            "bob-device-1",
        )
        .expect("bob device 1 MlsClient");
        let bob_device_2 = MlsClient::new_fresh(
            "8d1f2c34-aaaa-bbbb-cccc-dddddddddddd@wire.com",
            "bob-device-2",
        )
        .expect("bob device 2 MlsClient");
        let kps = vec![
            one_key_package_bytes(&bob_device_1),
            one_key_package_bytes(&bob_device_2),
        ];
        // Sanity check that the per-device claim count stays well
        // below the per-client pool that `INITIAL_KEY_PACKAGE_COUNT`
        // dictates.
        assert!(kps.len() < INITIAL_KEY_PACKAGE_COUNT);

        let bundle = alice
            .add_members_commit(&group_id, &kps)
            .expect("multi-KP add-commit must succeed");
        assert!(!bundle.is_empty(), "bundle bytes must be produced");
        // Merging is what actually exercises the multi-add path —
        // openmls validates leaf-key uniqueness at merge time.
        alice
            .merge_pending_commit(&group_id)
            .expect("merge multi-add commit");
        // Post-merge: alice + 2 bob devices, but `group_leaves`
        // dedupes by user_id, so the public-facing list is 2 users.
        let leaves = alice.group_leaves(&group_id).expect("leaves");
        let user_ids: std::collections::HashSet<&str> =
            leaves.iter().map(|l| l.user_id.as_str()).collect();
        assert_eq!(user_ids.len(), 2, "alice + bob (deduped across devices)");
    }

    #[test]
    fn add_members_commit_produces_public_message_commit() {
        // The MIXED_PLAINTEXT wire-format invariant (set by
        // `create_group`) makes the Add-commit ship as `PublicMessage`,
        // which Wire's commit-bundle parser requires (observations.md §28).
        // No backfill is involved — the invariant holds at group entry.
        let alice = alice_client();
        let group_id = alice_group_id(&alice);
        let bob = bob_client();
        let bob_kp = one_key_package_bytes(&bob);
        let bundle = alice
            .add_members_commit(&group_id, &[bob_kp])
            .expect("add_members_commit must succeed");
        let (commit_msg, _rest) =
            MlsMessageIn::tls_deserialize_bytes(&bundle).expect("commit segment must parse");
        assert_eq!(
            commit_msg.wire_format(),
            WireFormat::PublicMessage,
            "Add-commit must be PublicMessage under MIXED_PLAINTEXT"
        );
    }
}
