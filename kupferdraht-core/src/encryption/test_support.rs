// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Shared unit-test fixtures for the encryption module (test-only;
//! `#[cfg(test)]` at the `mod` declaration).
//!
//! Extracted from `external_removal.rs`'s test module in Step 71 so
//! the Step-28 external-sender machinery (a group whose
//! `external_senders` extension trusts a forgeable signer, plus
//! forged external Remove proposals) is available to every commit
//! path's tests — the gap-B.20 fix pins Add/Remove-commit behavior
//! against queued external proposals.

use openmls::extensions::SenderExtensionIndex;
use openmls::group::{GroupId, MlsGroup};
use openmls::messages::external_proposals::ExternalProposal;
use openmls::prelude::{BasicCredential, LeafNodeIndex};
use openmls_basic_credential::SignatureKeyPair;
use openmls_traits::OpenMlsProvider;
use tls_codec::Serialize as _;

use crate::encryption::mls_client::{MLS_SIGNATURE_SCHEME, MlsClient};
use crate::encryption::provider::TxProvider;

pub(crate) const ALICE_USER: &str = "a11ce000-0000-4000-8000-000000000001@wire.com";
pub(crate) const BOB_USER: &str = "8d1f2c34-aaaa-bbbb-cccc-dddddddddddd@wire.com";

/// Build a 2-member group (Alice = creator/self, Bob added) whose
/// `external_senders` extension carries `es_signer`'s public key,
/// so an external Remove proposal signed by `es_signer` validates.
/// Returns `(alice_client, group_id, es_signer)`.
pub(crate) fn group_with_alice_and_bob() -> (MlsClient, Vec<u8>, SignatureKeyPair) {
    let es_signer = SignatureKeyPair::new(MLS_SIGNATURE_SCHEME).expect("external-sender signer");
    let alice = MlsClient::new_fresh(ALICE_USER, "a11ce00000000001").expect("alice MlsClient");
    let group_id = b"kupferdraht-test-group-id-28".to_vec();
    alice
        .create_group(&group_id, &es_signer.to_public_vec())
        .expect("create_group");
    add_member(&alice, &group_id, BOB_USER, "deadbeefcafef00d");
    (alice, group_id, es_signer)
}

/// Add one fresh-device member to Alice's group (generate a real
/// `KeyPackage`, Add-commit, merge).
pub(crate) fn add_member(alice: &MlsClient, group_id: &[u8], user_id: &str, client_id: &str) {
    let member = MlsClient::new_fresh(user_id, client_id).expect("member MlsClient");
    let kp = member
        .generate_key_packages(1)
        .expect("generate_key_packages")
        .into_iter()
        .next()
        .expect("one KeyPackage");
    alice
        .add_members_commit(group_id, &[kp])
        .expect("add member");
    alice.merge_pending_commit(group_id).expect("merge add");
}

pub(crate) fn load_raw(alice: &MlsClient, group_id: &[u8]) -> MlsGroup {
    MlsGroup::load(
        alice.provider().autocommit_provider().storage(),
        &GroupId::from_slice(group_id),
    )
    .expect("load group")
    .expect("group present")
}

pub(crate) fn own_leaf_index(alice: &MlsClient, group_id: &[u8]) -> LeafNodeIndex {
    load_raw(alice, group_id).own_leaf_index()
}

/// The (single) peer leaf index — the one member that is not us.
/// Panics when the group has more than one peer (enforced, not just
/// documented — a silent first-match would target an arbitrary leaf
/// after `add_member`); use [`leaf_index_of`] there.
pub(crate) fn peer_leaf_index(alice: &MlsClient, group_id: &[u8]) -> LeafNodeIndex {
    let group = load_raw(alice, group_id);
    let own = group.own_leaf_index();
    let peers: Vec<LeafNodeIndex> = group
        .members()
        .map(|m| m.index)
        .filter(|i| *i != own)
        .collect();
    assert!(
        peers.len() == 1,
        "peer_leaf_index requires exactly one peer, found {} — use leaf_index_of",
        peers.len()
    );
    peers[0]
}

/// Leaf index of the member whose `BasicCredential` identity is
/// `{uuid}:{client}@{domain}` for the given `{uuid}@{domain}`.
pub(crate) fn leaf_index_of(
    alice: &MlsClient,
    group_id: &[u8],
    qualified_user: &str,
) -> LeafNodeIndex {
    let (uuid, domain) = qualified_user
        .split_once('@')
        .expect("qualified `{uuid}@{domain}`");
    let group = load_raw(alice, group_id);
    group
        .members()
        .find(|m| {
            let Ok(basic) = BasicCredential::try_from(m.credential.clone()) else {
                return false;
            };
            let id = String::from_utf8_lossy(basic.identity()).into_owned();
            id.starts_with(&format!("{uuid}:")) && id.ends_with(&format!("@{domain}"))
        })
        .expect("member with that identity present")
        .index
}

pub(crate) fn current_epoch(alice: &MlsClient, group_id: &[u8]) -> u64 {
    load_raw(alice, group_id).epoch().as_u64()
}

/// Forge an external-sender Remove proposal (`PublicMessage`) for
/// `removed`, signed with `es_signer`, at the group's current epoch.
pub(crate) fn external_remove_proposal(
    alice: &MlsClient,
    group_id: &[u8],
    es_signer: &SignatureKeyPair,
    removed: LeafNodeIndex,
) -> Vec<u8> {
    let group = load_raw(alice, group_id);
    let proposal = ExternalProposal::new_remove::<TxProvider<'_>>(
        removed,
        GroupId::from_slice(group_id),
        group.epoch(),
        es_signer,
        SenderExtensionIndex::new(0),
    )
    .expect("new_remove external proposal");
    proposal
        .tls_serialize_detached()
        .expect("serialize external proposal")
}

/// A group of Alice (self) and Bob in which Bob is a real member (joined
/// through the Welcome), so Bob can send member messages. Returns
/// `(alice, bob, group_id)`. Step 106.
pub(crate) fn group_with_alice_and_joined_bob() -> (MlsClient, MlsClient, Vec<u8>) {
    use openmls::prelude::MlsMessageIn;
    use tls_codec::DeserializeBytes as _;

    let es_signer = SignatureKeyPair::new(MLS_SIGNATURE_SCHEME).expect("external-sender signer");
    let alice = MlsClient::new_fresh(ALICE_USER, "a11ce00000000001").expect("alice MlsClient");
    let bob = MlsClient::new_fresh(BOB_USER, "deadbeefcafef00d").expect("bob MlsClient");
    let group_id = b"kupferdraht-test-group-id-106".to_vec();
    alice
        .create_group(&group_id, &es_signer.to_public_vec())
        .expect("create_group");
    let kp = bob
        .generate_key_packages(1)
        .expect("generate_key_packages")
        .into_iter()
        .next()
        .expect("one KeyPackage");
    let bundle = alice.add_members_commit(&group_id, &[kp]).expect("add bob");
    alice.merge_pending_commit(&group_id).expect("merge add");
    // commit || group_info || welcome
    let (_, rest) = MlsMessageIn::tls_deserialize_bytes(&bundle).expect("commit segment");
    let (_, welcome) = MlsMessageIn::tls_deserialize_bytes(rest).expect("group_info segment");
    bob.absorb_welcome(welcome).expect("bob joins");
    (alice, bob, group_id)
}

/// A **member** Remove proposal: `proposer` proposes removing `removed`
/// (Step 106 — not the external sender's).
pub(crate) fn member_remove_proposal(
    proposer: &MlsClient,
    group_id: &[u8],
    removed: LeafNodeIndex,
) -> Vec<u8> {
    let provider = proposer.provider().autocommit_provider();
    let mut group = load_raw(proposer, group_id);
    let (message, _ref) = group
        .propose_remove_member(&provider, proposer.signature_keys(), removed)
        .expect("propose_remove_member");
    message
        .tls_serialize_detached()
        .expect("serialize member proposal")
}
