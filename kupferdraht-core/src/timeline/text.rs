// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Text-family interactions — the `Text` `GenericMessage` plus the
//! edits, replies, and mentions layered on top of it (Step 30 split
//! of the former monolithic `timeline/send.rs`).
//!
//! Pipeline per send (unchanged from Step 4):
//!
//! 1. Build a fresh `GenericMessage` (`Text` variant) with a UUID
//!    `message_id` (`wire::proto::build_text_generic_message`).
//! 2. Encrypt under the local MLS group state
//!    (`MlsClient::encrypt_text_message`) → TLS-serialized
//!    `MlsMessage(PrivateMessage)` ciphertext.
//! 3. Upload the ciphertext to `POST /v14/mls/messages`
//!    (`HttpClient::send_mls_message`).
//! 4. Persist a `timeline_events` row keyed by the same
//!    `message_id`, `delivery_status='submitted'`.
//!
//! The MLS-encryption step is synchronous in openmls; it runs under
//! the `MlsClient` mutex briefly and the guard is dropped before the
//! `await` on the HTTP upload (LEARNINGS §3.1 — never hold an
//! openmls lock across an `await`).

use crate::time::now_ms;

use crate::client::Client;
use crate::error::KupferdrahtError;
use crate::timeline::{MessageHandle, Timeline};
use crate::wire::proto::{MentionInput, QuoteParent, build_text_generic_message};

impl Timeline {
    /// Encrypt + upload + persist a single text message. Returns a
    /// [`MessageHandle`] so the caller can correlate the locally-
    /// recorded row with the same event arriving via sync later.
    ///
    /// # Errors
    /// * `KupferdrahtError::InvalidInput` — room missing from cache, or
    ///   the room has no local MLS group (call `Room::join` first).
    /// * `KupferdrahtError::Encoding` — protobuf encode or MLS TLS
    ///   serialize failure.
    /// * `KupferdrahtError::Generic` — openmls rejects the encrypt step
    ///   (eviction, pending proposal, …).
    /// * `KupferdrahtError::Network` / `BackendRejected` — Wire transport
    ///   or 4xx / 5xx response from `POST /v14/mls/messages`.
    /// * `KupferdrahtError::Storage` — `timeline_events` insert failure.
    pub async fn send_text(&self, body: String) -> Result<MessageHandle, KupferdrahtError> {
        self.client.send_text(&self.room_id, body, None, &[]).await
    }

    /// Step 14g — eager auto-join + send. Probes the local openmls
    /// store via `Client::has_mls_group`; on missing-group, calls
    /// [`Client::join_room`] first, then proceeds with
    /// [`Self::send_text`]. The probe + conditional join replace
    /// the original "lazy + error-string match" plan because
    /// matching on `KupferdrahtError::InvalidInput(msg).contains(...)`
    /// is fragile. Callers that want to handle the missing-MLS
    /// case explicitly can still do so by typed-matching
    /// [`KupferdrahtError::MlsGroupMissing`] on a plain `send_text`.
    ///
    /// Closes `v1-api-ergonomics-audit.md §1.2` — `send_text`
    /// fails confusingly when the room is cached but the openmls
    /// store has no group. Use `send_text_auto_join` for the "I
    /// just want to send to this room, set things up if needed"
    /// pattern; keep `send_text` for callers that want to decline
    /// the join (e.g. handle an invite gracefully).
    ///
    /// # Errors
    /// * `KupferdrahtError::InvalidInput` when the room is not in the
    ///   cache or has no `group_id` on its metadata snapshot
    ///   (caller should `Client::room_or_load` first).
    /// * Transport / MLS / backend errors from `Client::join_room`
    ///   if the probe fired and the join failed.
    /// * Same downstream errors as [`Self::send_text`] for the
    ///   send leg.
    pub async fn send_text_auto_join(
        &self,
        body: String,
    ) -> Result<MessageHandle, KupferdrahtError> {
        let room = self.client.room(&self.room_id).await.ok_or_else(|| {
            KupferdrahtError::InvalidInput(format!(
                "send_text_auto_join: {} not in cache",
                self.room_id
            ))
        })?;
        let group_id = room.group_id_bytes().ok_or_else(|| {
            KupferdrahtError::InvalidInput(format!(
                "send_text_auto_join: {} has no group_id",
                self.room_id
            ))
        })?;
        if !self.client.has_mls_group(&group_id) {
            self.client.join_room(&self.room_id).await?;
        }
        self.send_text(body).await
    }

    /// Send a text message that mentions one or more users
    /// (Step 6-II-g). `mentions` declares UTF-16 spans of the
    /// body that point at qualified user-ids; Wire's UI renders
    /// each span as a clickable mention pill. matrix-rust-sdk
    /// analog: included in `Timeline::send` content.
    ///
    /// Caller is responsible for computing UTF-16 offsets that
    /// match `body` (the [`crate::timeline::Mention`] struct's
    /// docs spell out the format). The CLI auto-parses `@<handle>`
    /// tokens via `Client::resolve_mentions_in_body`.
    ///
    /// # Errors
    /// Same as [`Self::send_text`]; additionally `InvalidInput` if
    /// any mention span is out of range or has length 0.
    pub async fn send_text_with_mentions(
        &self,
        body: String,
        mentions: Vec<crate::timeline::Mention>,
    ) -> Result<MessageHandle, KupferdrahtError> {
        self.client
            .send_text(&self.room_id, body, None, &mentions)
            .await
    }

    /// Edit a previously-sent message in this room (Step 6-II-c).
    /// `item_id` is the `message_id` (UUID) of the original
    /// message. The edit emits a `MessageEdit { replacing_message_id,
    /// text { new_body } }` Wire-side and applies the same body
    /// replacement + `edited_at_ms` stamp to the local
    /// `timeline_events` row. matrix-rust-sdk analog: `Timeline::edit`.
    ///
    /// Returns a fresh [`MessageHandle`] for the *edit event itself*;
    /// the parent's `message_id` does not change.
    ///
    /// # Errors
    /// Same as [`Self::send_text`]; additionally `InvalidInput` when
    /// `item_id` is empty.
    pub async fn edit(
        &self,
        item_id: String,
        new_body: String,
    ) -> Result<MessageHandle, KupferdrahtError> {
        if item_id.is_empty() {
            return Err(KupferdrahtError::InvalidInput(
                "edit: item_id must be non-empty".into(),
            ));
        }
        self.client
            .edit_message(&self.room_id, &item_id, new_body)
            .await
    }

    /// Send a `Text` message that quotes a previous message in the
    /// same room (Step 6-II-b). `parent_id` is the
    /// `GenericMessage.message_id` of the message being replied to;
    /// it is emitted on the wire as
    /// `Text.quote.quoted_message_id` and persisted on the local
    /// row's `reply_to_id` column.
    ///
    /// matrix-rust-sdk analog: `Timeline::send_reply`.
    ///
    /// # Errors
    /// Same as [`Self::send_text`]; additionally `InvalidInput` when
    /// `parent_id` is empty.
    pub async fn send_reply(
        &self,
        parent_id: String,
        body: String,
    ) -> Result<MessageHandle, KupferdrahtError> {
        if parent_id.is_empty() {
            return Err(KupferdrahtError::InvalidInput(
                "send_reply: parent_id must be non-empty".into(),
            ));
        }
        self.client
            .send_text(&self.room_id, body, Some(&parent_id), &[])
            .await
    }
}

impl Client {
    /// Resolve a reply's quote parent and build the `Text` `GenericMessage`,
    /// returning `(message_id, plaintext)`. Split out of `send_text` to keep
    /// it within the per-function line budget.
    async fn build_text_message(
        &self,
        room_id: &str,
        body: &str,
        reply_to: Option<&str>,
        mentions: &[crate::timeline::Mention],
    ) -> Result<(String, Vec<u8>), KupferdrahtError> {
        let parent_lookup = match reply_to {
            Some(parent_id) => Some(self.load_quote_parent(room_id, parent_id).await?),
            None => None,
        };
        let quote_parent = parent_lookup.as_ref().map(quote_parent_of);
        let proto_mentions = to_proto_mentions(mentions);
        build_text_generic_message(body, quote_parent, &proto_mentions)
    }

    /// The persisted parent row a reply quotes. Wire's
    /// `quoted_message_sha256` requires the parent's body + send-time
    /// (observations.md §33) — without the local parent no valid wire
    /// quote can be built. Step 65 (review S4): hard-fail instead of
    /// silently sending plain text that only Kupferdraht renders as a
    /// reply. Shared by `send_reply` and the resend path (Step 79).
    pub(super) async fn load_quote_parent(
        &self,
        room_id: &str,
        parent_id: &str,
    ) -> Result<crate::storage::TimelineEventRecord, KupferdrahtError> {
        self.lookup_timeline_event(room_id, parent_id)
            .await?
            .ok_or_else(|| {
                KupferdrahtError::InvalidInput(format!(
                    "send_reply: parent {parent_id} not found locally — sync the room \
                     first (the wire quote needs the parent's body and send time)"
                ))
            })
    }

    /// See [`Timeline::send_text`] for the caller-facing contract.
    /// `reply_to` populates `Text.quote.quoted_message_id` (Step 6-II-b)
    /// when `Some`. `mentions` populates `Text.mentions` (Step 6-II-g).
    pub(crate) async fn send_text(
        &self,
        room_id: &str,
        body: String,
        reply_to: Option<&str>,
        mentions: &[crate::timeline::Mention],
    ) -> Result<MessageHandle, KupferdrahtError> {
        debug_assert!(!room_id.is_empty(), "send_text: room_id must be non-empty");
        let group_id = self.resolve_room_group_id(room_id, "send_text").await?;
        let (message_id, plaintext) = self
            .build_text_message(room_id, &body, reply_to, mentions)
            .await?;
        // Step 97a: the row is written as `Sending` BEFORE the upload — a
        // cancelled or crashed send leaves a resendable row (Step 25a kept
        // failed sends visible; this keeps interrupted ones too).
        let _in_flight = self.begin_send(&message_id);
        let sent_at_ms = now_ms();
        self.persist_and_emit_outgoing_text(
            &message_id,
            room_id,
            &body,
            sent_at_ms,
            reply_to,
            mentions,
            crate::timeline::DeliveryStatus::Sending,
        )
        .await?;
        let upload = self
            .encrypt_and_upload(&group_id, "send_text", &plaintext)
            .await;
        self.finish_send(room_id, &message_id, upload.is_ok(), None)
            .await;
        upload?;
        Ok(MessageHandle {
            message_id,
            sent_at_ms,
        })
    }

    /// See [`Timeline::edit`] for the caller-facing contract.
    pub(crate) async fn edit_message(
        &self,
        room_id: &str,
        replacing_message_id: &str,
        new_body: String,
    ) -> Result<MessageHandle, KupferdrahtError> {
        // Step 95b (ADR-0028): receivers reject an edit by anyone but the
        // original user — refuse it before it goes out.
        self.ensure_own_target(room_id, replacing_message_id, "edit")
            .await?;
        let group_id = self.resolve_room_group_id(room_id, "edit").await?;
        // 6-II-g symmetry with send_text: auto-parse `@<handle>`
        // tokens and re-emit them — mentions belong to the body
        // version, so the edit must carry the new set (CHANGELOG.md §35.C).
        let mentions = self.resolve_mentions_in_body(&new_body).await?;
        let proto_mentions = to_proto_mentions(&mentions);
        let (edit_event_id, plaintext) = crate::wire::proto::build_message_edit_generic_message(
            replacing_message_id,
            &new_body,
            &proto_mentions,
        )?;
        self.encrypt_and_upload(&group_id, "edit", &plaintext)
            .await?;
        let edited_at_ms = now_ms();
        self.persist_local_edit(
            room_id,
            replacing_message_id,
            &edit_event_id,
            new_body,
            edited_at_ms,
            &mentions,
        )
        .await?;
        Ok(MessageHandle {
            message_id: edit_event_id,
            sent_at_ms: edited_at_ms,
        })
    }

    /// Apply an own edit locally: the body and its mentions as one write,
    /// keyed by the row's stable id resolved **before** the edit advances
    /// the chain pointer (Step 105 — resolved after, an edit of an edit
    /// found no row and lost its mentions and its update), then the
    /// `UpdateAt`.
    async fn persist_local_edit(
        &self,
        room_id: &str,
        replacing_message_id: &str,
        edit_event_id: &str,
        new_body: String,
        edited_at_ms: i64,
        mentions: &[crate::timeline::Mention],
    ) -> Result<(), KupferdrahtError> {
        let Some(store) = self.store_handle() else {
            return Ok(());
        };
        let Some(stable_id) = self
            .canonical_message_id(room_id, replacing_message_id)
            .await?
        else {
            return Ok(());
        };
        let records = Self::local_mention_records(room_id, &stable_id, mentions);
        let edit = crate::storage::MessageEditRecord {
            room_id: room_id.to_string(),
            message_id: stable_id,
            replacing_message_id: replacing_message_id.to_string(),
            edit_event_id: edit_event_id.to_string(),
            new_body,
            edited_at_ms,
        };
        if store
            .apply_message_edit_with_mentions(&edit, &records)
            .await?
        {
            self.emit_message_update_at(room_id, &edit.message_id)
                .await?;
        }
        Ok(())
    }
}

/// Project a persisted parent row into the quote input the text
/// builders take.
pub(super) fn quote_parent_of(parent: &crate::storage::TimelineEventRecord) -> QuoteParent<'_> {
    QuoteParent {
        message_id: &parent.message_id,
        body: &parent.body,
        sent_at_ms: parent.sent_at_ms,
    }
}

/// Convert public-API `Mention` entries into Wire's protobuf
/// `MentionInput` shape. Used by the outbound text + edit paths and
/// the resend path (Step 79).
pub(super) fn to_proto_mentions(mentions: &[crate::timeline::Mention]) -> Vec<MentionInput> {
    mentions
        .iter()
        .map(|m| MentionInput {
            start: i32::try_from(m.start).unwrap_or(i32::MAX),
            length: i32::try_from(m.length).unwrap_or(i32::MAX),
            qualified_user_id: m.user_id.clone(),
        })
        .collect()
}
