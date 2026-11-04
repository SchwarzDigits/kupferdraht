// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Keyed sends — one entry for every send the caller may retry with the
//! same idempotency key (Step 130a). The key and the outcome of its call
//! live in the account store (`send_keys`, V32); the same key answers the
//! stored outcome instead of reaching the backend a second time.

use crate::client::Client;
use crate::error::KupferdrahtError;
use crate::storage::{AttachmentRecord, MentionRecord, SendKeyRecord, Store, TimelineEventRecord};
use crate::timeline::{Mention, MessageHandle, Timeline};

/// The longest idempotency key [`Timeline::send_keyed`] accepts, in
/// characters.
const MAX_KEY_CHARS: usize = 128;
/// `send_keys.kind` of a key written with a message's `Sending` row.
const KIND_MESSAGE: &str = "message";
/// `send_keys.kind` of a key written with a reaction set.
const KIND_REACTION: &str = "reaction";

/// What a keyed send sends. matrix-rust-sdk analog: the content handed to
/// `Room::send(content).with_transaction_id(txn_id)`.
#[derive(Debug, Clone, PartialEq, uniffi::Enum)]
pub enum OutgoingContent {
    /// A text message; joins the room's MLS group first when this client
    /// has none (as [`Timeline::send_text_auto_join`]).
    Text {
        body: String,
        mentions: Vec<Mention>,
    },
    /// A text reply quoting `parent_id` (as [`Timeline::send_reply`]).
    Reply { parent_id: String, body: String },
    /// A knock (as [`Timeline::send_knock`]).
    Knock { hot_knock: bool },
    /// A map point (as [`Timeline::send_location`]).
    Location {
        latitude: f32,
        longitude: f32,
        name: Option<String>,
        zoom: Option<i32>,
    },
    /// A file sent from memory (the browser's `sendFile`).
    File {
        bytes: Vec<u8>,
        filename: String,
        mime_type: String,
    },
    /// A reaction toggle on `item_id` (as [`Timeline::toggle_reaction`]).
    Reaction { item_id: String, emoji: String },
}

impl OutgoingContent {
    /// The `send_keys.kind` a key of this content is stored under.
    fn key_kind(&self) -> &'static str {
        if matches!(self, Self::Reaction { .. }) {
            KIND_REACTION
        } else {
            KIND_MESSAGE
        }
    }
}

/// The idempotency key of a keyed send, handed down the send path to the
/// write that stores it with what it answers for: the message's `Sending`
/// row, or the reaction set (Step 130a). `None` in its place is an unkeyed
/// send.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SendKey<'a>(&'a str);

impl SendKey<'_> {
    /// The key record a message's row is written with.
    pub(crate) fn message_record(self, event: &TimelineEventRecord) -> SendKeyRecord {
        self.record(
            KIND_MESSAGE,
            &event.room_id,
            &event.message_id,
            event.sent_at_ms,
        )
    }

    /// The key record a reaction set is written with: the toggle's handle.
    pub(crate) fn reaction_record(self, room_id: &str, handle: &MessageHandle) -> SendKeyRecord {
        self.record(
            KIND_REACTION,
            room_id,
            &handle.message_id,
            handle.sent_at_ms,
        )
    }

    /// A key dated at its send: `created_at_ms` is the handle's
    /// `sent_at_ms`.
    fn record(self, kind: &str, room_id: &str, message_id: &str, sent_at_ms: i64) -> SendKeyRecord {
        SendKeyRecord {
            idempotency_key: self.0.to_string(),
            room_id: room_id.to_string(),
            kind: kind.to_string(),
            message_id: message_id.to_string(),
            sent_at_ms,
            created_at_ms: sent_at_ms,
        }
    }
}

/// Write an outgoing row with its attachment, or else its mentions — and
/// with `key` when the send has one — as one write (Step 105, Step 130a).
/// `Ok(false)` when the row already exists.
///
/// # Errors
/// `KupferdrahtError::Storage` on a failed write.
pub(crate) async fn insert_outgoing_row(
    store: &dyn Store,
    event: &TimelineEventRecord,
    mentions: &[MentionRecord],
    attachment: Option<&AttachmentRecord>,
    key: Option<SendKey<'_>>,
) -> Result<bool, KupferdrahtError> {
    match (key, attachment) {
        (Some(key), _) => {
            store
                .insert_timeline_event_keyed(
                    event,
                    mentions,
                    attachment,
                    &key.message_record(event),
                )
                .await
        }
        (None, Some(attachment)) => {
            store
                .insert_timeline_event_with_attachment(event, attachment)
                .await
        }
        (None, None) => {
            store
                .insert_timeline_event_with_mentions(event, mentions)
                .await
        }
    }
}

impl Timeline {
    /// Send `content` under `idempotency_key`: the first call sends, a
    /// later call with the same key answers the stored outcome — the
    /// handle of a sent message or reaction, a resend under the same id
    /// for a send that failed — instead of sending again. Keys live 28
    /// days. matrix-rust-sdk analog: `Room::send(content)
    /// .with_transaction_id(txn_id)`. Step 130a.
    ///
    /// # Errors
    /// * `InvalidInput` — a key that is empty or longer than 128
    ///   characters, a key used for another room or another kind of
    ///   content, a key whose send is still in flight on this client, a
    ///   client without a store.
    /// * The errors of the send `content` stands for.
    pub async fn send_keyed(
        &self,
        idempotency_key: String,
        content: OutgoingContent,
    ) -> Result<MessageHandle, KupferdrahtError> {
        validate_idempotency_key(&idempotency_key)?;
        let store = self.client.store_handle().ok_or_else(|| {
            KupferdrahtError::InvalidInput(
                "send_keyed: no Store attached — set ClientBuilder::base_dir(...) first".into(),
            )
        })?;
        match store.send_key(&idempotency_key).await? {
            Some(record) => {
                self.answer_known_key(store.as_ref(), &record, &content)
                    .await
            }
            None => self.send_with_key(SendKey(&idempotency_key), content).await,
        }
    }

    /// Send `content` for the first time, storing `key` with its row or
    /// its reaction set.
    async fn send_with_key(
        &self,
        key: SendKey<'_>,
        content: OutgoingContent,
    ) -> Result<MessageHandle, KupferdrahtError> {
        let room_id = self.room_id.as_str();
        match content {
            OutgoingContent::Text { body, mentions } => {
                self.send_text_keyed(body, &mentions, key).await
            }
            OutgoingContent::Reply { parent_id, body } => {
                self.send_reply_keyed(&parent_id, body, key).await
            }
            OutgoingContent::Knock { hot_knock } => {
                self.client.send_knock(room_id, hot_knock, Some(key)).await
            }
            OutgoingContent::Location {
                latitude,
                longitude,
                name,
                zoom,
            } => {
                (self.client)
                    .send_location(
                        room_id,
                        latitude,
                        longitude,
                        name.as_deref(),
                        zoom,
                        Some(key),
                    )
                    .await
            }
            OutgoingContent::File {
                bytes,
                filename,
                mime_type,
            } => {
                (self.client)
                    .send_bytes(room_id, &bytes, &filename, &mime_type, Some(key))
                    .await
            }
            OutgoingContent::Reaction { item_id, emoji } => {
                self.toggle_reaction_keyed(&item_id, &emoji, key).await
            }
        }
    }

    /// A keyed text with its mentions; joins the room's MLS group first
    /// when this client has none (as [`Self::send_text_auto_join`]).
    async fn send_text_keyed(
        &self,
        body: String,
        mentions: &[Mention],
        key: SendKey<'_>,
    ) -> Result<MessageHandle, KupferdrahtError> {
        self.join_mls_group_if_missing("send_keyed").await?;
        self.client
            .send_text(&self.room_id, body, None, mentions, Some(key))
            .await
    }

    /// A keyed reply quoting `parent_id` (as [`Self::send_reply`]).
    async fn send_reply_keyed(
        &self,
        parent_id: &str,
        body: String,
        key: SendKey<'_>,
    ) -> Result<MessageHandle, KupferdrahtError> {
        super::text::validate_reply_parent(parent_id)?;
        self.client
            .send_text(&self.room_id, body, Some(parent_id), &[], Some(key))
            .await
    }

    /// A keyed reaction toggle (as [`Self::toggle_reaction`]).
    async fn toggle_reaction_keyed(
        &self,
        item_id: &str,
        emoji: &str,
        key: SendKey<'_>,
    ) -> Result<MessageHandle, KupferdrahtError> {
        super::reaction::validate_reaction(item_id, emoji)?;
        self.client
            .toggle_reaction(&self.room_id, item_id, emoji, Some(key))
            .await
    }

    /// Answer a key that is already stored, without sending `content`
    /// again: refused for another room or another kind of content; a
    /// reaction's stored handle; for a message, what its row's state calls
    /// for ([`Client::answer_known_message_key`]).
    async fn answer_known_key(
        &self,
        store: &dyn Store,
        record: &SendKeyRecord,
        content: &OutgoingContent,
    ) -> Result<MessageHandle, KupferdrahtError> {
        if record.room_id != self.room_id {
            return Err(KupferdrahtError::InvalidInput(format!(
                "send_keyed: key {} was used in another room",
                record.idempotency_key
            )));
        }
        if record.kind != content.key_kind() {
            return Err(KupferdrahtError::InvalidInput(format!(
                "send_keyed: key {} was used for a {}",
                record.idempotency_key, record.kind
            )));
        }
        if record.kind == KIND_REACTION {
            return Ok(stored_handle(record));
        }
        self.client.answer_known_message_key(store, record).await
    }
}

impl Client {
    /// Answer a known message key from the state of its row: a failed or
    /// abandoned send is resent under its id (Step 79); a send still in
    /// flight on this client is refused; any other row — delivered,
    /// deleted, or no longer stored — answers the stored handle, with the
    /// row's current `sent_at_ms` when the row exists.
    async fn answer_known_message_key(
        &self,
        store: &dyn Store,
        record: &SendKeyRecord,
    ) -> Result<MessageHandle, KupferdrahtError> {
        debug_assert_eq!(record.kind, KIND_MESSAGE, "a message key");
        let Some(row) = store
            .load_timeline_event_in_room(&record.room_id, &record.message_id)
            .await?
        else {
            return Ok(stored_handle(record));
        };
        if self.is_resendable(&row) {
            return self.resend_row(&row).await;
        }
        if row.redacted_at_ms.is_none() && self.is_in_flight(&row.message_id) {
            return Err(KupferdrahtError::InvalidInput(format!(
                "send_keyed: key {} — message {} is still being sent",
                record.idempotency_key, row.message_id
            )));
        }
        Ok(MessageHandle {
            message_id: record.message_id.clone(),
            sent_at_ms: row.sent_at_ms,
        })
    }
}

/// The handle a stored key answers.
fn stored_handle(record: &SendKeyRecord) -> MessageHandle {
    MessageHandle {
        message_id: record.message_id.clone(),
        sent_at_ms: record.sent_at_ms,
    }
}

/// A key is 1 to [`MAX_KEY_CHARS`] characters.
fn validate_idempotency_key(idempotency_key: &str) -> Result<(), KupferdrahtError> {
    let chars = idempotency_key.chars().count();
    if chars == 0 || chars > MAX_KEY_CHARS {
        return Err(KupferdrahtError::InvalidInput(format!(
            "send_keyed: an idempotency key is 1 to {MAX_KEY_CHARS} characters, got {chars}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_is_one_to_128_characters() {
        assert!(validate_idempotency_key("").is_err());
        assert!(validate_idempotency_key("k").is_ok());
        assert!(
            validate_idempotency_key(&"ü".repeat(128)).is_ok(),
            "characters, not bytes"
        );
        assert!(validate_idempotency_key(&"k".repeat(129)).is_err());
    }

    #[test]
    fn only_a_reaction_is_keyed_as_a_reaction() {
        let reaction = OutgoingContent::Reaction {
            item_id: "m".into(),
            emoji: "👍".into(),
        };
        assert_eq!(reaction.key_kind(), KIND_REACTION);
        assert_eq!(
            OutgoingContent::Knock { hot_knock: false }.key_kind(),
            KIND_MESSAGE
        );
    }
}
