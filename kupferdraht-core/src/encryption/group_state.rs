// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Read-only helpers that load a persisted `MlsGroup` and surface
//! application-layer properties derived from it — today just the
//! leaf credential identities consumed by `Room::sync_members`
//! (Step 3d).
//!
//! `MlsClient::group_leaves` deliberately takes the raw `group_id`
//! bytes (not a `GroupId`) so callers don't need to pull the openmls
//! `GroupId` type through the core module boundary.

use openmls::group::{GroupId, MlsGroup};
use openmls_traits::OpenMlsProvider;

use crate::encryption::leaf_identity::{identity_error, parse_leaf_identity};
use crate::encryption::mls_client::MlsClient;
use crate::error::KupferdrahtError;

/// Leaf-level view of a group member at a specific epoch. Kupferdraht's
/// public `RoomMember` is built from this plus a Wire `/list-users`
/// lookup for display info.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GroupLeaf {
    /// Wire `{uuid}@{domain}` user id derived by stripping the
    /// `:{client_id}` middle segment from the leaf's
    /// `BasicCredential` payload.
    pub(crate) user_id: String,
    /// Raw `BasicCredential` identity bytes — kept for diagnostics
    /// (CLI `room-members` prints this alongside the user id).
    #[allow(dead_code)]
    pub(crate) identity: Vec<u8>,
}

#[cfg_attr(not(test), allow(dead_code))]
impl MlsClient {
    /// This device's own Wire user id in `{uuid}@{domain}` form,
    /// derived from the `BasicCredential` identity payload. Same
    /// format `group_leaves` emits per leaf, so it can be compared
    /// for equality to identify the self-member without the caller
    /// threading the domain through from login.
    ///
    /// # Errors
    /// Returns `KupferdrahtError::Encoding` if the credential payload is
    /// not Wire's `{uuid}:{client_id}@{domain}` shape. Should not
    /// happen for `MlsClient` instances built via `new_fresh*` /
    /// `resume_with_storage` — those construct the identity bytes
    /// deterministically.
    pub fn own_wire_user_id(&self) -> Result<String, KupferdrahtError> {
        parse_leaf_identity(&self.credential_with_key().credential)
            .map(|leaf| leaf.user_id)
            .map_err(|e| identity_error("own_wire_user_id", &e))
    }

    /// Load the persisted group from one consistent snapshot (Step
    /// 97c) — a multi-row load never mixes two epochs.
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` if the load fails.
    pub(crate) fn load_group_snapshot(
        &self,
        group_id: &GroupId,
    ) -> Result<Option<MlsGroup>, KupferdrahtError> {
        self.provider().with_read_tx(|provider| {
            MlsGroup::load(provider.storage(), group_id)
                .map_err(|e| KupferdrahtError::Storage(format!("load MlsGroup: {e}")))
        })
    }

    /// `true` when an MLS group with this id has persisted state in
    /// the local openmls storage. Used by `Client::join_all_rooms`
    /// (Step 3e) to skip rooms we've already externally-committed
    /// into without a network round trip.
    ///
    /// # Errors
    /// Returns `KupferdrahtError::Storage` if the storage provider errors
    /// while probing.
    pub fn group_exists(&self, group_id_bytes: &[u8]) -> Result<bool, KupferdrahtError> {
        debug_assert!(
            !group_id_bytes.is_empty(),
            "group_id_bytes must be non-empty"
        );
        let group_id = GroupId::from_slice(group_id_bytes);
        let loaded = self.load_group_snapshot(&group_id)?;
        Ok(loaded.is_some())
    }

    /// The group's current epoch from the persisted openmls state —
    /// the authoritative value after a join (Step 66; the rooms cache
    /// may be arbitrarily stale).
    ///
    /// # Errors
    /// * `KupferdrahtError::Storage` if the group fails to load.
    /// * `KupferdrahtError::InvalidInput` if no group with that id is
    ///   persisted locally.
    pub fn group_epoch(&self, group_id_bytes: &[u8]) -> Result<u64, KupferdrahtError> {
        debug_assert!(
            !group_id_bytes.is_empty(),
            "group_id_bytes must be non-empty"
        );
        let group_id = GroupId::from_slice(group_id_bytes);
        let group = self.load_group_snapshot(&group_id)?.ok_or_else(|| {
            KupferdrahtError::InvalidInput(
                "group_epoch: no MLS group with that id is persisted locally".into(),
            )
        })?;
        Ok(group.epoch().as_u64())
    }

    /// Enumerate the leaves of the locally-persisted MLS group keyed
    /// by `group_id_bytes`. Returns one `GroupLeaf` per leaf node,
    /// deduplicated on `user_id` (a single Wire user may have
    /// multiple devices in the group).
    ///
    /// # Errors
    /// * `KupferdrahtError::Storage` if loading the group from the openmls
    ///   storage provider fails.
    /// * `KupferdrahtError::InvalidInput` if no group with that id is in
    ///   storage.
    /// * `KupferdrahtError::Encoding` if a leaf credential is not UTF-8 or
    ///   does not carry Wire's `{uuid}:{client_id}@{domain}` shape.
    pub fn group_leaves(&self, group_id_bytes: &[u8]) -> Result<Vec<GroupLeaf>, KupferdrahtError> {
        debug_assert!(
            !group_id_bytes.is_empty(),
            "group_id_bytes must be non-empty"
        );
        let group_id = GroupId::from_slice(group_id_bytes);
        let group = self.load_group_snapshot(&group_id)?.ok_or_else(|| {
            KupferdrahtError::InvalidInput(
                "group_leaves: no MLS group with that id is persisted locally".into(),
            )
        })?;
        let mut seen = std::collections::HashSet::<String>::new();
        let mut out = Vec::new();
        for member in group.members() {
            let identity = member.credential.serialized_content().to_vec();
            let user_id = parse_leaf_identity(&member.credential)
                .map_err(|e| identity_error("group_leaves", &e))?
                .user_id;
            if seen.insert(user_id.clone()) {
                out.push(GroupLeaf { user_id, identity });
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_leaves_errors_on_unknown_group() {
        let client = MlsClient::new_fresh(
            "a11ce000-0000-4000-8000-000000000001@wire.com",
            "a11ce00000000001",
        )
        .expect("MlsClient");
        // Nothing was ever committed against this client, so loading
        // any group id must fail cleanly.
        let err = client
            .group_leaves(b"never-stored")
            .expect_err("missing group must fail");
        assert!(
            matches!(err, KupferdrahtError::InvalidInput(_)),
            "expected InvalidInput, got {err:?}"
        );
    }

    #[test]
    fn group_leaves_enumerates_external_commit_joiner() {
        // Drive Bob through `join_by_external_commit` against an
        // Alice-created group, then enumerate his local leaves — the
        // post-join MLS state must list both Alice and Bob.
        use openmls::extensions::Extensions;
        use openmls::group::{MlsGroup, MlsGroupCreateConfig};
        use openmls::prelude::{BasicCredential, CredentialWithKey};
        use openmls_basic_credential::SignatureKeyPair;
        use tls_codec::Serialize as _;

        use crate::encryption::mls_client::{
            MLS_CIPHERSUITE, MLS_SIGNATURE_SCHEME, leaf_capabilities,
        };
        use crate::encryption::provider::KupferdrahtProvider;

        // --- Alice's side -----------------------------------------
        let alice_provider = KupferdrahtProvider::open_in_memory().expect("alice provider");
        let alice_provider = alice_provider.autocommit_provider();
        let alice_signer = SignatureKeyPair::new(MLS_SIGNATURE_SCHEME).expect("alice signer");
        alice_signer
            .store(alice_provider.storage())
            .expect("alice signer store");
        let alice_credential = CredentialWithKey {
            credential: BasicCredential::new(b"alice-uuid:alice-client@example.test".to_vec())
                .into(),
            signature_key: alice_signer.public().to_vec().into(),
        };
        let config = MlsGroupCreateConfig::builder()
            .ciphersuite(MLS_CIPHERSUITE)
            .use_ratchet_tree_extension(true)
            .capabilities(leaf_capabilities())
            .with_group_context_extensions(Extensions::empty())
            .build();
        let alice_group = MlsGroup::new(&alice_provider, &alice_signer, &config, alice_credential)
            .expect("alice group");
        let gi_bytes = alice_group
            .export_group_info(alice_provider.crypto(), &alice_signer, true)
            .expect("export_group_info")
            .tls_serialize_detached()
            .expect("serialize group_info");
        let group_id_bytes = alice_group.group_id().as_slice().to_vec();

        // --- Bob's side -------------------------------------------
        let bob =
            MlsClient::new_fresh("bob-uuid@example.test", "bob-client").expect("bob MlsClient");
        let _bundle = bob
            .join_by_external_commit(&gi_bytes)
            .expect("bob joins alice's group");

        let leaves = bob
            .group_leaves(&group_id_bytes)
            .expect("group_leaves must succeed after join");
        // After an external commit that adds Bob (and inline-removes
        // Alice if identities collided — they don't here), Bob's local
        // tree has exactly Bob as the only member; Alice is still
        // there too at a different leaf index.
        let user_ids: std::collections::HashSet<&str> =
            leaves.iter().map(|l| l.user_id.as_str()).collect();
        assert!(
            user_ids.contains("bob-uuid@example.test"),
            "leaves must include the joiner"
        );
        // Alice's leaf survived because Bob's credential carries a
        // different signature key than Alice's (so no auto-remove).
        assert!(
            user_ids.contains("alice-uuid@example.test"),
            "leaves must include the original group creator"
        );
    }
}
