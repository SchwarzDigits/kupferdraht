// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 95b (ADR-0028) — the sending side of Wire's edit / delete rule.
//!
//! Every receiver rejects an edit or delete of a message whose sender is
//! another user (`sync/dispatch/authorize.rs`), so `Timeline::edit` and
//! `Timeline::redact` refuse one before anything is uploaded — otherwise
//! the local row would diverge from every other client's. Any device of
//! the self user counts as "own". A target the local store does not know
//! cannot be checked and goes out as before.

use crate::client::Client;
use crate::error::KupferdrahtError;

impl Client {
    /// `Ok(())` when `target_id` in `room_id` is the self user's message
    /// or unknown locally; `InvalidInput` when another user sent it.
    ///
    /// # Errors
    /// `KupferdrahtError::InvalidInput` for another user's message;
    /// `KupferdrahtError::Storage` / `Encoding` from the target lookup.
    pub(crate) async fn ensure_own_target(
        &self,
        room_id: &str,
        target_id: &str,
        op: &str,
    ) -> Result<(), KupferdrahtError> {
        let Some(store) = self.store_handle() else {
            return Ok(());
        };
        let Some(target) = store
            .load_timeline_event_in_room(room_id, target_id)
            .await?
        else {
            return Ok(());
        };
        let own = self
            .user_id()
            .is_some_and(|me| me.as_str() == target.sender_user_id);
        if own {
            return Ok(());
        }
        Err(KupferdrahtError::InvalidInput(format!(
            "{op}: can only {op} your own messages — {target_id} was sent by {}",
            target.sender_user_id
        )))
    }
}
