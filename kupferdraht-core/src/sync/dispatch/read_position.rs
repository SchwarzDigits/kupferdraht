// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Inbound `LastRead` dispatch — Step 129a: the read position another device
//! of the account reported through the personal room.

use std::sync::Arc;

use tracing::{debug, info};

use crate::client::Client;
use crate::error::KupferdrahtError;
use crate::sync::service::SyncService;

use super::InboundOrigin;

/// Whether a `LastRead` may move this device's read position: only one the
/// account's own other device sent into the personal room. Wire's clients
/// require the same two things of each other (`docs/wire/api-notes.md`,
/// Step 129a); anything else is a stranger's claim and is dropped.
pub(crate) fn read_position_is_authorized(
    origin_room_id: &str,
    sender_user_id: &str,
    personal_room_id: Option<&str>,
    own_user_id: &str,
) -> bool {
    personal_room_id == Some(origin_room_id) && sender_user_id == own_user_id
}

/// [`read_position_is_authorized`] for an inbound message, the drop logged.
fn from_the_accounts_own_device(client: &Client, origin: InboundOrigin<'_>) -> bool {
    let personal_room_id = client.personal_room().map(|room| room.id());
    let authorized = read_position_is_authorized(
        origin.room_id,
        origin.sender_user_id,
        personal_room_id.as_deref(),
        &client.own_qualified_user_id(),
    );
    if !authorized {
        debug!(
            message_id = origin.message_id,
            room_id = origin.room_id,
            sender = origin.sender_user_id,
            "LastRead outside the personal room or from another user — dropped"
        );
    }
    authorized
}

impl SyncService {
    /// Apply an inbound `LastRead`: under [`read_position_is_authorized`],
    /// map its backend time onto this device's position of the named room
    /// and move the marker forward only; a move clears the hand-set flag
    /// and names the room in the sync pass's `changed_rooms` (Step 146). A
    /// room this device does not know, or one with nothing sent by then,
    /// changes nothing. Nothing is persisted as a timeline row: a
    /// `LastRead` is signalling, as Wire's clients treat it. Step 129a.
    pub(crate) async fn dispatch_inbound_read_position(
        &self,
        client: &Arc<Client>,
        origin: InboundOrigin<'_>,
        last_read: crate::wire::proto::LastRead,
    ) -> Result<(), KupferdrahtError> {
        if !from_the_accounts_own_device(client, origin) {
            return Ok(());
        }
        let Some(store) = client.store_handle() else {
            return Ok(());
        };
        let own_domain = client.account().user_id().domain().to_string();
        let room_id = crate::wire::proto::last_read_room_id(&last_read, &own_domain);
        let Some(position) = store
            .latest_arrival_sent_at_or_before(&room_id, last_read.last_read_timestamp)
            .await?
        else {
            debug!(
                message_id = origin.message_id,
                room_id,
                last_read_timestamp = last_read.last_read_timestamp,
                "LastRead names a room or a time this device has nothing for — dropped"
            );
            return Ok(());
        };
        if store.advance_read_marker(&room_id, position).await? {
            client.note_room_changed(&room_id);
            info!(
                room_id,
                position, "read position moved by another device of the account"
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::read_position_is_authorized;

    const PERSONAL: &str = "personal@example.test";
    const GROUP: &str = "group@example.test";
    const ME: &str = "me@example.test";
    const BOB: &str = "bob@example.test";

    #[test]
    fn the_own_user_in_the_personal_room_is_authorized() {
        assert!(read_position_is_authorized(
            PERSONAL,
            ME,
            Some(PERSONAL),
            ME
        ));
    }

    #[test]
    fn another_room_is_not() {
        assert!(!read_position_is_authorized(GROUP, ME, Some(PERSONAL), ME));
    }

    #[test]
    fn another_user_is_not_even_in_the_personal_room() {
        assert!(!read_position_is_authorized(
            PERSONAL,
            BOB,
            Some(PERSONAL),
            ME
        ));
    }

    #[test]
    fn without_a_known_personal_room_nothing_is() {
        assert!(!read_position_is_authorized(PERSONAL, ME, None, ME));
    }
}
