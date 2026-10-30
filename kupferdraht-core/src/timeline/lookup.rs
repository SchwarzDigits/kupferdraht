// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Read-side message-id lookups on `Client` — Step 30 split of the
//! former monolithic `timeline/send.rs`.
//!
//! These helpers project stored timeline / reaction / receipt rows for
//! a single message id, resolving Wire-Web's chain pointers to the
//! row's canonical PK first so a post-edit / post-delete re-read always
//! returns the right row. They back the timeline read model
//! (`Client::messages` and the receive paths) — none of them send.

use crate::client::Client;
use crate::error::KupferdrahtError;

impl Client {
    /// Crate-private helper: resolve `target_id` (could be the
    /// row's stable PK or a chain pointer from the V12 chain) to
    /// the row's canonical PK. Returns `None` if no Store is
    /// attached or no row matches. Used by Step 6-II-e's reaction
    /// receive path so reactions land under the stable PK
    /// regardless of which chain link Wire-Web addressed.
    pub(crate) async fn canonical_message_id(
        &self,
        room_id: &str,
        target_id: &str,
    ) -> Result<Option<String>, KupferdrahtError> {
        let Some(store) = self.store_handle() else {
            return Ok(None);
        };
        store.canonical_message_id(room_id, target_id).await
    }

    /// Crate-private helper: load all reactions for one message,
    /// already projected into the public-API
    /// `Vec<ReactionGroup>` shape (grouped by emoji). Step 6-II-e.
    pub(crate) async fn load_reactions_for_message(
        &self,
        room_id: &str,
        message_id: &str,
    ) -> Result<Vec<crate::timeline::ReactionGroup>, KupferdrahtError> {
        let Some(store) = self.store_handle() else {
            return Ok(Vec::new());
        };
        let rows = store.load_reactions_for_room(room_id).await?;
        Ok(crate::timeline::group_reactions_by_emoji(
            rows.into_iter().filter(|r| r.message_id == message_id),
        ))
    }

    /// Crate-private helper: load `read_by` (the list of reader
    /// user ids that have READ-confirmed this message) for one
    /// message. Step 6-II-f.
    pub(crate) async fn load_read_by_for_message(
        &self,
        room_id: &str,
        message_id: &str,
    ) -> Result<Vec<String>, KupferdrahtError> {
        let Some(store) = self.store_handle() else {
            return Ok(Vec::new());
        };
        let rows = store.load_read_receipts_for_room(room_id).await?;
        let mut readers: Vec<String> = rows
            .into_iter()
            .filter(|r| r.message_id == message_id)
            .map(|r| r.reader_user_id)
            .collect();
        readers.sort();
        readers.dedup();
        Ok(readers)
    }

    /// Crate-private helper: load `delivered_to` (the list of confirmer
    /// user ids that have DELIVERED-confirmed this message) for one
    /// message. Step 25b. Mirror of [`Self::load_read_by_for_message`].
    pub(crate) async fn load_delivered_to_for_message(
        &self,
        room_id: &str,
        message_id: &str,
    ) -> Result<Vec<String>, KupferdrahtError> {
        debug_assert!(
            !message_id.is_empty(),
            "load_delivered_to_for_message: message_id must be non-empty"
        );
        let Some(store) = self.store_handle() else {
            return Ok(Vec::new());
        };
        let rows = store.load_delivery_receipts_for_room(room_id).await?;
        let mut confirmers: Vec<String> = rows
            .into_iter()
            .filter(|r| r.message_id == message_id)
            .map(|r| r.confirmer_user_id)
            .collect();
        confirmers.sort();
        confirmers.dedup();
        Ok(confirmers)
    }

    /// Crate-private helper: load a single `timeline_events` row.
    /// Step 6-II-b onwards.
    ///
    /// `target_id` may be either the row's stable PK (`message_id`)
    /// or the current chain pointer (`current_edit_event_id`)
    /// populated by V12. The lookup resolves the chain pointer to
    /// the canonical PK first, then loads the row by it. This
    /// matches the OR-clause semantics of
    /// [`crate::storage::Store::canonical_message_id`] /
    /// [`crate::storage::Store::apply_message_edit`] /
    /// [`crate::storage::Store::apply_message_delete`] so every
    /// post-apply re-read returns the row regardless of which
    /// id the caller has in hand.
    ///
    /// Returns `None` if no `Store` is attached (metadata-only
    /// resume mode) or no row matches by either id.
    pub(crate) async fn lookup_timeline_event(
        &self,
        room_id: &str,
        target_id: &str,
    ) -> Result<Option<crate::storage::TimelineEventRecord>, KupferdrahtError> {
        let Some(store) = self.store_handle() else {
            return Ok(None);
        };
        let Some(canonical) = store.canonical_message_id(room_id, target_id).await? else {
            return Ok(None);
        };
        let rows = store.load_timeline_events(room_id).await?;
        Ok(rows.into_iter().find(|r| r.message_id == canonical))
    }
}
