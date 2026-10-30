// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Timeline-pipeline helpers on `Client`.
//!
//! Step 6-II's outbound orchestrators (`send_text`, `edit_message`,
//! `redact_message`, `toggle_reaction`, `send_read_confirmation`) and
//! the inbound dispatchers in [`crate::sync::dispatch`] share a small
//! set of common building blocks: resolve a room's MLS group id,
//! encrypt + upload under the lock, project public-API mentions to
//! storage / proto shapes, emit a fully-overlaid `TimelineDiff::UpdateAt`,
//! and persist the outgoing-text row.
//!
//! Those helpers live here so the per-interaction orchestrators
//! (`super::text`, `super::reaction`, `super::confirmation`,
//! `super::delete`, `super::knock`, `super::location`,
//! `super::ephemeral`) stay focused on
//! each pipeline's spine, and the receive handlers in
//! [`crate::sync::dispatch`] stay focused on the per-content-variant
//! dispatch table. No public-API symbols.
//!
//! Why a separate file: keeping these shared helpers out of the
//! per-interaction send modules (Step 30) stops any one of them from
//! re-growing toward the 868-line monolith the original
//! `timeline/send.rs` had become, and
//! keeps each file under 300 LOC and single-responsibility.

use crate::client::Client;
use crate::error::KupferdrahtError;
use crate::storage::TimelineEventRecord;
use crate::timeline::items::{DeliveryStatus, TimelineDiff, TimelineItem};

impl Client {
    /// Resolve `room_id` to its locally-cached `Room` and return the
    /// MLS `group_id` bytes. Used by every outbound text-leg orchestrator
    /// (`send_text`, `edit`, `redact`, `toggle_reaction`, `mark_as_read`).
    /// `op` names the calling operation for error messages.
    pub(crate) async fn resolve_room_group_id(
        &self,
        room_id: &str,
        op: &'static str,
    ) -> Result<Vec<u8>, KupferdrahtError> {
        let room = self.room(room_id).await.ok_or_else(|| {
            KupferdrahtError::InvalidInput(format!("{op}: {room_id} not in cache"))
        })?;
        room.group_id_bytes().ok_or_else(|| {
            KupferdrahtError::InvalidInput(format!(
                "{op}: {room_id} has no group_id — run sync_metadata first"
            ))
        })
    }

    /// Encrypt `plaintext` under the room's MLS group state and upload the
    /// ciphertext via `POST /v14/mls/messages`. The funnel every message send
    /// goes through (text / file / reply / reaction / knock / location / redact
    /// / edit / receipts).
    ///
    /// Self-heals a stale epoch (Step 37): if Wire rejects with
    /// `mls-stale-message`, the local group is behind the server. Resolve the
    /// room from `group_id`, [`resync_room`](Self::resync_room) to advance to
    /// the current epoch, then re-encrypt + resend **once**. A second stale (the
    /// epoch raced again mid-resync) or any other error propagates. The
    /// `group_id` is stable across the resync, so the resend is valid.
    pub(crate) async fn encrypt_and_upload(
        &self,
        group_id: &[u8],
        op: &'static str,
        plaintext: &[u8],
    ) -> Result<(), KupferdrahtError> {
        match self.encrypt_and_send_once(group_id, op, plaintext).await {
            Err(KupferdrahtError::MlsStaleMessage) => {
                let room_id = self.room_id_for_group(group_id).await.ok_or_else(|| {
                    KupferdrahtError::Generic(format!(
                        "{op}: mls-stale-message but no cached room owns this group"
                    ))
                })?;
                self.resync_room(&room_id).await?;
                self.encrypt_and_send_once(group_id, op, plaintext).await?;
            }
            other => other?,
        }
        Ok(())
    }

    /// A single encrypt-at-current-epoch + `POST /v14/mls/messages` attempt.
    /// The MLS lock is dropped before the HTTP `await` (LEARNINGS §3.1).
    async fn encrypt_and_send_once(
        &self,
        group_id: &[u8],
        op: &'static str,
        plaintext: &[u8],
    ) -> Result<(), KupferdrahtError> {
        let mls = self.mls_client_required(op)?;
        let ciphertext = {
            // Step 97e (ADR-0032): never inside an open notification window.
            let _gate = self.mls_write_gate().await;
            let guard = crate::encryption::lock_mls(mls);
            guard.encrypt_text_message(group_id, plaintext)?
        };
        self.http_send(self.api_version(), &ciphertext).await
    }

    /// Resolve the cached room that owns `group_id` (for stale-message resync).
    /// `None` when no cached room matches — e.g. a group whose room snapshot
    /// was evicted; the caller surfaces a clear error.
    async fn room_id_for_group(&self, group_id: &[u8]) -> Option<String> {
        self.rooms()
            .await
            .into_iter()
            .find(|room| room.group_id_bytes().as_deref() == Some(group_id))
            .map(|room| room.id())
    }

    /// The local user's own qualified id (`<localpart>@<domain>`). Used
    /// by anything that records "the local user did X" (the sender of a
    /// sent message, a reaction, a read receipt). Infallible since Step
    /// 19 — the domain is carried in [`UserId`].
    pub(crate) fn own_qualified_user_id(&self) -> String {
        self.account().user_id().as_str().to_string()
    }

    /// The `mentions` rows of a local body version, keyed by the row's
    /// stable id. Used by `send_text` (the original PK) and
    /// `edit_message` (the stable PK resolved before the edit, Step 105).
    pub(crate) fn local_mention_records(
        room_id: &str,
        message_id: &str,
        mentions: &[crate::timeline::Mention],
    ) -> Vec<crate::storage::MentionRecord> {
        mentions
            .iter()
            .map(|m| crate::storage::MentionRecord {
                message_id: message_id.to_string(),
                room_id: room_id.to_string(),
                user_id: m.user_id.clone(),
                start: i32::try_from(m.start).unwrap_or(i32::MAX),
                length: i32::try_from(m.length).unwrap_or(i32::MAX),
            })
            .collect()
    }

    /// Resolve a list of caller-supplied message ids (which may be
    /// either the row's stable PK or a chain pointer) to their
    /// canonical PKs. Ids that don't match any local row are dropped
    /// silently — the caller is doing a best-effort batch operation
    /// (e.g. read receipts) and a missing parent is tolerable.
    pub(crate) async fn resolve_canonical_ids(
        &self,
        room_id: &str,
        ids: &[String],
    ) -> Result<Vec<String>, KupferdrahtError> {
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(canonical) = self.canonical_message_id(room_id, id).await? {
                out.push(canonical);
            }
        }
        Ok(out)
    }

    /// Compute the new full reaction set for `(canonical_id,
    /// self_user_id)` after toggling `emoji`: existing + emoji on a
    /// fresh add, existing - emoji on a remove. Returns the resulting
    /// set ordered as the rows came back from storage.
    pub(crate) async fn toggle_user_reaction_set(
        &self,
        room_id: &str,
        canonical_id: &str,
        self_user_id: &str,
        emoji: &str,
    ) -> Result<Vec<String>, KupferdrahtError> {
        let Some(store) = self.store_handle() else {
            return Ok(vec![emoji.to_string()]);
        };
        let rows = store.load_reactions_for_room(room_id).await?;
        let mut current: Vec<String> = rows
            .into_iter()
            .filter(|r| r.message_id == canonical_id && r.sender_user_id == self_user_id)
            .map(|r| r.emoji)
            .collect();
        if current.iter().any(|e| e == emoji) {
            current.retain(|e| e != emoji);
        } else {
            current.push(emoji.to_string());
        }
        Ok(current)
    }

    /// Load mentions for `message_id` and project into the public-API
    /// `Vec<Mention>` shape. Empty when no `Store` is attached.
    pub(crate) async fn load_mentions_for_message(
        &self,
        room_id: &str,
        message_id: &str,
    ) -> Result<Vec<crate::timeline::Mention>, KupferdrahtError> {
        let Some(store) = self.store_handle() else {
            return Ok(Vec::new());
        };
        let rows = store.load_mentions_for_room(room_id).await?;
        Ok(rows
            .into_iter()
            .filter(|r| r.message_id == message_id)
            .map(|r| crate::timeline::Mention {
                user_id: r.user_id,
                start: u32::try_from(r.start).unwrap_or(0),
                length: u32::try_from(r.length).unwrap_or(0),
            })
            .collect())
    }

    /// Re-read the row, reactions, read-receipts, and mentions for
    /// `canonical_id`, build a fully-overlaid `TimelineItem`, and
    /// dispatch a `TimelineDiff::UpdateAt`. Silent no-op when the
    /// row isn't locally persisted (metadata-only resume mode).
    /// Used by every receive- and send-path mutation that doesn't
    /// produce a brand-new tail item.
    pub(crate) async fn emit_message_update_at(
        &self,
        room_id: &str,
        canonical_id: &str,
    ) -> Result<(), KupferdrahtError> {
        let Some(record) = self.lookup_timeline_event(room_id, canonical_id).await? else {
            return Ok(());
        };
        let reactions = self
            .load_reactions_for_message(room_id, canonical_id)
            .await?;
        let read_by = self.load_read_by_for_message(room_id, canonical_id).await?;
        let delivered_to = self
            .load_delivered_to_for_message(room_id, canonical_id)
            .await?;
        let mentions = self
            .load_mentions_for_message(room_id, canonical_id)
            .await?;
        let mut item = TimelineItem::from_record(record);
        item.delivery_status = self.settled_status(item.delivery_status, &item.item_id);
        // Step 65 (review S5): an asset row degrades to Text in
        // from_record — reconstruct before the Text-only mention
        // overlay below, or a reaction on a file emits an UpdateAt
        // whose item is Text with the filename.
        if let Some(store) = self.store_handle()
            && let Some(attachment) = store.get_attachment(canonical_id, room_id).await?
        {
            crate::timeline::reconstruct_asset_kind(&mut item, &[attachment], &self.weak());
        }
        item.reactions = reactions;
        item.read_by = read_by;
        item.delivered_to = delivered_to;
        if let crate::timeline::TimelineItemKind::Text {
            body,
            reply_to,
            mentions: existing,
        } = &mut item.kind
        {
            let _ = existing;
            item.kind = crate::timeline::TimelineItemKind::Text {
                body: std::mem::take(body),
                reply_to: reply_to.take(),
                mentions,
            };
        }
        self.dispatch_timeline_diff(&TimelineDiff::UpdateAt {
            room_id: room_id.to_string(),
            index: u32::MAX,
            item,
        });
        Ok(())
    }

    /// Project the freshly-persisted outgoing-text record into a
    /// `TimelineDiff::Append` carrying the inline mention overlay
    /// the caller passed at send time. Used by `send_text` so
    /// subscribers see the mention-decorated item without a
    /// `load_mentions_for_room` round-trip.
    pub(crate) fn emit_outgoing_text_append(
        &self,
        record: TimelineEventRecord,
        mentions: &[crate::timeline::Mention],
    ) {
        let room_id = record.room_id.clone();
        let mut item = TimelineItem::from_record(record);
        item.kind = match item.kind {
            crate::timeline::TimelineItemKind::Text { body, reply_to, .. } => {
                crate::timeline::TimelineItemKind::Text {
                    body,
                    reply_to,
                    mentions: mentions.to_vec(),
                }
            }
            other @ (crate::timeline::TimelineItemKind::Redacted
            | crate::timeline::TimelineItemKind::Asset(_)
            | crate::timeline::TimelineItemKind::Knock { .. }
            | crate::timeline::TimelineItemKind::Location { .. }
            | crate::timeline::TimelineItemKind::Task(_)
            | crate::timeline::TimelineItemKind::Case(_)
            | crate::timeline::TimelineItemKind::TaskListMarker { .. }) => other,
        };
        self.dispatch_timeline_diff(&TimelineDiff::Append { room_id, item });
    }

    /// The `timeline_events` row every outbound send persists: the
    /// local user as sender, `kind` / `body` as the packed
    /// representation, no edit / delete / expiry stamps. Callers set
    /// the path-specific decorations (`reply_to_id`, `expires_at_ms`)
    /// before inserting. Shared by the text, content (knock /
    /// location), and ephemeral send paths. Step 89.
    pub(super) fn outgoing_record(
        &self,
        message_id: &str,
        room_id: &str,
        kind: &str,
        body: &str,
        sent_at_ms: i64,
        status: DeliveryStatus,
    ) -> TimelineEventRecord {
        debug_assert!(
            !message_id.is_empty(),
            "outgoing_record: message_id must be set"
        );
        TimelineEventRecord {
            message_id: message_id.to_string(),
            room_id: room_id.to_string(),
            sender_user_id: self.own_qualified_user_id(),
            sender_device_id: self.device_id().map(str::to_string).unwrap_or_default(),
            kind: kind.to_string(),
            body: body.to_string(),
            sent_at_ms,
            delivery_status: status.as_db_str().into(),
            reply_to_id: None,
            edited_at_ms: None,
            redacted_at_ms: None,
            expires_at_ms: None,
            ephemeral_ttl_ms: None,
        }
    }

    /// Persist an outgoing text row (with `status`), its local mentions, and
    /// emit the `TimelineDiff::Append` — the shared tail of `send_text` for
    /// both the `Submitted` and `Failed` outcomes. Step 25a.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn persist_and_emit_outgoing_text(
        &self,
        message_id: &str,
        room_id: &str,
        body: &str,
        sent_at_ms: i64,
        reply_to: Option<&str>,
        mentions: &[crate::timeline::Mention],
        status: DeliveryStatus,
    ) -> Result<(), KupferdrahtError> {
        let Some(store) = self.store_handle() else {
            return Ok(());
        };
        let mut record =
            self.outgoing_record(message_id, room_id, "text", body, sent_at_ms, status);
        record.reply_to_id = reply_to.map(str::to_string);
        // Step 105: the row and its mentions are one write.
        let mention_records = Self::local_mention_records(room_id, message_id, mentions);
        if !store
            .insert_timeline_event_with_mentions(&record, &mention_records)
            .await?
        {
            return Err(outgoing_row_exists(message_id));
        }
        self.emit_outgoing_text_append(record, mentions);
        Ok(())
    }

    /// Persist + emit an outbound non-text content row (Knock / Location).
    /// `kind` / `body` are the packed representation `from_record` decodes
    /// back into the structured `TimelineItemKind`; there is no reply or
    /// mention rewrite (those are Text-only). Step 26.
    pub(super) async fn persist_and_emit_outgoing_content(
        &self,
        message_id: &str,
        room_id: &str,
        kind: &str,
        body: &str,
        sent_at_ms: i64,
        status: DeliveryStatus,
    ) -> Result<(), KupferdrahtError> {
        let Some(store) = self.store_handle() else {
            return Ok(());
        };
        let event = self.outgoing_record(message_id, room_id, kind, body, sent_at_ms, status);
        store.insert_timeline_event(&event).await?;
        self.dispatch_timeline_diff(&TimelineDiff::Append {
            room_id: room_id.to_string(),
            item: TimelineItem::from_record(event),
        });
        Ok(())
    }
}

/// A fresh outbound message id that already has a row — an outbound id is
/// generated per send, so this is a store anomaly, never a replay.
pub(crate) fn outgoing_row_exists(message_id: &str) -> KupferdrahtError {
    KupferdrahtError::Storage(format!("outbound message {message_id} is already stored"))
}
