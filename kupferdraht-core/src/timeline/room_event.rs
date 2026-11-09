// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 145 — room events as timeline items: who joined, left, renamed or
//! created a room. Data, not sentences: the embedder words them.
//!
//! matrix-rust-sdk analog: `matrix_sdk_ui::timeline::{MembershipChange,
//! RoomMembershipChange}`, here one item per backend event with a list of
//! users where matrix has one item per user.

use sha2::{Digest as _, Sha256};

use crate::client::Client;
use crate::error::KupferdrahtError;
use crate::timeline::TimelineItemKind;

const MEMBERSHIP_CHANGE_KIND: &str = "membership_change";
const ROOM_NAME_CHANGE_KIND: &str = "room_name_change";
const ROOM_CREATED_KIND: &str = "room_created";
/// Changing it, or what is hashed under it, stands a redelivered event twice.
const ROOM_EVENT_ID_TAG: &str = "kupferdraht.room-event.v1";

/// What happened to the users of a [`RoomMembershipChange`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum MembershipChange {
    /// The user joined through a join link; no other member added them.
    Joined,
    /// Another member added the users.
    Added,
    /// The user left, or their account was deleted.
    Left,
    /// Another member removed the users.
    Kicked,
}

/// One membership event of a room. `actor` and `users` are qualified user
/// ids; `users` names the own user where it is one of them. The actor is the
/// backend's statement, not authenticated by MLS.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct RoomMembershipChange {
    pub change: MembershipChange,
    pub actor: String,
    pub users: Vec<String>,
}

impl MembershipChange {
    /// The name the browser surface and the stored row use.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Joined => "joined",
            Self::Added => "added",
            Self::Left => "left",
            Self::Kicked => "kicked",
        }
    }

    fn from_str(raw: &str) -> Option<Self> {
        match raw {
            "joined" => Some(Self::Joined),
            "added" => Some(Self::Added),
            "left" => Some(Self::Left),
            "kicked" => Some(Self::Kicked),
            _ => None,
        }
    }
}

/// `timeline_events.body` of a membership item.
#[derive(serde::Serialize, serde::Deserialize)]
struct MembershipBody {
    change: String,
    actor: String,
    users: Vec<String>,
}

/// `timeline_events.body` of a rename or a creation.
#[derive(serde::Serialize, serde::Deserialize)]
struct NameBody {
    name: Option<String>,
}

/// Whether `kind` of a `timeline_events` row is a room event, not a message.
pub(crate) fn is_room_event_kind(kind: &str) -> bool {
    matches!(
        kind,
        MEMBERSHIP_CHANGE_KIND | ROOM_NAME_CHANGE_KIND | ROOM_CREATED_KIND
    )
}

/// The `(kind, body)` a room event item is stored as; `None` for any other
/// kind of item.
pub(crate) fn pack_room_event(kind: &TimelineItemKind) -> Option<(&'static str, String)> {
    let (row_kind, body) = match kind {
        TimelineItemKind::MembershipChange(change) => (
            MEMBERSHIP_CHANGE_KIND,
            serde_json::to_string(&MembershipBody {
                change: change.change.as_str().to_string(),
                actor: change.actor.clone(),
                users: change.users.clone(),
            }),
        ),
        TimelineItemKind::RoomNameChange { name } => (
            ROOM_NAME_CHANGE_KIND,
            serde_json::to_string(&NameBody {
                name: Some(name.clone()),
            }),
        ),
        TimelineItemKind::RoomCreated { name } => (
            ROOM_CREATED_KIND,
            serde_json::to_string(&NameBody { name: name.clone() }),
        ),
        _ => return None,
    };
    Some((row_kind, body.ok()?))
}

/// Read a stored room event back; `None` for a malformed body (the caller
/// degrades to a Text fallback, as for the other packed kinds).
pub(crate) fn room_event_kind_from_row(kind: &str, body: &str) -> Option<TimelineItemKind> {
    match kind {
        MEMBERSHIP_CHANGE_KIND => {
            let packed: MembershipBody = serde_json::from_str(body).ok()?;
            Some(TimelineItemKind::MembershipChange(RoomMembershipChange {
                change: MembershipChange::from_str(&packed.change)?,
                actor: packed.actor,
                users: packed.users,
            }))
        }
        ROOM_NAME_CHANGE_KIND => {
            let packed: NameBody = serde_json::from_str(body).ok()?;
            Some(TimelineItemKind::RoomNameChange { name: packed.name? })
        }
        ROOM_CREATED_KIND => {
            let packed: NameBody = serde_json::from_str(body).ok()?;
            Some(TimelineItemKind::RoomCreated { name: packed.name })
        }
        _ => None,
    }
}

/// An item's id, from the event's content and not its notification, so the
/// same event arriving twice keeps one row: the first 16 bytes of SHA-256
/// over a JSON array, as a UUID of version 8, the shape of every item id.
pub(crate) fn room_event_id(
    event_type: &str,
    room_id: &str,
    actor: &str,
    time: &str,
    body: &str,
) -> String {
    let input =
        serde_json::json!([ROOM_EVENT_ID_TAG, event_type, room_id, actor, time, body]).to_string();
    let digest = Sha256::digest(input.as_bytes());
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    uuid::Builder::from_custom_bytes(bytes)
        .into_uuid()
        .to_string()
}

impl Client {
    /// Whether `target_id` in `room_id` is a room event's item.
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` / `Encoding` from the lookup.
    pub(crate) async fn is_room_event_item(
        &self,
        room_id: &str,
        target_id: &str,
    ) -> Result<bool, KupferdrahtError> {
        let Some(store) = self.store_handle() else {
            return Ok(false);
        };
        Ok(store
            .load_timeline_event_in_room(room_id, target_id)
            .await?
            .is_some_and(|row| is_room_event_kind(&row.kind)))
    }

    /// `InvalidInput` when `target_id` is a room event's item: it is no
    /// message to react to, quote, edit or delete.
    ///
    /// # Errors
    /// `InvalidInput` for such an item; otherwise as
    /// [`Self::is_room_event_item`].
    pub(crate) async fn ensure_not_room_event(
        &self,
        room_id: &str,
        target_id: &str,
        op: &str,
    ) -> Result<(), KupferdrahtError> {
        if self.is_room_event_item(room_id, target_id).await? {
            return Err(KupferdrahtError::InvalidInput(format!(
                "{op}: {target_id} is a room event, not a message"
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::MembershipChange;

    #[test]
    fn each_change_has_the_browsers_name() {
        assert_eq!(MembershipChange::Joined.as_str(), "joined");
        assert_eq!(MembershipChange::Added.as_str(), "added");
        assert_eq!(MembershipChange::Left.as_str(), "left");
        assert_eq!(MembershipChange::Kicked.as_str(), "kicked");
    }
}
