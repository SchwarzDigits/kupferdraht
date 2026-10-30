// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Self-resync after device replacement (Step 7e).
//!
//! `kupferdraht-cli login` registers a brand-new Wire client on every
//! invocation — the local MLS leaves left behind by the prior device
//! are unreachable from the new identity (CHANGELOG.md §29). This module
//! provides the MLS-layer primitives that the higher-level
//! `Client::resync_existing_rooms` orchestrator drives to recover.
//!
//! The recovery flow is intentionally simple:
//!
//! 1. [`MlsClient::group_belongs_to_self`] inspects the persisted
//!    group's `own_leaf` credential and reports `true` when it
//!    carries the *current* device's identity. Rooms where the
//!    answer is `false` were joined by an earlier `device_id`.
//! 2. [`MlsClient::discard_group`] deletes the persisted MLS state
//!    for a group via openmls's `MlsGroup::delete`. Used to forget
//!    a stale leaf before the regular external-commit join path
//!    rebuilds the group with the current credential.
//!
//! The actual rejoin uses the unchanged Step-3 code path
//! (`fetch_group_info` → `MlsClient::join_by_external_commit` →
//! `upload_commit_bundle`); resync does not need a different MLS
//! pipeline. Wire's `external_senders` extension stays satisfied
//! across the discard/rejoin because the new external commit
//! re-fetches the live `GroupInfo` each time.

#![cfg_attr(not(test), allow(dead_code))]

use openmls::group::{GroupId, MlsGroup};
use openmls_traits::OpenMlsProvider;

use crate::encryption::mls_client::MlsClient;
use crate::error::KupferdrahtError;

impl MlsClient {
    /// Return `true` when the persisted group's `own_leaf`
    /// credential carries the current device's identity bytes — i.e.
    /// when no resync is needed. Returns `false` when the leaf was
    /// placed by a prior `device_id` of the same `user_id`.
    ///
    /// # Errors
    /// * `KupferdrahtError::InvalidInput` if no group with this id is
    ///   persisted in the local storage.
    /// * `KupferdrahtError::Storage` for openmls storage failures.
    /// * `KupferdrahtError::Generic` if the loaded group has no
    ///   `own_leaf` (an invariant violation — every persisted group
    ///   has a leaf for the local member).
    pub fn group_belongs_to_self(&self, group_id_bytes: &[u8]) -> Result<bool, KupferdrahtError> {
        debug_assert!(
            !group_id_bytes.is_empty(),
            "group_id_bytes must be non-empty"
        );
        self.provider().with_read_tx(|provider| {
            let group_id = GroupId::from_slice(group_id_bytes);
            let group = MlsGroup::load(provider.storage(), &group_id)
                .map_err(|e| KupferdrahtError::Storage(format!("load MlsGroup: {e}")))?
                .ok_or_else(|| {
                    KupferdrahtError::InvalidInput(
                        "group_belongs_to_self: group not in storage".into(),
                    )
                })?;
            let leaf = group.own_leaf().ok_or_else(|| {
                KupferdrahtError::Generic(
                    "MlsGroup has no own_leaf — openmls invariant violation".into(),
                )
            })?;
            let leaf_identity = leaf.credential().serialized_content();
            let self_identity = self.credential_with_key().credential.serialized_content();
            Ok(leaf_identity == self_identity)
        })
    }

    /// Delete the persisted state of the group identified by
    /// `group_id_bytes`. Used to forget a stale leaf before the
    /// regular external-commit join path rebuilds the group with the
    /// current credential.
    ///
    /// # Errors
    /// * `KupferdrahtError::InvalidInput` if no group with this id is
    ///   persisted in the local storage.
    /// * `KupferdrahtError::Storage` for openmls storage failures.
    pub fn discard_group(&self, group_id_bytes: &[u8]) -> Result<(), KupferdrahtError> {
        debug_assert!(
            !group_id_bytes.is_empty(),
            "group_id_bytes must be non-empty"
        );
        self.provider().with_write_tx(|provider| {
            let group_id = GroupId::from_slice(group_id_bytes);
            let mut group = MlsGroup::load(provider.storage(), &group_id)
                .map_err(|e| KupferdrahtError::Storage(format!("load MlsGroup: {e}")))?
                .ok_or_else(|| {
                    KupferdrahtError::InvalidInput("discard_group: group not in storage".into())
                })?;
            group
                .delete(provider.storage())
                .map_err(|e| KupferdrahtError::Storage(format!("delete MlsGroup: {e}")))?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openmls::group::MlsGroup as RawMlsGroup;

    /// Standard P-256 backend-removal-key placeholder used by every
    /// `create_group` call in this module's tests. Group construction
    /// only checks length; signature verification only triggers if a
    /// Remove proposal arrives — the tests don't exercise that path.
    const REMOVAL_KEY_PLACEHOLDER: [u8; 65] = [0x04u8; 65];

    fn alice_client(client_id: &str) -> MlsClient {
        MlsClient::new_fresh("a11ce000-0000-4000-8000-000000000001@wire.com", client_id)
            .expect("alice MlsClient")
    }

    fn alice_group_id(alice: &MlsClient) -> Vec<u8> {
        let group_id = b"kupferdraht-test-group-id-7e".to_vec();
        alice
            .create_group(&group_id, &REMOVAL_KEY_PLACEHOLDER)
            .expect("create_group");
        group_id
    }

    #[test]
    fn group_belongs_to_self_is_true_for_freshly_created_group() {
        let alice = alice_client("a11ce00000000001");
        let group_id = alice_group_id(&alice);
        assert!(
            alice
                .group_belongs_to_self(&group_id)
                .expect("group_belongs_to_self"),
            "a group created by `alice` must report `belongs_to_self=true`"
        );
    }

    #[test]
    fn group_belongs_to_self_errors_when_group_not_in_storage() {
        let alice = alice_client("a11ce00000000001");
        let err = alice
            .group_belongs_to_self(b"never-stored")
            .expect_err("must fail without prior create_group");
        assert!(
            matches!(err, KupferdrahtError::InvalidInput(_)),
            "expected InvalidInput, got {err:?}"
        );
    }

    #[test]
    fn discard_group_removes_persisted_state() {
        // After discard the same `group_belongs_to_self` lookup must
        // surface InvalidInput because the group is no longer in
        // openmls storage. Re-creating under the same id is allowed.
        let alice = alice_client("a11ce00000000001");
        let group_id = alice_group_id(&alice);
        alice.discard_group(&group_id).expect("discard_group");
        let err = alice
            .group_belongs_to_self(&group_id)
            .expect_err("post-discard lookup must fail");
        assert!(
            matches!(err, KupferdrahtError::InvalidInput(_)),
            "expected InvalidInput, got {err:?}"
        );
        // openmls storage is empty for this group_id.
        let provider = alice.provider().autocommit_provider();
        let group_id_obj = GroupId::from_slice(&group_id);
        assert!(
            RawMlsGroup::load(provider.storage(), &group_id_obj)
                .expect("storage probe")
                .is_none(),
            "MlsGroup::load must yield None after discard"
        );
    }

    #[test]
    fn discard_group_errors_when_group_not_in_storage() {
        let alice = alice_client("a11ce00000000001");
        let err = alice
            .discard_group(b"never-stored")
            .expect_err("must fail without prior create_group");
        assert!(
            matches!(err, KupferdrahtError::InvalidInput(_)),
            "expected InvalidInput, got {err:?}"
        );
    }
}
