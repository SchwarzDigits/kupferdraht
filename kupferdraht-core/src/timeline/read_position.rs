// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 129a — the read position across devices: a read of a room is told
//! to the account's other devices as Wire's `LastRead` in the personal room
//! (the rail Step 86 opened), the way Wire's own clients tell each other.

use tracing::warn;

use crate::client::Client;
use crate::error::KupferdrahtError;

impl Client {
    /// Tell the account's other devices that `room_id` is read up to
    /// `sent_at_ms`, the backend's time of the item read (Step 151's
    /// `sent_at_ms` — the one time every device of the account shares).
    /// Best effort, as the Step-25b receipts: the local marker has moved
    /// already, a failure here is logged and changes nothing. A read inside
    /// the personal room tells nobody — the other devices read the same
    /// room — and so does an account without a personal room in its
    /// snapshot.
    pub(crate) async fn share_read_position(&self, room_id: &str, sent_at_ms: i64) {
        debug_assert!(!room_id.is_empty(), "a read position names its room");
        if let Err(e) = self.send_read_position(room_id, sent_at_ms).await {
            warn!(
                room_id,
                error = %e,
                "the read position could not be shared with the account's other devices"
            );
        }
    }

    /// The Step-86 join inheritance, then the upload: a device whose openmls
    /// store has no group for the personal room joins it first (the first
    /// device establishes it, a later one external-joins).
    async fn send_read_position(
        &self,
        room_id: &str,
        sent_at_ms: i64,
    ) -> Result<(), KupferdrahtError> {
        let Some(personal) = self.personal_room() else {
            return Ok(());
        };
        let personal_id = personal.id();
        if personal_id == room_id {
            return Ok(());
        }
        let group_id = personal.group_id_bytes().ok_or_else(|| {
            KupferdrahtError::InvalidInput(format!(
                "share_read_position: {personal_id} has no group_id"
            ))
        })?;
        if !self.has_mls_group(&group_id) {
            self.join_room(&personal_id).await?;
        }
        let (_, plaintext) =
            crate::wire::proto::build_last_read_generic_message(room_id, sent_at_ms)?;
        self.encrypt_and_upload(&group_id, "share_read_position", &plaintext)
            .await?;
        Ok(())
    }
}
