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
use crate::storage::Store;
use crate::timeline::keyed::SendKey;
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
        validate_reaction(&item_id, &emoji)?;
        self.client
            .toggle_reaction(&self.room_id, &item_id, &emoji, None)
            .await
    }
}

/// A reaction names its target and its emoji.
pub(super) fn validate_reaction(item_id: &str, emoji: &str) -> Result<(), KupferdrahtError> {
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
    Ok(())
}

impl Client {
    /// See [`Timeline::toggle_reaction`] for the caller-facing
    /// contract. Step 6-II-e. `key` is stored with the new reaction set,
    /// after the upload (Step 130a).
    pub(crate) async fn toggle_reaction(
        &self,
        room_id: &str,
        item_id: &str,
        emoji: &str,
        key: Option<SendKey<'_>>,
    ) -> Result<MessageHandle, KupferdrahtError> {
        let group_id = self
            .resolve_room_group_id(room_id, "toggle_reaction")
            .await?;
        let store = self.reaction_store()?;
        let self_user_id = self.own_qualified_user_id();
        let canonical_id = self.reaction_target(room_id, item_id).await?;
        let new_set = self
            .toggle_user_reaction_set(room_id, &canonical_id, &self_user_id, emoji)
            .await?;
        let csv = new_set.join(",");
        // Wire-side: canonical PK for stability across edit chains.
        let (event_id, plaintext) =
            crate::wire::proto::build_reaction_generic_message(&canonical_id, &csv)?;
        self.encrypt_and_upload(&group_id, "toggle_reaction", &plaintext)
            .await?;
        let handle = MessageHandle {
            message_id: event_id,
            sent_at_ms: now_ms(),
        };
        let own_set = OwnReactionSet {
            room_id,
            canonical_id: &canonical_id,
            sender_user_id: &self_user_id,
            emojis: &new_set,
        };
        store_own_reaction_set(store.as_ref(), &own_set, &handle, key).await?;
        self.emit_message_update_at(room_id, &canonical_id).await?;
        Ok(handle)
    }

    /// The store a toggle reads the own set from and writes it to.
    fn reaction_store(&self) -> Result<&std::sync::Arc<dyn Store>, KupferdrahtError> {
        self.store_handle().ok_or_else(|| {
            KupferdrahtError::InvalidInput(
                "toggle_reaction: no Store attached — set ClientBuilder::base_dir(...) first"
                    .into(),
            )
        })
    }

    /// The stable id of the row `item_id` names, the id a reaction targets.
    async fn reaction_target(
        &self,
        room_id: &str,
        item_id: &str,
    ) -> Result<String, KupferdrahtError> {
        self.canonical_message_id(room_id, item_id)
            .await?
            .ok_or_else(|| {
                KupferdrahtError::InvalidInput(format!(
                    "toggle_reaction: no row matching {item_id} in {room_id}"
                ))
            })
    }
}

/// The own reaction set on one message after a toggle.
struct OwnReactionSet<'a> {
    room_id: &'a str,
    canonical_id: &'a str,
    sender_user_id: &'a str,
    emojis: &'a [String],
}

/// Store the own reaction set stamped with the toggle's `sent_at_ms` —
/// with the toggle's key as one write for a keyed toggle (Step 130a).
async fn store_own_reaction_set(
    store: &dyn Store,
    set: &OwnReactionSet<'_>,
    handle: &MessageHandle,
    key: Option<SendKey<'_>>,
) -> Result<(), KupferdrahtError> {
    match key {
        Some(key) => {
            store
                .replace_user_reactions_keyed(
                    set.room_id,
                    set.canonical_id,
                    set.sender_user_id,
                    set.emojis,
                    &key.reaction_record(set.room_id, handle),
                )
                .await
        }
        None => {
            store
                .replace_user_reactions(
                    set.room_id,
                    set.canonical_id,
                    set.sender_user_id,
                    set.emojis,
                    handle.sent_at_ms,
                )
                .await
        }
    }
}
