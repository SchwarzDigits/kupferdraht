// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! External-commit join flow (RFC 9420 §12.5).
//!
//! A fresh Kupferdraht device that discovers an existing MLS group via
//! `sync_metadata` is not yet a member of it. Joining requires:
//!
//! 1. Fetching the group's public join material (`GroupInfo`) via
//!    `GET /v14/conversations/{d}/{id}/groupinfo`.
//! 2. Building an external commit locally that adds our leaf to the
//!    group (openmls's `ExternalCommitBuilder`).
//! 3. Uploading the commit bundle to
//!    `POST /v14/mls/commit-bundles` so every other member advances
//!    to the new epoch in lockstep.
//!
//! This module implements step 2. Step 1 lives in
//! [`crate::wire::http::groups`]; step 3 uses the same transport module
//! (`upload_commit_bundle`).
//!
//! ## Leaf capabilities: `[Basic, X509]`
//!
//! We declare the standard Kupferdraht leaf capabilities from
//! [`super::mls_client::leaf_capabilities`] — Basic **and** X509. Groups
//! that have gone through Wire's e2ei enrollment embed a
//! `RequiredCapabilities` extension listing X509; a leaf that only
//! advertises Basic is rejected with `LeafNodeValidationError::
//! UnsupportedCredentials` deep in openmls. See LEARNINGS §1.3.
//!
//! ## Bundle wire format: `commit || group_info` — both MlsMessage-framed
//!
//! Wire's `/mls/commit-bundles` parser reads each segment as a full
//! `MlsMessage` envelope (`ProtocolVersion || WireFormat || body`). The
//! commit segment is already `MlsMessageOut` as emitted by openmls; the
//! `GroupInfo` segment must be wrapped via `MlsMessageOut::from(gi)`
//! before TLS-serialization — raw `GroupInfo` bytes cause Wire to
//! respond with `400 "Unknown ContentType: N"`. See LEARNINGS §1.5.
//!
//! External commits never emit a Welcome (we are joining ourselves, not
//! inviting anyone), so the bundle has exactly two segments.

use openmls::group::{
    ExternalCommitBuilder, MIXED_PLAINTEXT_WIRE_FORMAT_POLICY, MlsGroupJoinConfig,
};
use openmls::messages::group_info::{GroupInfo, VerifiableGroupInfo};
use openmls::prelude::{MlsMessageBodyIn, MlsMessageIn, MlsMessageOut};
use openmls_traits::OpenMlsProvider;
use tls_codec::{Deserialize as _, Serialize as _};
use tracing::debug;

use crate::encryption::mls_client::{MlsClient, leaf_capabilities};
use crate::error::KupferdrahtError;

// The public method and its helpers are exercised by this module's
// unit tests; the core-layer caller (`Client::join_room`) lands in
// cycle 3c. Until then, `cargo build` sees them as unused.
#[cfg_attr(not(test), allow(dead_code))]
impl MlsClient {
    /// Build and locally apply an external commit against the group
    /// described by `group_info_bytes`. On return:
    ///
    /// * The new `MlsGroup` has been persisted through the openmls
    ///   storage provider and is already at `server_epoch + 1` (the
    ///   pending commit is merged inside `finalize`).
    /// * The returned bytes are the commit bundle the caller uploads
    ///   to `POST /v14/mls/commit-bundles` — two concatenated
    ///   TLS-serialized `MlsMessage` segments: the commit
    ///   (`PublicMessage`) followed by the new `GroupInfo`.
    ///
    /// The caller is responsible for reconciling a failed upload: if
    /// `upload_commit_bundle` is rejected by Wire, local state is now
    /// ahead of the server. A future revision may add rollback on top.
    ///
    /// # Errors
    /// * `KupferdrahtError::Encoding` if `group_info_bytes` does not parse
    ///   as an `MlsMessage(GroupInfo)` or if TLS serialization of any
    ///   bundle segment fails.
    /// * `KupferdrahtError::Generic` if openmls rejects any stage of the
    ///   external-commit builder (missing ratchet tree, signature
    ///   failure, unsupported ciphersuite, storage failure, …).
    pub fn join_by_external_commit(
        &self,
        group_info_bytes: &[u8],
    ) -> Result<Vec<u8>, KupferdrahtError> {
        // Backend data (`GET …/groupinfo`), never asserted (Step 95c).
        if group_info_bytes.is_empty() {
            return Err(KupferdrahtError::Encoding(
                "join_by_external_commit: empty GroupInfo".into(),
            ));
        }
        let verifiable_group_info = parse_verifiable_group_info(group_info_bytes)?;
        debug!(
            group_info_len = group_info_bytes.len(),
            "build external commit"
        );
        let (commit_out, group_info) = self.run_external_commit(verifiable_group_info)?;
        serialize_bundle(&commit_out, group_info)
    }

    /// Wraps openmls's `ExternalCommitBuilder` → `build_group` →
    /// `leaf_node_parameters` → `load_psks` → `build` → `finalize`
    /// chain. On success the new group is persisted and merged; the
    /// returned tuple is the commit message and the new `GroupInfo`.
    /// The whole chain runs in one transaction (Step 97c).
    ///
    /// Leaf-lifetime validation is skipped: Wire's other clients (iOS /
    /// desktop / Android) emit leaf lifetimes that don't match openmls's
    /// default acceptance window — joining a pre-existing group then
    /// trips `LeafNodeValidation::Lifetime(NotCurrent)`. The lifetime is
    /// a peer's choice, not a property we can enforce on the joiner
    /// side. The local leaf we add through this commit goes through
    /// openmls's normal validation; only pre-existing leaves are exempt.
    fn run_external_commit(
        &self,
        verifiable_group_info: VerifiableGroupInfo,
    ) -> Result<(MlsMessageOut, Option<GroupInfo>), KupferdrahtError> {
        let config = external_join_config();
        let leaf_params = openmls::treesync::LeafNodeParameters::builder()
            .with_credential_with_key(self.credential_with_key().clone())
            .with_capabilities(leaf_capabilities())
            .build();
        self.provider().with_write_tx(|provider| {
            let (_group, bundle) = ExternalCommitBuilder::new()
                .with_config(config)
                .skip_lifetime_validation()
                .build_group(
                    provider,
                    verifiable_group_info,
                    self.credential_with_key().clone(),
                )
                .map_err(|e| {
                    KupferdrahtError::Generic(format!("external-commit build_group: {e:?}"))
                })?
                .leaf_node_parameters(leaf_params)
                .load_psks(provider.storage())
                .map_err(|e| {
                    KupferdrahtError::Generic(format!("external-commit load_psks: {e:?}"))
                })?
                .build(
                    provider.rand(),
                    provider.crypto(),
                    self.signature_keys(),
                    |_| true,
                )
                .map_err(|e| KupferdrahtError::Generic(format!("external-commit build: {e:?}")))?
                .finalize(provider)
                .map_err(|e| {
                    KupferdrahtError::Generic(format!("external-commit finalize: {e:?}"))
                })?;
            let (commit_out, _welcome_always_none_for_external, group_info) =
                bundle.into_contents();
            Ok((commit_out, group_info))
        })
    }
}

/// The join config of an external commit. `MlsGroupJoinConfig` does
/// not carry a ciphersuite setter — openmls inherits the ciphersuite
/// from the joined group's context, not from the join config. The
/// wire-format policy is openmls's default (`PURE_CIPHERTEXT`) unless
/// overridden; we set `MIXED_PLAINTEXT` so subsequent handshake commits
/// (Add / Remove / self-update) ship as `PublicMessage`. Wire's
/// commit-bundle endpoint can only inspect plaintext commits — a
/// `PrivateMessage` commit lands as `400 bad-request: "Missing commit"`
/// regardless of epoch (observations.md §28). Application messages
/// stay encrypted under `MIXED_PLAINTEXT`, matching what
/// `group_create.rs` chooses for Kupferdraht-created rooms.
fn external_join_config() -> MlsGroupJoinConfig {
    MlsGroupJoinConfig::builder()
        .use_ratchet_tree_extension(true)
        .wire_format_policy(MIXED_PLAINTEXT_WIRE_FORMAT_POLICY)
        .build()
}

#[cfg_attr(not(test), allow(dead_code))]
fn parse_verifiable_group_info(bytes: &[u8]) -> Result<VerifiableGroupInfo, KupferdrahtError> {
    // Wire's `GET /v14/conversations/{d}/{id}/groupinfo` actually
    // returns the raw TLS-serialized `VerifiableGroupInfo` body, not
    // an `MlsMessage(GroupInfo)` envelope (live-validated: the leading
    // byte was `0x20`, not the `0x00 0x01 0x00 0x04` MlsMessage
    // prefix). PLAN.md Step 3 anticipated this fallback.
    //
    // Try MlsMessage first so a future Wire change to envelope-framed
    // delivery still works without a code change; fall back to raw
    // `VerifiableGroupInfo` for the current shape.
    if let Ok(msg) = MlsMessageIn::tls_deserialize(&mut &bytes[..])
        && let MlsMessageBodyIn::GroupInfo(gi) = msg.extract()
    {
        return Ok(gi);
    }
    VerifiableGroupInfo::tls_deserialize(&mut &bytes[..]).map_err(|e| {
        KupferdrahtError::Encoding(format!(
            "parse VerifiableGroupInfo (raw or MlsMessage-framed): {e}"
        ))
    })
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn serialize_bundle(
    commit: &MlsMessageOut,
    group_info: Option<GroupInfo>,
) -> Result<Vec<u8>, KupferdrahtError> {
    serialize_bundle_with_welcome(commit, group_info, None)
}

/// Like [`serialize_bundle`] but also lays down the optional
/// `Welcome` segment. Wire's commit-bundle parser accepts the
/// segment order
/// `commit:MlsMessage || group_info:MlsMessage || welcome?:MlsMessage`.
/// Add-commits supply a Welcome (the new members' epoch keys);
/// external-commit and self-update flows pass `None`.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn serialize_bundle_with_welcome(
    commit: &MlsMessageOut,
    group_info: Option<GroupInfo>,
    welcome: Option<MlsMessageOut>,
) -> Result<Vec<u8>, KupferdrahtError> {
    let mut out = Vec::with_capacity(4096);
    commit
        .tls_serialize(&mut out)
        .map_err(|e| KupferdrahtError::Encoding(format!("commit TLS: {e}")))?;
    let group_info = group_info.ok_or_else(|| {
        KupferdrahtError::Generic(
            "commit bundle produced no GroupInfo \
             (openmls invariant violation, not a Wire error)"
                .into(),
        )
    })?;
    let group_info_msg: MlsMessageOut = group_info.into();
    group_info_msg
        .tls_serialize(&mut out)
        .map_err(|e| KupferdrahtError::Encoding(format!("group_info TLS: {e}")))?;
    if let Some(welcome) = welcome {
        welcome
            .tls_serialize(&mut out)
            .map_err(|e| KupferdrahtError::Encoding(format!("welcome TLS: {e}")))?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use openmls::extensions::{Extension, Extensions, RequiredCapabilitiesExtension};
    use openmls::group::{MlsGroup, MlsGroupCreateConfig};
    use openmls::prelude::{CredentialType, WireFormat};
    use openmls_basic_credential::SignatureKeyPair;
    use tls_codec::DeserializeBytes as _;

    use crate::encryption::mls_client::MLS_CIPHERSUITE;
    use crate::encryption::provider::KupferdrahtProvider;

    /// Qualified Wire user-ID for the joining client. Only used to
    /// derive the `BasicCredential` payload — the group creator and the
    /// joiner both run through the same `MlsClient` constructor, so no
    /// cross-identity plumbing is needed.
    const BOB_USER: &str = "a11ce000-0000-4000-8000-000000000001@wire.com";
    const BOB_CLIENT: &str = "a11ce00000000001";

    /// Creates a standalone MLS group owned by an ad-hoc Alice provider
    /// and exports the TLS-serialized `MlsMessage(GroupInfo)` bytes that
    /// a remote party would receive from Wire. `required_capabilities`
    /// is threaded into the group's context extensions so tests can
    /// exercise the LEARNINGS §1.3 X509 regression case.
    /// Step 95c: `GET …/groupinfo` answered 200 with an empty body used
    /// to reach a `debug_assert!` — backend data. It is an `Encoding`
    /// error like any other unparsable `GroupInfo`.
    #[test]
    fn an_empty_group_info_is_an_encoding_error() {
        let bob = MlsClient::new_fresh(BOB_USER, BOB_CLIENT).expect("bob MlsClient");

        let err = bob
            .join_by_external_commit(&[])
            .expect_err("nothing to join");

        assert!(matches!(err, KupferdrahtError::Encoding(_)), "got {err:?}");
    }

    fn alice_group_info_bytes(required_x509: bool) -> Vec<u8> {
        let alice_provider = KupferdrahtProvider::open_in_memory().expect("alice provider");
        let alice_provider = alice_provider.autocommit_provider();
        let alice_signer =
            SignatureKeyPair::new(crate::encryption::mls_client::MLS_SIGNATURE_SCHEME)
                .expect("alice signer");
        alice_signer
            .store(alice_provider.storage())
            .expect("store alice signer");
        let alice_credential = openmls::prelude::CredentialWithKey {
            credential: openmls::prelude::BasicCredential::new(b"alice@wire.com".to_vec()).into(),
            signature_key: alice_signer.public().to_vec().into(),
        };

        let mut create_config_builder = MlsGroupCreateConfig::builder()
            .ciphersuite(MLS_CIPHERSUITE)
            .use_ratchet_tree_extension(true)
            .capabilities(leaf_capabilities());
        if required_x509 {
            let required = RequiredCapabilitiesExtension::new(&[], &[], &[CredentialType::X509]);
            let gce = Extensions::single(Extension::RequiredCapabilities(required))
                .expect("RequiredCapabilities is valid as a group-context extension");
            create_config_builder = create_config_builder.with_group_context_extensions(gce);
        }
        let config = create_config_builder.build();

        let alice_group = MlsGroup::new(
            &alice_provider,
            &alice_signer,
            &config,
            alice_credential.clone(),
        )
        .expect("alice new group");

        let group_info = alice_group
            .export_group_info(alice_provider.crypto(), &alice_signer, true)
            .expect("export_group_info");
        group_info
            .tls_serialize_detached()
            .expect("group_info TLS serialize")
    }

    fn bob_client() -> MlsClient {
        MlsClient::new_fresh(BOB_USER, BOB_CLIENT).expect("bob MlsClient")
    }

    #[test]
    fn rejects_empty_group_info_bytes() {
        let client = bob_client();
        // debug_assert in `join_by_external_commit` catches empty input
        // in debug builds; in release it would hit the deserialize path
        // and surface as Encoding. We test the release path by using a
        // one-byte slice that fails to parse — the debug_assert only
        // blocks the *empty* case.
        let err = client
            .join_by_external_commit(&[0xFFu8])
            .expect_err("garbage bytes must fail");
        assert!(
            matches!(err, KupferdrahtError::Encoding(_)),
            "expected Encoding, got {err:?}"
        );
    }

    #[test]
    fn produces_two_message_framed_segments_against_plain_group() {
        let client = bob_client();
        let gi_bytes = alice_group_info_bytes(false);
        let bundle = client
            .join_by_external_commit(&gi_bytes)
            .expect("join must succeed");
        assert_bundle_shape_is_commit_then_group_info(&bundle);
    }

    #[test]
    fn joins_group_that_requires_x509_credential_support() {
        // LEARNINGS §1.3 regression — a leaf with default caps (Basic
        // only) would fail here with `UnsupportedCredentials`. Our
        // `leaf_capabilities()` returns [Basic, X509], so the join
        // passes.
        let client = bob_client();
        let gi_bytes = alice_group_info_bytes(true);
        let bundle = client
            .join_by_external_commit(&gi_bytes)
            .expect("X509-required group must accept [Basic, X509] leaf");
        assert_bundle_shape_is_commit_then_group_info(&bundle);
    }

    /// Parses the concatenated bundle bytes as two back-to-back
    /// `MlsMessage` envelopes and asserts the wire-format discriminants:
    /// commit is `PublicMessage`, `GroupInfo` is `GroupInfo`.
    fn assert_bundle_shape_is_commit_then_group_info(bundle: &[u8]) {
        let (commit_msg, rest) =
            MlsMessageIn::tls_deserialize_bytes(bundle).expect("first segment must parse");
        assert_eq!(
            commit_msg.wire_format(),
            WireFormat::PublicMessage,
            "external-commit wire format must be PublicMessage"
        );
        let (gi_msg, trailing) =
            MlsMessageIn::tls_deserialize_bytes(rest).expect("second segment must parse");
        assert_eq!(
            gi_msg.wire_format(),
            WireFormat::GroupInfo,
            "second segment must be an MlsMessage(GroupInfo) envelope"
        );
        assert!(
            matches!(gi_msg.extract(), MlsMessageBodyIn::GroupInfo(_)),
            "second segment body must be GroupInfo"
        );
        assert!(
            trailing.is_empty(),
            "bundle must contain exactly two MlsMessage segments"
        );
    }
}
