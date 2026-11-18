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

use super::InboundOrigin;

impl SyncService {
    /// Step 11g: dispatch an inbound `GenericMessage::Asset`
    /// content variant. Replaces the warn-and-skip stub added in
    /// 11a (sync/service.rs `dispatch_event` Attachment arm).
    ///
    /// Flow:
    /// 1. AES_GCM gate: log warn, skip persist + callbacks (the
    ///    attachment is unconsumable per ADR-0011 §AES-GCM toleration).
    /// 2. `Asset.NotUploaded` (Cancelled / Failed): silently
    ///    discard — the sender's upload never completed.
    /// 3. `Asset.Uploaded(RemoteData)` with `AES_CBC`: pack the
    ///    raw `otr_key` via the per-account DEK envelope; insert
    ///    `timeline_events` + `attachments` rows; notify event
    ///    handlers with `MessageBody::Attachment(AttachmentSummary)`; emit
    ///    `TimelineDiff::Append { kind: Attachment(...) }`.
    pub(crate) async fn dispatch_inbound_attachment(
        &self,
        client: &Arc<Client>,
        origin: InboundOrigin<'_>,
        attachment: crate::wire::proto::Asset,
    ) -> Result<(), KupferdrahtError> {
        let InboundOrigin {
            room_id,
            sender_user_id,
            sender_device_id,
            message_id,
            sent_at_ms,
        } = origin;
        let Some(decoded) = decode_inbound_attachment(message_id, attachment)? else {
            // NotUploaded / AES_GCM / no-status — log was done
            // inside the decoder; skip silently here.
            return Ok(());
        };
        let Some(store) = client.store_handle().cloned() else {
            debug!(message_id, "no store attached — skip attachment persist");
            return Ok(());
        };
        let received_at_ms = now_ms();
        let Some((attachment, timeline_record)) =
            persist_inbound_attachment(&store, client, origin, decoded, received_at_ms).await?
        else {
            return Ok(());
        };
        let summary = crate::timeline::AttachmentSummary::from_attachment(
            attachment,
            sender_user_id.to_string(),
            Arc::downgrade(client),
        );
        let message = Message {
            room_id: room_id.to_string(),
            sender_user_id: sender_user_id.to_string(),
            sender_device_id: sender_device_id.map(str::to_string),
            body: MessageBody::Attachment(Box::new(summary.clone())),
            sent_at_ms,
            received_at_ms,
            message_id: message_id.to_string(),
            reply_to_id: None,
            expires_at_ms: None,
        };
        self.notify_message(&message);
        client.dispatch_timeline_diff(&crate::timeline::TimelineDiff::Append {
            room_id: room_id.to_string(),
            item: attachment_timeline_item(&summary, &timeline_record),
        });
        // 25b: auto-acknowledge delivery (1:1 only, best-effort). Reaching
        // here means the row was newly inserted (a self-sent round-trip
        // returns `None` above and never gets here).
        self.queue_delivery_confirmation(room_id, message_id);
        info!(message_id, "inbound attachment dispatched + persisted");
        Ok(())
    }
}

/// Decoded subset of an inbound `Asset` proto sufficient for the
/// receive-path persist step. `None` means "logged + skip"
/// (NotUploaded, AES_GCM, missing status — all surface as
/// drain-continuing no-ops).
struct DecodedAttachment {
    original: crate::wire::proto::asset::Original,
    remote: crate::wire::proto::asset::RemoteData,
    attachment_id: String,
    cipher: &'static str,
    raw_key: [u8; 32],
}

fn decode_inbound_attachment(
    message_id: &str,
    attachment: crate::wire::proto::Asset,
) -> Result<Option<DecodedAttachment>, KupferdrahtError> {
    let original = attachment.original.ok_or_else(|| {
        KupferdrahtError::Encoding(format!(
            "inbound attachment {message_id} missing Original metadata"
        ))
    })?;
    let Some(status) = attachment.status else {
        debug!(
            message_id,
            "inbound attachment has no status — skip (NotUploaded or schema drift)"
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
    let cipher = match remote.encryption.unwrap_or(0) {
        0 => "AES_CBC",
        1 => "AES_GCM",
        other => {
            warn!(
                message_id,
                encryption = other,
                "inbound attachment has unknown EncryptionAlgorithm; skip"
            );
            return Ok(None);
        }
    };
    if cipher == "AES_GCM" {
        warn!(
            message_id,
            attachment_id = ?remote.asset_id,
            "inbound attachment uses AES_GCM — unsupported in V1 (ADR-0011), skip"
        );
        return Ok(None);
    }
    let attachment_id = remote.asset_id.clone().ok_or_else(|| {
        KupferdrahtError::Encoding(format!(
            "inbound attachment {message_id} missing RemoteData.asset_id"
        ))
    })?;
    let raw_key: [u8; 32] = remote.otr_key.as_slice().try_into().map_err(|_| {
        KupferdrahtError::Encoding(format!(
            "inbound attachment {message_id}: the content key is not 32 bytes (got {})",
            remote.otr_key.len()
        ))
    })?;
    Ok(Some(DecodedAttachment {
        original,
        remote,
        attachment_id,
        cipher,
        raw_key,
    }))
}

async fn persist_inbound_attachment(
    store: &Arc<dyn crate::storage::Store>,
    _client: &Arc<Client>,
    origin: InboundOrigin<'_>,
    decoded: DecodedAttachment,
    received_at_ms: i64,
) -> Result<Option<(crate::storage::AttachmentRecord, TimelineEventRecord)>, KupferdrahtError> {
    let InboundOrigin {
        room_id,
        sender_user_id,
        sender_device_id,
        message_id,
        sent_at_ms,
    } = origin;
    // The content key is stored verbatim in the whole-DB-encrypted
    // `attachments.otr_key` column (the attachment content is separately
    // E2E-encrypted under it for Wire).
    let key_ct = decoded.raw_key.to_vec();
    let timeline_record = TimelineEventRecord {
        message_id: message_id.to_string(),
        room_id: room_id.to_string(),
        sender_user_id: sender_user_id.to_string(),
        sender_device_id: sender_device_id.map(str::to_string).unwrap_or_default(),
        kind: "attachment".into(),
        body: decoded.original.name.clone().unwrap_or_default(),
        sent_at_ms,
        arrived_at_ms: received_at_ms,
        delivery_status: DeliveryStatus::Received.as_db_str().into(),
        reply_to_id: None,
        edited_at_ms: None,
        redacted_at_ms: None,
        expires_at_ms: None,
        ttl_ms: None,
    };
    let attachment = crate::storage::AttachmentRecord {
        message_id: message_id.to_string(),
        room_id: room_id.to_string(),
        mime_type: decoded.original.mime_type,
        filename: decoded.original.name,
        size_bytes: decoded.original.size,
        sha256: decoded.remote.sha256,
        key_ct,
        attachment_id: decoded.attachment_id,
        attachment_token: decoded.remote.asset_token,
        attachment_domain: decoded.remote.asset_domain.unwrap_or_default(),
        cipher: decoded.cipher.to_string(),
        cache_path: None,
        created_at_ms: received_at_ms,
    };
    // Step 105: the row and its attachment are one write.
    if !store
        .insert_timeline_event_with_attachment(&timeline_record, &attachment)
        .await?
    {
        // Self-sent attachment round-trip (or a replay) — the rows exist.
        debug!(
            message_id,
            "inbound attachment already in timeline_events (self-sent round-trip); skip"
        );
        return Ok(None);
    }
    Ok(Some((attachment, timeline_record)))
}

/// Build a `TimelineItem { kind: Attachment(summary) }` from the
/// just-persisted record. Crate-private helper for
/// `dispatch_inbound_attachment`.
fn attachment_timeline_item(
    summary: &crate::timeline::AttachmentSummary,
    record: &TimelineEventRecord,
) -> crate::timeline::TimelineItem {
    let mut item = crate::timeline::TimelineItem::from_record(record.clone());
    item.kind = crate::timeline::TimelineItemKind::Attachment(Box::new(summary.clone()));
    item
}
