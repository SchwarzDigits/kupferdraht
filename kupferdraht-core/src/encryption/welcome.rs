// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Receive-side Welcome absorption (Step 69, closes gap-B.24).
//!
//! The send side of an Add lives in `group_invite.rs`; this module
//! is the joiner's half: a claimed device turns the
//! `conversation.mls-welcome` payload into a persisted, operational
//! `MlsGroup` at the Add-commit's epoch — *before* the initiator's
//! first application message arrives, which is exactly the message
//! the pre-Step-69 skip-and-heal path lost to MLS forward secrecy
//! (live-observed, observations §67).

use openmls::group::{MIXED_PLAINTEXT_WIRE_FORMAT_POLICY, MlsGroupJoinConfig, StagedWelcome};
use openmls::prelude::{MlsMessageBodyIn, MlsMessageIn, Welcome};
use tls_codec::Deserialize as _;
use tracing::debug;

use crate::encryption::mls_client::MlsClient;
use crate::error::KupferdrahtError;

impl MlsClient {
    /// Absorb a `conversation.mls-welcome` payload and return the
    /// joined `group_id` (raw bytes).
    ///
    /// Pipeline: TLS-parse the `MlsMessage`, require a `Welcome`
    /// body, stage it (`StagedWelcome` — the join config mirrors
    /// [`join_by_external_commit`](Self::join_by_external_commit):
    /// ratchet-tree extension on, `MIXED_PLAINTEXT` so later
    /// handshake commits stay inspectable by Wire, and leaf-lifetime
    /// validation skipped because peer clients emit lifetimes
    /// outside openmls's acceptance window), then merge via
    /// `into_group` — openmls persists the group in the same step.
    ///
    /// A Welcome whose group **already exists locally** is skipped
    /// (`Ok`, no state change): the external-commit join won the
    /// race, and the older Welcome epoch must never overwrite the
    /// live group. Staging before that check is unavoidable — the
    /// `group_id` sits in the Welcome's encrypted `GroupInfo` — and
    /// consumes the one-time key-package secret either way (openmls
    /// deletes it from storage; a redelivered Welcome therefore
    /// fails as `NoMatchingKeyPackage`, which the sync drain treats
    /// as skip, not as damage). Staging and storing run in one
    /// transaction (Step 97c): a join that fails after staging rolls
    /// back and keeps the key package, so a retry can still join.
    ///
    /// # Errors
    /// * `KupferdrahtError::Encoding` for TLS-parse failures, a non-`Welcome`
    ///   body, an empty payload or an empty group id — wire data, never
    ///   a panic.
    /// * `KupferdrahtError::Generic` for openmls staging/merge failures
    ///   (including the consumed-key-package redelivery case).
    ///   Deliberately **not** `InvalidInput`/`Storage`: the drain's
    ///   room-recovery filter treats those as broken-local-state
    ///   signals, and a garbage Welcome must not be able to trigger
    ///   discard+rejoin churn (Step-64 narrowing).
    pub fn absorb_welcome(&self, welcome_bytes: &[u8]) -> Result<Vec<u8>, KupferdrahtError> {
        let welcome = parse_welcome(welcome_bytes)?;
        let config = MlsGroupJoinConfig::builder()
            .use_ratchet_tree_extension(true)
            .wire_format_policy(MIXED_PLAINTEXT_WIRE_FORMAT_POLICY)
            .build();
        self.provider().with_inbound_tx(|provider| {
            let staged = StagedWelcome::build_from_welcome(provider, &config, welcome)
                .map_err(|e| KupferdrahtError::Generic(format!("absorb_welcome stage: {e:?}")))?
                .skip_lifetime_validation()
                .build()
                .map_err(|e| KupferdrahtError::Generic(format!("absorb_welcome build: {e:?}")))?;
            let group_id = staged.group_context().group_id().as_slice().to_vec();
            // The inviter chooses the group id (it sits in the encrypted
            // GroupInfo) — runtime data, refused as wire damage (Step 95c).
            if group_id.is_empty() {
                return Err(KupferdrahtError::Encoding(
                    "absorb_welcome: the Welcome carries an empty group id".into(),
                ));
            }
            if self.group_exists(&group_id)? {
                debug!(
                    group_id_len = group_id.len(),
                    "absorb_welcome: group already local (external join won the race); skipping"
                );
                return Ok(group_id);
            }
            staged
                .into_group(provider)
                .map_err(|e| KupferdrahtError::Generic(format!("absorb_welcome merge: {e:?}")))?;
            debug!(group_id_len = group_id.len(), "absorb_welcome: joined");
            Ok(group_id)
        })
    }
}

/// Parse a TLS `MlsMessage(Welcome)` — wire data, refused as
/// `Encoding` when empty, malformed or of another type.
fn parse_welcome(welcome_bytes: &[u8]) -> Result<Welcome, KupferdrahtError> {
    if welcome_bytes.is_empty() {
        return Err(KupferdrahtError::Encoding(
            "absorb_welcome: empty welcome payload".into(),
        ));
    }
    let msg = MlsMessageIn::tls_deserialize_exact(welcome_bytes)
        .map_err(|e| KupferdrahtError::Encoding(format!("absorb_welcome TLS parse: {e}")))?;
    let MlsMessageBodyIn::Welcome(welcome) = msg.extract() else {
        return Err(KupferdrahtError::Encoding(
            "absorb_welcome: payload is not MlsMessage(Welcome)".into(),
        ));
    };
    Ok(welcome)
}
