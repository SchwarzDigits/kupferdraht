// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Emoji reactions (`Reaction` `GenericMessage`) — Step 6-II-e,
//! split out of the former monolithic `timeline/send.rs` by Step 30.
//!
//! Wire's `Reaction` is a full-set payload, not a single-emoji toggle:
//! the `emoji` field carries the caller's **complete** emoji set on
//! the target message after the toggle, comma-separated. Kupferdraht owns
//! the add/remove decision locally, then emits the resulting set.

use crate::time::now_ms;

use crate::client::Client;
use crate::error::KupferdrahtError;
use crate::timeline::{MessageHandle, Timeline};

impl Timeline {
    /// Toggle a single emoji reaction on `item_id`. Step 6-II-e.
    /// If the caller already has this emoji on that message,
    /// removes it; otherwise adds it. Wire-side emits a fresh
    /// `Reaction` event whose `emoji` field is the caller's
    /// **complete** set of emojis on this message after the
    /// toggle, comma-separated — Wire's `Reaction` is a full-set
    /// payload, not a single-emoji toggle on the wire.
    ///
    /// matrix-rust-sdk analog: `Timeline::toggle_reaction`.
    ///
    /// # Errors
    /// Same as [`Self::send_text`]; additionally `InvalidInput`
    /// when `item_id` or `emoji` is empty.
    pub async fn toggle_reaction(
        &self,
        item_id: String,
        emoji: String,
    ) -> Result<MessageHandle, KupferdrahtError> {
        if item_id.is_empty() {
            return Err(KupferdrahtError::InvalidInput(
                "toggle_reaction: item_id must be non-empty".into(),
            ));
        }
        if emoji.is_empty() {
            return Err(KupferdrahtError::InvalidInput(
                "toggle_reaction: emoji must be non-empty".into(),
            ));
        }
        self.client
            .toggle_reaction(&self.room_id, &item_id, &emoji)
            .await
    }
}

impl Client {
    /// See [`Timeline::toggle_reaction`] for the caller-facing
    /// contract. Step 6-II-e.
    pub(crate) async fn toggle_reaction(
        &self,
        room_id: &str,
        item_id: &str,
        emoji: &str,
    ) -> Result<MessageHandle, KupferdrahtError> {
        let group_id = self
            .resolve_room_group_id(room_id, "toggle_reaction")
            .await?;
        let store = self.store_handle().ok_or_else(|| {
            KupferdrahtError::InvalidInput(
                "toggle_reaction: no Store attached — set ClientBuilder::base_dir(...) first"
                    .into(),
            )
        })?;
        let self_user_id = self.own_qualified_user_id();
        let canonical_id = self
            .canonical_message_id(room_id, item_id)
            .await?
            .ok_or_else(|| {
                KupferdrahtError::InvalidInput(format!(
                    "toggle_reaction: no row matching {item_id} in {room_id}"
                ))
            })?;
        let new_set = self
            .toggle_user_reaction_set(room_id, &canonical_id, &self_user_id, emoji)
            .await?;
        let csv = new_set.join(",");
        // Wire-side: canonical PK for stability across edit chains.
        let (event_id, plaintext) =
            crate::wire::proto::build_reaction_generic_message(&canonical_id, &csv)?;
        self.encrypt_and_upload(&group_id, "toggle_reaction", &plaintext)
            .await?;
        let sent_at_ms = now_ms();
        store
            .replace_user_reactions(room_id, &canonical_id, &self_user_id, &new_set, sent_at_ms)
            .await?;
        self.emit_message_update_at(room_id, &canonical_id).await?;
        Ok(MessageHandle {
            message_id: event_id,
            sent_at_ms,
        })
    }
}
