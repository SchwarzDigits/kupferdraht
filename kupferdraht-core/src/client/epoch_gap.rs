// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Is a suspected epoch gap real? (Step 106, ADR-0034 d.1)
//!
//! A message from a later epoch than the local group looks like a missed
//! commit, but the epoch of a message the client cannot decrypt is not
//! authenticated. The backend, as delivery service, knows the
//! conversation's current epoch; only when it is ahead of the local group
//! is the room healed.

use crate::error::KupferdrahtError;
use crate::rooms::parse_room_id;

use super::Client;

impl Client {
    /// Whether the backend's current epoch for `room_id` is ahead of the
    /// local MLS group's — read fresh from the conversation record, not
    /// from the rooms cache, which can be stale.
    ///
    /// # Errors
    /// `InvalidInput` for an unqualified or uncached room, a room without
    /// a group id, or a conversation the backend does not return; the
    /// local group's and the HTTP layer's errors otherwise.
    pub(crate) async fn backend_epoch_ahead(
        &self,
        room_id: &str,
    ) -> Result<bool, KupferdrahtError> {
        let qid = parse_room_id(room_id).ok_or_else(|| {
            KupferdrahtError::InvalidInput(format!(
                "epoch check: `{room_id}` is not a qualified id"
            ))
        })?;
        let group_id = self
            .room(room_id)
            .await
            .and_then(|room| room.group_id_bytes())
            .ok_or_else(|| {
                KupferdrahtError::InvalidInput(format!(
                    "epoch check: {room_id} has no cached group"
                ))
            })?;
        let local = {
            let mls = self.mls_client_required("backend_epoch_ahead")?;
            crate::encryption::lock_mls(mls).group_epoch(&group_id)?
        };
        let remote = self
            .http
            .list_conversations(self.api_version, &self.current_access_token(), &[qid])
            .await?
            .found
            .first()
            .map(|conversation| conversation.epoch)
            .ok_or_else(|| {
                KupferdrahtError::InvalidInput(format!(
                    "epoch check: the backend returned no conversation for {room_id}"
                ))
            })?;
        tracing::debug!(room_id, local, remote, "epoch gap check");
        Ok(remote > local)
    }
}
