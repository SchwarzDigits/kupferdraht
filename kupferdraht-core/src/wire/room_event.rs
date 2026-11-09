// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! The four room events shown in a timeline (Step 145), in Wire's field
//! names; their shapes are measured in `docs/wire/api-notes.md`, "Step 145
//! discovery".

use serde::Deserialize;

use super::dto::QualifiedId;

/// A parsed `conversation.create`, `conversation.rename`,
/// `conversation.member-join` or `conversation.member-leave` event.
#[derive(Debug)]
pub(crate) struct RoomEvent {
    pub(crate) qualified_conversation: QualifiedId,
    pub(crate) qualified_from: QualifiedId,
    pub(crate) time: String,
    pub(crate) data: RoomEventData,
}

#[derive(Debug)]
pub(crate) enum RoomEventData {
    Create {
        name: Option<String>,
    },
    Rename {
        name: String,
    },
    MemberJoin {
        users: Vec<QualifiedId>,
    },
    MemberLeave {
        users: Vec<QualifiedId>,
        reason: MemberLeftReason,
    },
}

/// Wire's `EdMemberLeftReason`.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum MemberLeftReason {
    Left,
    Removed,
    UserDeleted,
}

#[derive(Deserialize)]
struct Envelope {
    qualified_conversation: QualifiedId,
    qualified_from: QualifiedId,
    time: String,
    data: serde_json::Value,
}

#[derive(Deserialize)]
struct CreateData {
    name: Option<String>,
}

#[derive(Deserialize)]
struct RenameData {
    name: String,
}

#[derive(Deserialize)]
struct SimpleMember {
    qualified_id: QualifiedId,
}

#[derive(Deserialize)]
struct MemberJoinData {
    users: Vec<SimpleMember>,
}

#[derive(Deserialize)]
struct MemberLeaveData {
    qualified_user_ids: Vec<QualifiedId>,
    reason: MemberLeftReason,
}

/// Parse `ev` as the room event `event_type` names; `None` for another type
/// or a shape that does not fit, an event naming no user among them.
pub(crate) fn parse_room_event(event_type: &str, ev: &serde_json::Value) -> Option<RoomEvent> {
    let envelope = Envelope::deserialize(ev).ok()?;
    let data = parse_data(event_type, &envelope.data)?;
    Some(RoomEvent {
        qualified_conversation: envelope.qualified_conversation,
        qualified_from: envelope.qualified_from,
        time: envelope.time,
        data,
    })
}

fn parse_data(event_type: &str, data: &serde_json::Value) -> Option<RoomEventData> {
    match event_type {
        "conversation.create" => {
            let create = CreateData::deserialize(data).ok()?;
            Some(RoomEventData::Create { name: create.name })
        }
        "conversation.rename" => {
            let rename = RenameData::deserialize(data).ok()?;
            Some(RoomEventData::Rename { name: rename.name })
        }
        "conversation.member-join" => {
            let join = MemberJoinData::deserialize(data).ok()?;
            let users: Vec<QualifiedId> = join.users.into_iter().map(|u| u.qualified_id).collect();
            (!users.is_empty()).then_some(RoomEventData::MemberJoin { users })
        }
        "conversation.member-leave" => {
            let leave = MemberLeaveData::deserialize(data).ok()?;
            (!leave.qualified_user_ids.is_empty()).then_some(RoomEventData::MemberLeave {
                users: leave.qualified_user_ids,
                reason: leave.reason,
            })
        }
        _ => None,
    }
}
