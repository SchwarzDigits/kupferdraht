// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Per-kind payload reconstruction for a failed-send resend (Step 79):
//! turn a persisted `timeline_events` row back into the `GenericMessage`
//! plaintext the original send produced, under the same `message_id`.
//! Companion of `super::resend`, which owns the orchestration.
//!
//! Text rows re-read their mentions from the `mentions` table and the
//! quote from the persisted parent; an ephemeral row (V28
//! `ephemeral_ttl_ms`) is re-wrapped in Wire's `Ephemeral` envelope so
//! it never leaves as a permanent text; an asset row rebuilds the
//! reference from its `attachments` row (the blob already sits on
//! `/v14/assets`). Task / case / marker rows are refused: they are
//! full-state LWW snapshots and replaying a stale one can regress every
//! receiver's fold (ADR-0014 / ADR-0018 / ADR-0020).

use zeroize::{Zeroize, Zeroizing};

use crate::client::Client;
use crate::error::KupferdrahtError;
use crate::storage::TimelineEventRecord;
use crate::timeline::text::{quote_parent_of, to_proto_mentions};
use crate::timeline::{TimelineItemKind, location_kind_from_body};
use crate::wire::proto::{
    AssetMessageInput, encode_asset_generic_message, encode_ephemeral_text_generic_message,
    encode_knock_generic_message, encode_location_generic_message, encode_text_generic_message,
};

/// The rebuilt plaintext plus the timer to re-arm once it is delivered.
/// Zeroized on drop: an asset reference carries the raw content key.
pub(super) struct ResendPayload {
    pub plaintext: Zeroizing<Vec<u8>>,
    /// `Some(ttl)` for an ephemeral row — the resend stamps
    /// `expires_at_ms = sent_at_ms + ttl` on success.
    pub ttl_ms: Option<i64>,
}

impl ResendPayload {
    fn plain(plaintext: Vec<u8>) -> Self {
        Self {
            plaintext: Zeroizing::new(plaintext),
            ttl_ms: None,
        }
    }
}

/// For a snapshot row kind, the API that re-sends the CURRENT state —
/// the recovery `resend` points at instead of replaying the stale row.
/// `None` for every resendable kind.
pub(super) fn snapshot_recovery_hint(kind: &str) -> Option<&'static str> {
    match kind {
        "task" => Some("update_task"),
        "case" => Some("set_case"),
        "task_list_marker" => Some("set_task_list"),
        _ => None,
    }
}

impl Client {
    /// Rebuild the wire plaintext of a failed row. `InvalidInput` for
    /// kinds `resend` does not cover and for rows whose persisted body
    /// cannot be decoded (runtime data — never a panic).
    pub(super) async fn rebuild_failed_payload(
        &self,
        row: &TimelineEventRecord,
    ) -> Result<ResendPayload, KupferdrahtError> {
        if let Some(recovery) = snapshot_recovery_hint(&row.kind) {
            return Err(KupferdrahtError::InvalidInput(format!(
                "resend: {} is a {} snapshot — replaying it could regress the receivers' \
                 state; re-send the current state via {recovery} instead",
                row.message_id, row.kind
            )));
        }
        match row.kind.as_str() {
            "text" => self.rebuild_text_payload(row).await,
            "knock" => Ok(ResendPayload::plain(encode_knock_generic_message(
                &row.message_id,
                row.body == "true",
            )?)),
            "location" => rebuild_location_payload(row),
            "asset" => self.rebuild_asset_payload(row).await,
            other => Err(KupferdrahtError::InvalidInput(format!(
                "resend: {} has kind {other:?}, which resend does not cover",
                row.message_id
            ))),
        }
    }

    /// Plain / reply / mention text, or — when the row carries a TTL —
    /// the ephemeral envelope (ephemerals carry neither quote nor
    /// mentions, Step 89).
    async fn rebuild_text_payload(
        &self,
        row: &TimelineEventRecord,
    ) -> Result<ResendPayload, KupferdrahtError> {
        if let Some(ttl_ms) = row.ephemeral_ttl_ms {
            let plaintext =
                encode_ephemeral_text_generic_message(&row.message_id, &row.body, ttl_ms)?;
            return Ok(ResendPayload {
                plaintext: Zeroizing::new(plaintext),
                ttl_ms: Some(ttl_ms),
            });
        }
        let parent = match row.reply_to_id.as_deref() {
            Some(parent_id) => Some(self.load_quote_parent(&row.room_id, parent_id).await?),
            None => None,
        };
        let mentions = self
            .load_mentions_for_message(&row.room_id, &row.message_id)
            .await?;
        let plaintext = encode_text_generic_message(
            &row.message_id,
            &row.body,
            parent.as_ref().map(quote_parent_of),
            &to_proto_mentions(&mentions),
        )?;
        Ok(ResendPayload::plain(plaintext))
    }

    /// The `Asset` reference from the persisted `attachments` row — the
    /// blob upload succeeded before the row existed, so only the MLS
    /// message travels again. The raw content key is wiped after use.
    async fn rebuild_asset_payload(
        &self,
        row: &TimelineEventRecord,
    ) -> Result<ResendPayload, KupferdrahtError> {
        let store = self
            .store_handle()
            .ok_or_else(|| KupferdrahtError::InvalidInput("resend: no Store attached".into()))?;
        let mut attachment = store
            .get_attachment(&row.message_id, &row.room_id)
            .await?
            .ok_or_else(|| {
                KupferdrahtError::InvalidInput(format!(
                    "resend: {} is an asset row without an attachment record",
                    row.message_id
                ))
            })?;
        let encoded = encode_asset_generic_message(
            &row.message_id,
            AssetMessageInput {
                mime_type: &attachment.mime_type,
                size: attachment.size_bytes,
                filename: attachment.filename.as_deref(),
                otr_key: &attachment.otr_key_ct,
                sha256: &attachment.sha256,
                asset_id: &attachment.asset_id,
                asset_token: attachment.asset_token.as_deref(),
                asset_domain: &attachment.asset_domain,
            },
        );
        attachment.otr_key_ct.zeroize();
        Ok(ResendPayload::plain(encoded?))
    }
}

/// A location row packs its coordinates as JSON (Step 26).
fn rebuild_location_payload(row: &TimelineEventRecord) -> Result<ResendPayload, KupferdrahtError> {
    let Some(TimelineItemKind::Location {
        latitude,
        longitude,
        name,
        zoom,
    }) = location_kind_from_body(&row.body)
    else {
        return Err(KupferdrahtError::InvalidInput(format!(
            "resend: {} has an unreadable location body",
            row.message_id
        )));
    };
    Ok(ResendPayload::plain(encode_location_generic_message(
        &row.message_id,
        latitude,
        longitude,
        name.as_deref(),
        zoom,
    )?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_kinds_name_their_state_sending_api() {
        assert_eq!(snapshot_recovery_hint("task"), Some("update_task"));
        assert_eq!(snapshot_recovery_hint("case"), Some("set_case"));
        assert_eq!(
            snapshot_recovery_hint("task_list_marker"),
            Some("set_task_list")
        );
        for kind in ["text", "knock", "location", "asset", ""] {
            assert_eq!(snapshot_recovery_hint(kind), None, "{kind:?} is resendable");
        }
    }

    #[test]
    fn an_unreadable_location_body_is_a_recoverable_error() {
        let row = TimelineEventRecord {
            message_id: "loc".into(),
            kind: "location".into(),
            body: "{not json".into(),
            ..TimelineEventRecord::default()
        };
        let err = rebuild_location_payload(&row).err().expect("garbage body");
        assert!(matches!(err, KupferdrahtError::InvalidInput(_)), "{err:?}");
    }
}
