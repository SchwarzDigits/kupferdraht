// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 145 — `conversation.create`, `rename`, `member-join` and
//! `member-leave` become timeline items. Display only: the membership the
//! commits change stays the MLS path's (`docs/backend-spec.md` §12.3).

use std::sync::Arc;

use tracing::warn;

use crate::client::Client;
use crate::error::KupferdrahtError;
use crate::identity::UserId;
use crate::storage::TimelineEventRecord;
use crate::time::now_ms;
use crate::timeline::room_event::{pack_room_event, room_event_id};
use crate::timeline::{
    DeliveryStatus, MembershipChange, RoomMembershipChange, TimelineDiff, TimelineItem,
    TimelineItemKind,
};
use crate::wire::dto::QualifiedId;
use crate::wire::room_event::{MemberLeftReason, RoomEvent, RoomEventData, parse_room_event};
use crate::wire::time::parse_time_ms;

/// Store one room event as a timeline item and emit its `Append`, once: a
/// redelivered event has the same id and writes nothing. Such an event may
/// still change the room list (Step 118). A malformed one is logged and
/// skipped, never a failed sync.
///
/// # Errors
/// `KupferdrahtError::Storage` from the write.
pub(crate) async fn dispatch_room_event(
    client: &Arc<Client>,
    event_type: &str,
    ev: &serde_json::Value,
) -> Result<(), KupferdrahtError> {
    client.mark_rooms_stale();
    let Some(record) = parse_room_event(event_type, ev).and_then(|e| record_of(event_type, &e))
    else {
        warn!(event_type, "room event malformed; skipping");
        return Ok(());
    };
    debug_assert!(!record.message_id.is_empty(), "a room event has an id");
    let Some(store) = client.store_handle() else {
        return Ok(());
    };
    if store
        .insert_timeline_event_with_mentions(&record, &[])
        .await?
    {
        client.dispatch_timeline_diff(&TimelineDiff::Append {
            room_id: record.room_id.clone(),
            item: TimelineItem::from_record(record),
        });
    }
    Ok(())
}

/// The row of `event`, or `None` when an id in it is unaddressable. Its
/// `sent_at_ms` is the event's `time` (Step 151), and its position this
/// device's clock like every other row's, so a server clock can neither sort
/// it against messages nor take a read marker past the next one.
fn record_of(event_type: &str, event: &RoomEvent) -> Option<TimelineEventRecord> {
    let room_id = qualified(&event.qualified_conversation)?;
    let actor = qualified(&event.qualified_from)?;
    let (kind, body) = pack_room_event(&item_kind(&event.data, &actor)?)?;
    let arrived_at_ms = now_ms();
    Some(TimelineEventRecord {
        message_id: room_event_id(event_type, &room_id, &actor, &event.time, &body),
        room_id,
        sender_user_id: actor,
        sender_device_id: String::new(),
        kind: kind.to_string(),
        body,
        sent_at_ms: parse_time_ms(&event.time).unwrap_or(arrived_at_ms),
        arrived_at_ms,
        delivery_status: DeliveryStatus::Received.as_db_str().into(),
        reply_to_id: None,
        edited_at_ms: None,
        redacted_at_ms: None,
        expires_at_ms: None,
        ephemeral_ttl_ms: None,
    })
}

fn item_kind(data: &RoomEventData, actor: &str) -> Option<TimelineItemKind> {
    Some(match data {
        RoomEventData::Create { name } => TimelineItemKind::RoomCreated { name: name.clone() },
        RoomEventData::Rename { name } => TimelineItemKind::RoomNameChange { name: name.clone() },
        RoomEventData::MemberJoin { users } => {
            let users = qualified_all(users)?;
            let change = if users.len() == 1 && users[0] == actor {
                MembershipChange::Joined
            } else {
                MembershipChange::Added
            };
            membership(change, actor, users)
        }
        RoomEventData::MemberLeave { users, reason } => {
            let change = match reason {
                // Wire names the deleted user as the actor: nobody removed them.
                MemberLeftReason::Left | MemberLeftReason::UserDeleted => MembershipChange::Left,
                MemberLeftReason::Removed => MembershipChange::Kicked,
            };
            membership(change, actor, qualified_all(users)?)
        }
    })
}

fn membership(change: MembershipChange, actor: &str, users: Vec<String>) -> TimelineItemKind {
    TimelineItemKind::MembershipChange(RoomMembershipChange {
        change,
        actor: actor.to_string(),
        users,
    })
}

fn qualified_all(users: &[QualifiedId]) -> Option<Vec<String>> {
    users.iter().map(qualified).collect()
}

/// Step 95c: backend JSON — a malformed id is treated as absent.
fn qualified(id: &QualifiedId) -> Option<String> {
    UserId::from_parts(&id.id, &id.domain)
        .ok()
        .map(|user_id| user_id.as_str().to_string())
}
