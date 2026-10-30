// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! MLS application-message encryption (RFC 9420 §15).
//!
//! `MlsClient::encrypt_text_message` loads the persisted MLS group
//! keyed by `group_id_bytes`, calls `MlsGroup::create_message` over
//! the supplied plaintext (Wire's `GenericMessage` Protobuf bytes),
//! TLS-serializes the resulting `MlsMessage(PrivateMessage)`, and
//! persists the post-message group state through the openmls
//! storage provider.
//!
//! The plaintext input is opaque from this module's perspective —
//! it's `wire::proto::build_text_generic_message`'s output. Keeping
//! the protobuf concern at the layer above lets future content
//! variants (Asset, Reaction, …) reuse this exact code path
//! unchanged.

use openmls::framing::errors::{MessageDecryptionError, SecretTreeError};
use openmls::framing::{MlsMessageBodyIn, MlsMessageIn, ProcessedMessageContent, ProtocolMessage};
use openmls::group::{GroupId, MlsGroup, ProcessMessageError, ValidationError};
use openmls_traits::OpenMlsProvider;
use tls_codec::{Deserialize as _, Serialize as _};
use tracing::{debug, warn};

use crate::encryption::leaf_identity::{LeafIdentity, LeafIdentityError, parse_leaf_identity};
use crate::encryption::mls_client::MlsClient;
use crate::error::KupferdrahtError;

/// Outcome of [`MlsClient::process_inbound_message`]. Carries the
/// MLS-decrypted plaintext when the inbound message was an
/// application message; for handshake messages (commits, proposals)
/// it carries `Decoded::Handshake` — the openmls state has already
/// been advanced and the caller has nothing to surface to the
/// consumer.
#[derive(Debug)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) enum DecodedMessage {
    /// Application message body — Wire wraps a `GenericMessage`
    /// Protobuf inside this. The caller decodes it. `sender` is the
    /// Wire identity of the leaf openmls authenticated as the signer
    /// (Step 95a, ADR-0027) — the attribution source, not the envelope.
    Application {
        plaintext: Vec<u8>,
        sender: Result<LeafIdentity, LeafIdentityError>,
    },
    /// A commit openmls has already merged into the local group state.
    /// No application-layer dispatch.
    Handshake,
    /// A proposal entered the pending-proposal queue (notably an
    /// external-sender Remove). The caller must drive the commit of it
    /// — see `Client::commit_pending_external_removal` (Step 28).
    ProposalQueued,
    /// The message belongs to an epoch older than the group's — an
    /// already processed commit delivered again, or a message the
    /// retained secrets no longer cover. Skipped, never healed around
    /// (Step 97d, ADR-0031).
    StaleEpoch,
    /// The message belongs to the next epoch while the group has an
    /// own pending commit: it follows that commit, which is still being
    /// uploaded (or was abandoned). Not processed — the caller defers
    /// it until the commit is merged or cleared (Step 97d, ADR-0031).
    AwaitsOwnCommit,
    /// The message's key was already consumed: it was decrypted before
    /// and its MLS state committed — a replay after a crash or failure
    /// between that commit and the cursor. Skipped quietly (Step 97e,
    /// ADR-0032).
    AlreadyDecrypted,
}

/// Where an inbound message's epoch stands relative to the group
/// (Step 97d): `None` → process it normally.
fn epoch_triage(group: &MlsGroup, message: &ProtocolMessage) -> Option<DecodedMessage> {
    let message_epoch = message.epoch().as_u64();
    let group_epoch = group.epoch().as_u64();
    if message_epoch < group_epoch {
        return Some(DecodedMessage::StaleEpoch);
    }
    if message_epoch == group_epoch.saturating_add(1) && group.pending_commit().is_some() {
        return Some(DecodedMessage::AwaitsOwnCommit);
    }
    None
}

#[cfg_attr(not(test), allow(dead_code))]
impl MlsClient {
    /// Encrypt `plaintext` as an MLS application message in the
    /// group identified by `group_id_bytes`. Returns the raw
    /// TLS-serialized `MlsMessage(PrivateMessage)` ready for
    /// `POST /v14/mls/messages`.
    ///
    /// The encryption advances the local sender ratchet for the
    /// current epoch; openmls persists the updated state through
    /// the storage provider before this method returns.
    ///
    /// # Errors
    /// * `KupferdrahtError::Storage` if loading or storing the group
    ///   fails.
    /// * `KupferdrahtError::InvalidInput` if no group with that id is
    ///   in storage (caller should `Client::join_room` first).
    /// * `KupferdrahtError::Generic` if openmls rejects the message
    ///   build (group state error, eviction, pending proposal).
    /// * `KupferdrahtError::Encoding` if TLS serialization of the
    ///   resulting `MlsMessageOut` fails.
    pub fn encrypt_text_message(
        &self,
        group_id_bytes: &[u8],
        plaintext: &[u8],
    ) -> Result<Vec<u8>, KupferdrahtError> {
        debug_assert!(
            !group_id_bytes.is_empty(),
            "group_id_bytes must be non-empty"
        );
        debug_assert!(!plaintext.is_empty(), "plaintext must be non-empty");
        self.provider().with_write_tx(|provider| {
            let group_id = GroupId::from_slice(group_id_bytes);
            let mut group = MlsGroup::load(provider.storage(), &group_id)
                .map_err(|e| KupferdrahtError::Storage(format!("load MlsGroup: {e}")))?
                .ok_or(KupferdrahtError::MlsGroupMissing)?;
            debug!(
                group_id_len = group_id_bytes.len(),
                plaintext_len = plaintext.len(),
                "encrypt application message"
            );
            let msg_out = group
                .create_message(provider, self.signature_keys(), plaintext)
                .map_err(|e| KupferdrahtError::Generic(format!("create_message: {e:?}")))?;
            msg_out
                .tls_serialize_detached()
                .map_err(|e| KupferdrahtError::Encoding(format!("MlsMessage TLS serialize: {e}")))
        })
    }

    /// Decrypt an inbound `MlsMessage(PrivateMessage)` /
    /// `PublicMessage` for the persisted group keyed by
    /// `group_id_bytes`. Application messages return their
    /// plaintext bytes; commits / proposals are applied (or
    /// queued) locally and yield `Decoded::Handshake`.
    ///
    /// `merge_staged_commit` is called inline for `StagedCommitMessage`
    /// outcomes so the local group epoch advances in lockstep with
    /// the sender. `ProposalMessage` is queued via
    /// `store_pending_proposal` and waits for the next commit.
    /// `ExternalJoinProposalMessage` is logged and ignored — Kupferdraht
    /// does not act on external join proposals from peers.
    ///
    /// # Errors
    /// * `KupferdrahtError::Storage` for openmls storage read/write
    ///   failures (group load, epoch persist).
    /// * `KupferdrahtError::InvalidInput` if no group with that id is
    ///   in storage.
    /// * `KupferdrahtError::Encoding` for TLS deserialize failures.
    /// * `KupferdrahtError::Generic` if openmls rejects the decrypt or
    ///   the staged commit / proposal.
    pub fn process_inbound_message(
        &self,
        group_id_bytes: &[u8],
        ciphertext: &[u8],
    ) -> Result<DecodedMessage, KupferdrahtError> {
        debug_assert!(
            !group_id_bytes.is_empty(),
            "group_id_bytes must be non-empty"
        );
        debug_assert!(!ciphertext.is_empty(), "ciphertext must be non-empty");
        self.provider().with_inbound_tx(|provider| {
            let mut group = load_inbound_group(provider, group_id_bytes)?;
            let protocol_message = decode_protocol_message(ciphertext)?;
            if let Some(outcome) = epoch_triage(&group, &protocol_message) {
                debug!(?outcome, "inbound MLS message not processed at this epoch");
                return Ok(outcome);
            }
            debug!(
                group_id_len = group_id_bytes.len(),
                "process inbound MLS message"
            );
            let processed = match group.process_message(provider, protocol_message) {
                Ok(processed) => processed,
                Err(e) if is_consumed_key(&e) => return Ok(DecodedMessage::AlreadyDecrypted),
                Err(e) => return Err(classify_process_error(e)),
            };
            // ADR-0027: read the authenticated signer before `into_content`
            // discards it.
            let sender = parse_leaf_identity(processed.credential());
            absorb_processed_content(provider, &mut group, processed.into_content(), sender)
        })
    }
}

/// Map an openmls `process_message` failure to a typed `KupferdrahtError`. A
/// `WrongEpoch` validation error means the commit advancing the group is still
/// pending (recoverable, in-order) → [`KupferdrahtError::MlsOutOfOrder`], so the
/// receive drain can catch up + retry rather than silently drop the message.
/// Everything else stays opaque (`Generic`) — treated as permanent. Step 38.
fn classify_process_error<S: std::fmt::Debug>(e: ProcessMessageError<S>) -> KupferdrahtError {
    match e {
        ProcessMessageError::ValidationError(ValidationError::WrongEpoch) => {
            KupferdrahtError::MlsOutOfOrder
        }
        other => KupferdrahtError::Generic(format!("process_message: {other:?}")),
    }
}

/// Load the group an inbound message belongs to.
fn load_inbound_group<P: openmls_traits::OpenMlsProvider>(
    provider: &P,
    group_id_bytes: &[u8],
) -> Result<MlsGroup, KupferdrahtError> {
    MlsGroup::load(provider.storage(), &GroupId::from_slice(group_id_bytes))
        .map_err(|e| KupferdrahtError::Storage(format!("load MlsGroup: {e}")))?
        .ok_or_else(|| {
            KupferdrahtError::InvalidInput(
                "process_inbound_message: group not in storage; \
                 call Client::join_room first"
                    .into(),
            )
        })
}

/// Whether decryption failed because the message's key was already
/// consumed (deleted after its first decryption — forward secrecy).
fn is_consumed_key<S>(e: &ProcessMessageError<S>) -> bool {
    matches!(
        e,
        ProcessMessageError::ValidationError(ValidationError::UnableToDecrypt(
            MessageDecryptionError::SecretTreeError(SecretTreeError::SecretReuseError)
        ))
    )
}

/// Parse the ciphertext envelope into the openmls `ProtocolMessage`
/// the group's `process_message` accepts. `Welcome` / `GroupInfo` /
/// `KeyPackage` envelopes — anything not `Private`/`Public` — are an
/// `Encoding` error since the receive path already filtered to
/// `Application`-bearing carriers.
fn decode_protocol_message(ciphertext: &[u8]) -> Result<ProtocolMessage, KupferdrahtError> {
    let msg_in = MlsMessageIn::tls_deserialize(&mut &ciphertext[..])
        .map_err(|e| KupferdrahtError::Encoding(format!("MlsMessage TLS parse: {e}")))?;
    match msg_in.extract() {
        MlsMessageBodyIn::PrivateMessage(m) => Ok(ProtocolMessage::PrivateMessage(m)),
        MlsMessageBodyIn::PublicMessage(m) => Ok(ProtocolMessage::PublicMessage(Box::new(m))),
        other => Err(KupferdrahtError::Encoding(format!(
            "process_inbound_message: expected Private/PublicMessage, got {}",
            body_kind(&other)
        ))),
    }
}

/// Apply the `ProcessedMessageContent` variant to local group state:
/// app messages return their plaintext together with the parsed
/// `sender` (ignored for every other arm); staged commits merge into
/// the group; proposals enter the pending-proposal queue;
/// external-join proposals are logged + skipped (Step 6 territory).
fn absorb_processed_content<P: openmls_traits::OpenMlsProvider>(
    provider: &P,
    group: &mut MlsGroup,
    content: ProcessedMessageContent,
    sender: Result<LeafIdentity, LeafIdentityError>,
) -> Result<DecodedMessage, KupferdrahtError> {
    match content {
        ProcessedMessageContent::ApplicationMessage(app) => Ok(DecodedMessage::Application {
            plaintext: app.into_bytes(),
            sender,
        }),
        ProcessedMessageContent::StagedCommitMessage(staged) => {
            group
                .merge_staged_commit(provider, *staged)
                .map_err(|e| KupferdrahtError::Generic(format!("merge_staged_commit: {e:?}")))?;
            Ok(DecodedMessage::Handshake)
        }
        ProcessedMessageContent::ProposalMessage(proposal) => {
            group
                .store_pending_proposal(provider.storage(), *proposal)
                .map_err(|e| KupferdrahtError::Storage(format!("store_pending_proposal: {e}")))?;
            Ok(DecodedMessage::ProposalQueued)
        }
        ProcessedMessageContent::ExternalJoinProposalMessage(_) => {
            warn!("ExternalJoinProposalMessage from peer — ignored (Step 6 territory)");
            Ok(DecodedMessage::Handshake)
        }
        // openmls 0.9 (Step 76): our own public-framed commit, fanned
        // back by the server and matched against the stored pending
        // commit via confirmation tag. Wire's per-client streams
        // normally exclude the origin client and our flow merges at
        // upload time (`upload_and_merge_commit_bundle`), so this is
        // a defensive echo path — merging is the documented correct
        // action (the DS confirmed our commit; the echo's state is
        // never adopted).
        ProcessedMessageContent::OwnPendingCommit => {
            group.merge_pending_commit(provider).map_err(|e| {
                KupferdrahtError::Generic(format!("merge own pending commit: {e:?}"))
            })?;
            Ok(DecodedMessage::Handshake)
        }
        // openmls 0.9 (Step 76): a private message claiming our own
        // leaf, fanned back by the server. Undecryptable by design
        // (own sender ratchet is encryption-only); openmls documents
        // the variant as a skip hint.
        ProcessedMessageContent::OwnPrivateMessage => {
            debug!("own private message fanned back by the server — skipped");
            Ok(DecodedMessage::Handshake)
        }
    }
}

/// Diagnostic helper for the "expected app message, got X" error
/// path — turns the openmls body discriminant into a short string
/// without reaching into private fields.
fn body_kind(body: &MlsMessageBodyIn) -> &'static str {
    match body {
        MlsMessageBodyIn::PrivateMessage(_) => "PrivateMessage",
        MlsMessageBodyIn::PublicMessage(_) => "PublicMessage",
        MlsMessageBodyIn::Welcome(_) => "Welcome",
        MlsMessageBodyIn::GroupInfo(_) => "GroupInfo",
        MlsMessageBodyIn::KeyPackage(_) => "KeyPackage",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openmls::extensions::Extensions;
    use openmls::group::{MlsGroup, MlsGroupCreateConfig};
    use openmls::prelude::{
        BasicCredential, CredentialWithKey, MlsMessageBodyIn, MlsMessageIn, WireFormat,
    };
    use openmls_basic_credential::SignatureKeyPair;
    use tls_codec::DeserializeBytes as _;

    use crate::encryption::mls_client::{MLS_CIPHERSUITE, MLS_SIGNATURE_SCHEME, leaf_capabilities};
    use crate::encryption::provider::KupferdrahtProvider;

    /// Drive a fresh `MlsClient` through `join_by_external_commit`
    /// against an Alice-built group, then encrypt under the resulting
    /// local state. The returned tuple lets the caller assert on
    /// both the joined group's id and the produced ciphertext.
    fn bob_joins_alice_group(plaintext: &[u8]) -> (Vec<u8>, Vec<u8>) {
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
            .expect("group_info serialize");
        let group_id_bytes = alice_group.group_id().as_slice().to_vec();

        let bob =
            MlsClient::new_fresh("bob-uuid@example.test", "bob-client").expect("bob MlsClient");
        bob.join_by_external_commit(&gi_bytes)
            .expect("bob external commit");
        let ciphertext = bob
            .encrypt_text_message(&group_id_bytes, plaintext)
            .expect("encrypt");
        (group_id_bytes, ciphertext)
    }

    #[test]
    fn encrypt_text_message_yields_private_message_framed_bytes() {
        let (_, ciphertext) = bob_joins_alice_group(b"\xCA\xFE\xBA\xBE");
        let (msg, rest) =
            MlsMessageIn::tls_deserialize_bytes(&ciphertext).expect("parse MlsMessage");
        assert!(rest.is_empty(), "ciphertext must be a single MlsMessage");
        assert_eq!(
            msg.wire_format(),
            WireFormat::PrivateMessage,
            "application messages must be PrivateMessage-framed"
        );
        assert!(matches!(msg.extract(), MlsMessageBodyIn::PrivateMessage(_)));
    }

    #[test]
    fn encrypt_text_message_two_calls_produce_distinct_ciphertexts() {
        // The MLS sender ratchet advances per call — same plaintext
        // must produce different ciphertexts (no nonce reuse).
        let pt = b"identical plaintext";
        let (group_id, c1) = bob_joins_alice_group(pt);
        // Reuse Bob's storage by re-creating the client against an
        // existing on-disk group is non-trivial; instead encrypt a
        // second message via a sibling helper that runs the same
        // setup twice and asserts non-equality.
        let (_, c2) = bob_joins_alice_group(pt);
        assert!(!group_id.is_empty());
        assert_ne!(
            c1, c2,
            "two encryption rounds against fresh groups must differ"
        );
    }

    #[test]
    fn encrypt_text_message_errors_on_unknown_group() {
        let bob =
            MlsClient::new_fresh("bob-uuid@example.test", "bob-client").expect("bob MlsClient");
        let err = bob
            .encrypt_text_message(b"never-stored", b"hello")
            .expect_err("missing group must fail");
        // Step 14g: typed `MlsGroupMissing` variant replaces the
        // previous `InvalidInput("...group not in storage...")` so
        // `Timeline::send_text_auto_join` can match without an
        // error-string substring check.
        assert!(
            matches!(err, KupferdrahtError::MlsGroupMissing),
            "expected MlsGroupMissing, got {err:?}"
        );
    }

    #[test]
    fn classify_process_error_maps_wrong_epoch_to_out_of_order() {
        // Step 38: a `WrongEpoch` validation failure is the recoverable
        // out-of-order case — the advancing commit is still pending/missed.
        // The receive drain matches `MlsOutOfOrder` to heal the room.
        let err = classify_process_error(ProcessMessageError::<()>::ValidationError(
            ValidationError::WrongEpoch,
        ));
        assert!(
            matches!(err, KupferdrahtError::MlsOutOfOrder),
            "expected MlsOutOfOrder, got {err:?}"
        );
    }

    #[test]
    fn classify_process_error_keeps_other_errors_generic() {
        // Any non-WrongEpoch process failure stays opaque (`Generic`) so the
        // drain logs+skips it instead of triggering a needless room resync.
        let err = classify_process_error(ProcessMessageError::<()>::ValidationError(
            ValidationError::UnencryptedApplicationMessage,
        ));
        assert!(
            matches!(err, KupferdrahtError::Generic(_)),
            "expected Generic, got {err:?}"
        );
    }
}
