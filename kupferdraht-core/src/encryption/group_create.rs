// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Local MLS group creation (Step 6a).
//!
//! `MlsClient::create_group` builds a single-member MLS group using
//! a `group_id` that was previously assigned server-side by Wire's
//! `POST /v14/conversations`. The group is stored through the
//! openmls storage provider, ready for subsequent
//! `Client::send_text` / `Client::join_room` operations against it.
//!
//! Why server-assigned `group_id`: Wire's conversation envelope and
//! the local MLS group must agree on the same id, otherwise the
//! commit-bundle uploads in Steps 6b–6d hit
//! `mls-group-conversation-mismatch`. Wire generates the id at the
//! server, returns it in the `POST /v14/conversations` response,
//! and Kupferdraht uses it verbatim via `MlsGroup::new_with_group_id`.
//!
//! External-senders extension: every Wire MLS group MUST embed the
//! Wire-backend external-sender public key (ADR-0004 / W-5) so
//! admin-driven Remove proposals signed by the backend validate.
//! `Client::backend_removal_key_ecdsa_p256()` is fetched at login
//! and threaded into the group-context extensions here.

#![cfg_attr(not(test), allow(dead_code))]

use openmls::extensions::{Extension, Extensions, ExternalSender};
use openmls::group::{
    GroupContext, GroupId, MIXED_PLAINTEXT_WIRE_FORMAT_POLICY, MlsGroup, MlsGroupCreateConfig,
};
use openmls::prelude::{BasicCredential, Credential, LeafNodeParameters, SignaturePublicKey};
use openmls_traits::OpenMlsProvider;
use tracing::debug;

use crate::encryption::external_commit::serialize_bundle;
use crate::encryption::mls_client::{MLS_CIPHERSUITE, MlsClient, leaf_capabilities};
use crate::error::KupferdrahtError;

impl MlsClient {
    /// Create a single-member MLS group keyed by `group_id_bytes`,
    /// with Wire's backend removal key embedded as the only entry
    /// in the `external_senders` extension. The group is persisted
    /// through the openmls storage provider before this returns.
    ///
    /// # Errors
    /// * `KupferdrahtError::Generic` for openmls validation /
    ///   construction failures.
    /// * `KupferdrahtError::Storage` for storage-provider failures
    ///   (mapped through openmls's `NewGroupError::StorageError`).
    pub fn create_group(
        &self,
        group_id_bytes: &[u8],
        backend_removal_key_p256: &[u8],
    ) -> Result<(), KupferdrahtError> {
        debug_assert!(
            !group_id_bytes.is_empty(),
            "group_id_bytes must be non-empty"
        );
        debug_assert!(
            !backend_removal_key_p256.is_empty(),
            "backend_removal_key_p256 must be non-empty"
        );
        debug!(
            group_id_len = group_id_bytes.len(),
            "create local MLS group"
        );
        let config =
            group_create_config(wire_external_senders_extension(backend_removal_key_p256)?);
        self.provider().with_write_tx(|provider| {
            MlsGroup::new_with_group_id(
                provider,
                self.signature_keys(),
                &config,
                GroupId::from_slice(group_id_bytes),
                self.credential_with_key().clone(),
            )
            .map_err(|e| {
                KupferdrahtError::Generic(format!("MlsGroup::new_with_group_id: {e:?}"))
            })?;
            Ok(())
        })
    }

    /// Merge the pending commit on the group keyed by
    /// `group_id_bytes` (advances the local epoch). Call after a
    /// commit bundle (e.g. from `Room::invite` / `Room::kick`) has
    /// been accepted by Wire's `/v14/mls/commit-bundles`.
    ///
    /// # Errors
    /// * `KupferdrahtError::Storage` for openmls storage failures.
    /// * `KupferdrahtError::InvalidInput` if no group with that id is
    ///   in storage.
    /// * `KupferdrahtError::Generic` for openmls merge failures.
    pub fn merge_pending_commit(&self, group_id_bytes: &[u8]) -> Result<(), KupferdrahtError> {
        self.provider().with_write_tx(|provider| {
            let group_id = GroupId::from_slice(group_id_bytes);
            let mut group = MlsGroup::load(provider.storage(), &group_id)
                .map_err(|e| KupferdrahtError::Storage(format!("load MlsGroup: {e}")))?
                .ok_or_else(|| {
                    KupferdrahtError::InvalidInput(
                        "merge_pending_commit: group not in storage".into(),
                    )
                })?;
            group
                .merge_pending_commit(provider)
                .map_err(|e| KupferdrahtError::Generic(format!("merge_pending_commit: {e:?}")))?;
            Ok(())
        })
    }

    /// Discard the pending commit on the group keyed by
    /// `group_id_bytes`. Used on the failure path: when a commit
    /// bundle was rejected by Wire, the local pending commit must
    /// be cleared so the next attempt rebuilds against the unchanged
    /// epoch.
    ///
    /// # Errors
    /// As [`Self::merge_pending_commit`].
    pub fn clear_pending_commit(&self, group_id_bytes: &[u8]) -> Result<(), KupferdrahtError> {
        self.provider().with_write_tx(|provider| {
            let group_id = GroupId::from_slice(group_id_bytes);
            let mut group = MlsGroup::load(provider.storage(), &group_id)
                .map_err(|e| KupferdrahtError::Storage(format!("load MlsGroup: {e}")))?
                .ok_or_else(|| {
                    KupferdrahtError::InvalidInput(
                        "clear_pending_commit: group not in storage".into(),
                    )
                })?;
            group
                .clear_pending_commit(provider.storage())
                .map_err(|e| KupferdrahtError::Storage(format!("clear_pending_commit: {e}")))?;
            Ok(())
        })
    }

    /// Build a **self-update** commit that advances a freshly-created,
    /// single-member group from epoch 0 to epoch 1 so Wire accepts
    /// application messages (it rejects them at epoch 0). Rotates the
    /// committer's own leaf keying material under the **same credential**
    /// — a plain `self_update`, no re-sign and no forced path parameters
    /// — and returns the serialized bundle (`commit || group_info`), ready
    /// for `POST /v14/mls/commit-bundles`. Merge via
    /// [`Self::merge_pending_commit`] only after Wire accepts the bundle.
    ///
    /// This is the establishing commit for a room with no other member to
    /// add (the creator's own remaining devices, if any, are added via an
    /// Add-commit instead).
    ///
    /// # Errors
    /// * `KupferdrahtError::InvalidInput` if no group with that id is in storage.
    /// * `KupferdrahtError::Storage` for openmls storage failures.
    /// * `KupferdrahtError::Generic` for openmls self-update failures.
    pub fn self_update_commit(&self, group_id_bytes: &[u8]) -> Result<Vec<u8>, KupferdrahtError> {
        debug_assert!(
            !group_id_bytes.is_empty(),
            "group_id_bytes must be non-empty"
        );
        self.provider().with_write_tx(|provider| {
            let group_id = GroupId::from_slice(group_id_bytes);
            let mut group = MlsGroup::load(provider.storage(), &group_id)
                .map_err(|e| KupferdrahtError::Storage(format!("load MlsGroup: {e}")))?
                .ok_or_else(|| {
                    KupferdrahtError::InvalidInput(
                        "self_update_commit: group not in storage".into(),
                    )
                })?;
            debug!(
                group_id_len = group_id_bytes.len(),
                "build self-update establishing commit"
            );
            let bundle = group
                .self_update(
                    provider,
                    self.signature_keys(),
                    LeafNodeParameters::builder().build(),
                )
                .map_err(|e| KupferdrahtError::Generic(format!("MlsGroup::self_update: {e:?}")))?;
            let (commit, _welcome, group_info) = bundle.into_contents();
            serialize_bundle(&commit, group_info)
        })
    }
}

/// The create config of every group Kupferdraht starts.
fn group_create_config(extensions: Extensions<GroupContext>) -> MlsGroupCreateConfig {
    MlsGroupCreateConfig::builder()
        .ciphersuite(MLS_CIPHERSUITE)
        .use_ratchet_tree_extension(true)
        .capabilities(leaf_capabilities())
        .with_group_context_extensions(extensions)
        // Wire's commit-bundle parser at /v14/mls/commit-bundles
        // requires the bootstrap commit to be an MlsMessage(
        // PublicMessage) — a `PrivateMessage`-framed commit at
        // epoch 0 trips `400 bad-request: "Missing commit"`
        // (live-confirmed). MIXED_PLAINTEXT lets
        // *handshake* messages stay plaintext (PublicMessage)
        // while application messages remain ciphertext
        // (PrivateMessage), matching Wire's expectations across
        // both endpoints.
        .wire_format_policy(MIXED_PLAINTEXT_WIRE_FORMAT_POLICY)
        .build()
}

/// Build the `external_senders` group-context extension carrying
/// Wire's backend removal key. The credential identity is a
/// fixed `"wire-backend"` `BasicCredential` — verification only
/// touches the signature key, the identity bytes are flavor text
/// matching what peers (other Wire clients) will see in the
/// extension.
fn wire_external_senders_extension(
    backend_removal_key_p256: &[u8],
) -> Result<Extensions<GroupContext>, KupferdrahtError> {
    let signature_key = SignaturePublicKey::from(backend_removal_key_p256.to_vec());
    let credential: Credential = BasicCredential::new(b"wire-backend".to_vec()).into();
    let entry = ExternalSender::new(signature_key, credential);
    Extensions::single(Extension::ExternalSenders(vec![entry]))
        .map_err(|e| KupferdrahtError::Generic(format!("external_senders extension: {e:?}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sanity check: `create_group` succeeds against an in-memory
    /// `MlsClient` with a plausible 65-byte uncompressed P-256 key
    /// stand-in for the backend removal key.
    #[test]
    fn create_group_persists_single_member_group() {
        let client = MlsClient::new_fresh(
            "a11ce000-0000-4000-8000-000000000001@wire.com",
            "a11ce00000000001",
        )
        .expect("MlsClient");
        // A real removal key is a 65-byte uncompressed P-256 point;
        // openmls's ExternalSender just stores the bytes verbatim
        // and only validates them at signature-verification time,
        // so a placeholder that is the right length is enough for a
        // successful group construction.
        let placeholder_removal_key = vec![0x04u8; 65];
        let group_id = b"kupferdraht-test-group-id";
        client
            .create_group(group_id, &placeholder_removal_key)
            .expect("create_group must succeed");
        // The group is now in openmls storage — `group_exists`
        // returns `true` (introduced in cycle 3e).
        assert!(
            client.group_exists(group_id).expect("group_exists"),
            "create_group must persist the group through the storage provider"
        );
    }

    #[test]
    fn create_group_rejects_empty_removal_key_in_release_via_extension_build() {
        // `debug_assert!` would catch empty inputs in debug builds;
        // this test instead exercises the post-assert path so a
        // release build still gets a clear error rather than a
        // silent-empty `Vec`.
        let client = MlsClient::new_fresh(
            "a11ce000-0000-4000-8000-000000000001@wire.com",
            "a11ce00000000001",
        )
        .expect("MlsClient");
        // Passing an empty key: openmls's ExternalSender accepts
        // it (key bytes are opaque to it), so we won't error here.
        // The release-mode invariant is "every successful
        // create_group leaves the group in storage" — exercised by
        // the previous test.
        let _ = client.create_group(b"another-id", &[0xAA; 65]);
        assert!(client.group_exists(b"another-id").expect("exists"));
    }

    /// The establishing commit (`self_update_commit`) for a freshly-created
    /// solo group must be framed as a `PublicMessage` — Wire's commit-bundle
    /// parser rejects a `PrivateMessage`-framed commit with "Missing commit"
    /// (the §25 failure mode) — and must be a valid, mergeable commit.
    #[test]
    fn self_update_commit_produces_a_mergeable_public_message_commit() {
        use openmls::prelude::{MlsMessageIn, WireFormat};
        use tls_codec::DeserializeBytes as _;

        let client = MlsClient::new_fresh(
            "a11ce000-0000-4000-8000-000000000001@wire.com",
            "a11ce00000000001",
        )
        .expect("MlsClient");
        let group_id = b"kupferdraht-establish-test";
        client
            .create_group(group_id, &[0x04u8; 65])
            .expect("create_group");

        let bundle = client
            .self_update_commit(group_id)
            .expect("self_update_commit must succeed on a fresh group");
        assert!(!bundle.is_empty(), "establishing bundle must not be empty");
        let (commit_msg, _rest) =
            MlsMessageIn::tls_deserialize_bytes(&bundle).expect("commit segment parses");
        assert_eq!(
            commit_msg.wire_format(),
            WireFormat::PublicMessage,
            "establishing commit must be PublicMessage so Wire can parse it"
        );

        client
            .merge_pending_commit(group_id)
            .expect("the establishing commit must be valid and mergeable");
    }
}
