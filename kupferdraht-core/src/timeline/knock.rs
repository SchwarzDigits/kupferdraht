// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Knocks / nudges (`Knock` `GenericMessage`) — Step 26, split out of
//! the former monolithic `timeline/send.rs` by Step 30.
//!
//! A knock is a contentless "ping"; `hot_knock` marks Wire's
//! "double-tap" repeat nudge. Persisted as a `timeline_events` row of
//! kind `knock` whose body is the `hot_knock` flag.

use crate::client::Client;
use crate::error::KupferdrahtError;
use crate::timeline::{MessageHandle, Timeline};

impl Timeline {
    /// Send a `Knock` (nudge) to this room. `hot_knock` is Wire's
    /// "double-tap" flag; plain knocks pass `false`. Step 26.
    ///
    /// # Errors
    /// Same downstream errors as [`Self::send_text`] (group resolve,
    /// encode, MLS encrypt, transport, storage).
    pub async fn send_knock(&self, hot_knock: bool) -> Result<MessageHandle, KupferdrahtError> {
        self.client.send_knock(&self.room_id, hot_knock).await
    }
}

impl Client {
    /// See [`Timeline::send_knock`] for the caller-facing contract. Step 26.
    pub(crate) async fn send_knock(
        &self,
        room_id: &str,
        hot_knock: bool,
    ) -> Result<MessageHandle, KupferdrahtError> {
        debug_assert!(!room_id.is_empty(), "send_knock: room_id must be non-empty");
        let group_id = self.resolve_room_group_id(room_id, "send_knock").await?;
        let (message_id, plaintext) = crate::wire::proto::build_knock_generic_message(hot_knock)?;
        // Step 97a: the row is written as `Sending` before the upload.
        let sent_at_ms = self
            .send_content_row(
                room_id,
                &group_id,
                "send_knock",
                crate::timeline::ContentRow {
                    message_id: &message_id,
                    plaintext: &plaintext,
                    kind: "knock",
                    body: &hot_knock.to_string(),
                },
            )
            .await?;
        Ok(MessageHandle {
            message_id,
            sent_at_ms,
        })
    }
}
