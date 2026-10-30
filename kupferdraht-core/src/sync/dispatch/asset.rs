// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Inbound `Asset` dispatch — Step 11g.

use std::sync::Arc;

use tracing::{debug, info, warn};

use crate::client::Client;
use crate::error::KupferdrahtError;
use crate::storage::TimelineEventRecord;
use crate::sync::event_handler::{Message, MessageBody};
use crate::sync::service::SyncService;
use crate::time::now_ms;
use crate::timeline::DeliveryStatus;

impl SyncService {
    /// Step 11g: dispatch an inbound `GenericMessage::Asset`
    /// content variant. Replaces the warn-and-skip stub added in
    /// 11a (sync/service.rs `dispatch_event` Asset arm).
    ///
    /// Flow:
    /// 1. AES_GCM gate: log warn, skip persist + callbacks (the
    ///    asset is unconsumable per ADR-0011 §AES-GCM toleration).
    /// 2. `Asset.NotUploaded` (Cancelled / Failed): silently
    ///    discard — the sender's upload never completed.
    /// 3. `Asset.Uploaded(RemoteData)` with `AES_CBC`: pack the
    ///    raw `otr_key` via the per-account DEK envelope; insert
    ///    `timeline_events` + `attachments` rows; notify event
    ///    handlers with `MessageBody::Asset(AssetSummary)`; emit
    ///    `TimelineDiff::Append { kind: Asset(...) }`.
    pub(crate) async fn dispatch_inbound_asset(
        &self,
        client: &Arc<Client>,
        room_id: &str,
        sender_user_id: &str,
        sender_device_id: Option<&str>,
        message_id: &str,
        asset: crate::wire::proto::Asset,
    ) -> Result<(), KupferdrahtError> {
        let Some(decoded) = decode_inbound_asset(message_id, asset)? else {
            // NotUploaded / AES_GCM / no-status — log was done
            // inside the decoder; skip silently here.
            return Ok(());
        };
        let Some(store) = client.store_handle().cloned() else {
            debug!(message_id, "no store attached — skip Asset persist");
            return Ok(());
        };
        let received_at_ms = now_ms();
        let Some((attachment, timeline_record)) = persist_inbound_asset(
            &store,
            client,
            room_id,
            sender_user_id,
            sender_device_id,
            message_id,
            decoded,
            received_at_ms,
        )
        .await?
        else {
            return Ok(());
        };
        let summary = crate::timeline::AssetSummary::from_attachment(
            attachment,
            sender_user_id.to_string(),
            Arc::downgrade(client),
        );
        let message = Message {
            room_id: room_id.to_string(),
            sender_user_id: sender_user_id.to_string(),
            sender_device_id: sender_device_id.map(str::to_string),
            body: MessageBody::Asset(Box::new(summary.clone())),
            received_at_ms,
            message_id: message_id.to_string(),
            reply_to_id: None,
            expires_at_ms: None,
        };
        self.notify_message(&message);
        client.dispatch_timeline_diff(&crate::timeline::TimelineDiff::Append {
            room_id: room_id.to_string(),
            item: asset_timeline_item(&summary, &timeline_record),
        });
        // 25b: auto-acknowledge delivery (1:1 only, best-effort). Reaching
        // here means the row was newly inserted (a self-sent round-trip
        // returns `None` above and never gets here).
        self.queue_delivery_confirmation(room_id, message_id);
        info!(message_id, "inbound Asset dispatched + persisted");
        Ok(())
    }
}

/// Decoded subset of an inbound `Asset` proto sufficient for the
/// receive-path persist step. `None` means "logged + skip"
/// (NotUploaded, AES_GCM, missing status — all surface as
/// drain-continuing no-ops).
struct DecodedAsset {
    original: crate::wire::proto::asset::Original,
    remote: crate::wire::proto::asset::RemoteData,
    asset_id: String,
    encryption_algo: &'static str,
    otr_key_raw: [u8; 32],
}

fn decode_inbound_asset(
    message_id: &str,
    asset: crate::wire::proto::Asset,
) -> Result<Option<DecodedAsset>, KupferdrahtError> {
    let original = asset.original.ok_or_else(|| {
        KupferdrahtError::Encoding(format!(
            "inbound Asset {message_id} missing Original metadata"
        ))
    })?;
    let Some(status) = asset.status else {
        debug!(
            message_id,
            "inbound Asset has no status — skip (NotUploaded or schema drift)"
        );
        return Ok(None);
    };
    let remote = match status {
        crate::wire::proto::asset::Status::Uploaded(r) => r,
        crate::wire::proto::asset::Status::NotUploaded(reason) => {
            debug!(
                message_id,
                reason, "inbound Asset.NotUploaded — sender aborted upload, skip"
            );
            return Ok(None);
        }
    };
    let encryption_algo = match remote.encryption.unwrap_or(0) {
        0 => "AES_CBC",
        1 => "AES_GCM",
        other => {
            warn!(
                message_id,
                encryption = other,
                "inbound Asset has unknown EncryptionAlgorithm; skip"
            );
            return Ok(None);
        }
    };
    if encryption_algo == "AES_GCM" {
        warn!(
            message_id,
            asset_id = ?remote.asset_id,
            "inbound Asset uses AES_GCM — unsupported in V1 (ADR-0011), skip"
        );
        return Ok(None);
    }
    let asset_id = remote.asset_id.clone().ok_or_else(|| {
        KupferdrahtError::Encoding(format!(
            "inbound Asset {message_id} missing RemoteData.asset_id"
        ))
    })?;
    let otr_key_raw: [u8; 32] = remote.otr_key.as_slice().try_into().map_err(|_| {
        KupferdrahtError::Encoding(format!(
            "inbound Asset {message_id} otr_key not 32 bytes (got {})",
            remote.otr_key.len()
        ))
    })?;
    Ok(Some(DecodedAsset {
        original,
        remote,
        asset_id,
        encryption_algo,
        otr_key_raw,
    }))
}

#[allow(clippy::too_many_arguments)]
async fn persist_inbound_asset(
    store: &Arc<dyn crate::storage::Store>,
    _client: &Arc<Client>,
    room_id: &str,
    sender_user_id: &str,
    sender_device_id: Option<&str>,
    message_id: &str,
    decoded: DecodedAsset,
    received_at_ms: i64,
) -> Result<Option<(crate::storage::AttachmentRecord, TimelineEventRecord)>, KupferdrahtError> {
    // The otr_key is stored verbatim in the whole-DB-encrypted
    // `attachments.otr_key` column (the asset content is separately
    // E2E-encrypted under it for Wire).
    let otr_key_ct = decoded.otr_key_raw.to_vec();
    let timeline_record = TimelineEventRecord {
        message_id: message_id.to_string(),
        room_id: room_id.to_string(),
        sender_user_id: sender_user_id.to_string(),
        sender_device_id: sender_device_id.map(str::to_string).unwrap_or_default(),
        kind: "asset".into(),
        body: decoded.original.name.clone().unwrap_or_default(),
        sent_at_ms: received_at_ms,
        delivery_status: DeliveryStatus::Received.as_db_str().into(),
        reply_to_id: None,
        edited_at_ms: None,
        redacted_at_ms: None,
        expires_at_ms: None,
        ephemeral_ttl_ms: None,
    };
    let attachment = crate::storage::AttachmentRecord {
        message_id: message_id.to_string(),
        room_id: room_id.to_string(),
        mime_type: decoded.original.mime_type,
        filename: decoded.original.name,
        size_bytes: decoded.original.size,
        sha256: decoded.remote.sha256,
        otr_key_ct,
        asset_id: decoded.asset_id,
        asset_token: decoded.remote.asset_token,
        asset_domain: decoded.remote.asset_domain.unwrap_or_default(),
        encryption_algo: decoded.encryption_algo.to_string(),
        cache_path: None,
        created_at_ms: received_at_ms,
    };
    // Step 105: the row and its attachment are one write.
    if !store
        .insert_timeline_event_with_attachment(&timeline_record, &attachment)
        .await?
    {
        // Self-sent asset round-trip (or a replay) — the rows exist.
        debug!(
            message_id,
            "inbound Asset already in timeline_events (self-sent round-trip); skip"
        );
        return Ok(None);
    }
    Ok(Some((attachment, timeline_record)))
}

/// Build a `TimelineItem { kind: Asset(summary) }` from the
/// just-persisted record. Crate-private helper for
/// `dispatch_inbound_asset`.
fn asset_timeline_item(
    summary: &crate::timeline::AssetSummary,
    record: &TimelineEventRecord,
) -> crate::timeline::TimelineItem {
    let mut item = crate::timeline::TimelineItem::from_record(record.clone());
    item.kind = crate::timeline::TimelineItemKind::Asset(Box::new(summary.clone()));
    item
}
