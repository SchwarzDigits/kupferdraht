// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `Client::find_message_room` — cross-room lookup of the
//! `room_id` that owns a given `message_id` (or edit-event id).
//!
//! Powers the CLI's room-resolution sugar for the five
//! message-scoped subcommands (`mark-read`, `react`,
//! `delete-message`, `edit`, `reply`): the user can pass only
//! `--message <id>` and the CLI looks up the owning room from
//! the local `timeline_events` table. The underlying
//! `Timeline::*` library API stays room-scoped — the lookup is
//! purely a CLI ergonomic over the existing `Client` surface.

use crate::client::Client;
use crate::error::KupferdrahtError;
use crate::timeline::TimelineItem;

impl Client {
    /// Look up the rooms that contain a given message id. The id
    /// may be either a stable `message_id` or an edit-event id
    /// stored in `current_edit_event_id` (Wire's edit-chain
    /// pointer). Returns an empty vec when the id is not in the
    /// local store, or when no `SqliteStore` is attached.
    ///
    /// In practice the result is `Vec` of length 0 or 1 — the
    /// composite primary key `(message_id, room_id)` permits the
    /// same id in multiple rooms but `UUIDv4` collisions don't
    /// happen client-side. Callers that want to disambiguate when
    /// length > 1 surface their own error.
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` on `SQLite` read failure.
    pub async fn find_message_room(
        &self,
        target_id: &str,
    ) -> Result<Vec<String>, KupferdrahtError> {
        let Some(store) = self.store_handle() else {
            return Ok(Vec::new());
        };
        store.find_message_room(target_id).await
    }

    /// Resolve a single timeline item by id from the local store.
    /// `message_id` may be a stable id or an edit-event id (Wire's
    /// edit chain), matching [`Self::find_message_room`]. Returns
    /// `None` when the id is not stored locally (or no `SqliteStore`
    /// is attached).
    ///
    /// Lightweight: the returned item is built straight from the
    /// stored row, so its overlay collections (`reactions`,
    /// `read_by`, `delivered_to`) are empty. Use [`Room::messages`]
    /// for a fully hydrated item. Primary use is resolving a reply's
    /// parent for a quote preview (the wrapper has only the parent
    /// id via `TimelineItemKind::Text.reply_to`).
    ///
    /// [`Room::messages`]: crate::ffi::Room::messages
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` on `SQLite` read failure.
    pub async fn find_message_by_id(
        &self,
        message_id: &str,
    ) -> Result<Option<TimelineItem>, KupferdrahtError> {
        let Some(store) = self.store_handle() else {
            return Ok(None);
        };
        let record = store.load_timeline_event(message_id).await?;
        // Step 107: settled like every other reader — an abandoned send
        // reads `Failed`.
        Ok(record.map(|record| {
            let mut item = TimelineItem::from_record(record);
            item.delivery_status = self.settled_status(item.delivery_status, &item.item_id);
            item
        }))
    }
}
