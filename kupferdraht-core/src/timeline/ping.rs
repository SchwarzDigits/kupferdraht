// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Pings — Step 26, split out of the former monolithic
//! `timeline/send.rs` by Step 30.
//!
//! A ping is a message without content that draws the room's
//! attention; `repeated` marks one sent again to insist. Persisted as a
//! `timeline_events` row of kind `ping` whose body is the `repeated`
//! flag.

use crate::client::Client;
use crate::error::KupferdrahtError;
use crate::timeline::keyed::SendKey;
use crate::timeline::{MessageHandle, Timeline};

impl Timeline {
    /// Send a ping to this room. `repeated` marks a ping sent again to
    /// insist; a plain ping passes `false`. Step 26.
    ///
    /// # Errors
    /// Same downstream errors as [`Self::send_text`] (group resolve,
    /// encode, MLS encrypt, transport, storage).
    pub async fn send_ping(&self, repeated: bool) -> Result<MessageHandle, KupferdrahtError> {
        self.client.send_ping(&self.room_id, repeated, None).await
    }
}

impl Client {
    /// See [`Timeline::send_ping`] for the caller-facing contract. Step 26.
    /// `key` is stored with the `Sending` row (Step 130a).
    pub(crate) async fn send_ping(
        &self,
        room_id: &str,
        repeated: bool,
        key: Option<SendKey<'_>>,
    ) -> Result<MessageHandle, KupferdrahtError> {
        debug_assert!(!room_id.is_empty(), "send_ping: room_id must be non-empty");
        let group_id = self.resolve_room_group_id(room_id, "send_ping").await?;
        let (message_id, plaintext) = crate::wire::proto::build_knock_generic_message(repeated)?;
        // Step 97a: the row is written as `Sending` before the upload.
        let sent_at_ms = self
            .send_content_row(
                room_id,
                &group_id,
                "send_ping",
                crate::timeline::ContentRow {
                    message_id: &message_id,
                    plaintext: &plaintext,
                    kind: "ping",
                    body: &repeated.to_string(),
                    key,
                },
            )
            .await?;
        Ok(MessageHandle {
            message_id,
            sent_at_ms,
        })
    }
}
