// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 146 — the rooms that changed since this client's previous summary,
//! for `SyncSummary::changed_rooms`. matrix-rust-sdk analog:
//! `sliding_sync::UpdateSummary::rooms`.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::PoisonError;

use crate::client::Client;
use crate::error::KupferdrahtError;
use crate::rooms::{Room, RoomMember};

/// What `rooms()` and `members()` show of a room: a difference in it is a
/// change. Epoch, group id and access are not shown and do not count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RoomSnapshot {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) members: Vec<RoomMember>,
    pub(crate) counterpart: Option<RoomMember>,
}

impl RoomSnapshot {
    pub(crate) fn of(room: &Room) -> Self {
        Self {
            id: room.id(),
            name: room.name(),
            members: room.members_slice().to_vec(),
            counterpart: room.counterpart(),
        }
    }

    /// The members in an order of their own, so a list the backend sends in
    /// another order is no change.
    fn keyed(self) -> (String, (String, Vec<RoomMember>, Option<RoomMember>)) {
        let mut members = self.members;
        members.sort_by(|a, b| a.user_id.cmp(&b.user_id));
        (self.id, (self.name, members, self.counterpart))
    }
}

/// The ids of the rooms that differ between `before` and `after`, a new and a
/// gone room included, in order.
pub(crate) fn changed_room_ids(before: &[RoomSnapshot], after: &[RoomSnapshot]) -> Vec<String> {
    let before: BTreeMap<_, _> = before.iter().cloned().map(RoomSnapshot::keyed).collect();
    let after: BTreeMap<_, _> = after.iter().cloned().map(RoomSnapshot::keyed).collect();
    let ids: BTreeSet<&String> = before.keys().chain(after.keys()).collect();
    ids.into_iter()
        .filter(|id| before.get(*id) != after.get(*id))
        .cloned()
        .collect()
}

impl Client {
    /// `room_id` changed: the next summary names it.
    pub(crate) fn note_room_changed(&self, room_id: &str) {
        debug_assert!(!room_id.is_empty(), "a changed room has an id");
        self.changed_rooms
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(room_id.to_string());
    }

    /// The rooms that changed since the previous call, in order; a summary
    /// takes them, so a failed pass leaves them for the next.
    pub(crate) fn take_changed_rooms(&self) -> Vec<String> {
        std::mem::take(
            &mut *self
                .changed_rooms
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        )
        .into_iter()
        .collect()
    }

    /// What this device knew of the room list before a refresh: the cache,
    /// or before the first refresh of a resumed client the stored rooms, so
    /// that it names only what changed while it was away (Step 146).
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` from reading the stored rooms.
    pub(crate) async fn known_room_list(&self) -> Result<Vec<RoomSnapshot>, KupferdrahtError> {
        let loaded = self.rooms_loaded.load(std::sync::atomic::Ordering::Acquire);
        if !loaded && let Some(store) = self.store_handle() {
            let stored = crate::rooms::rooms_from_records(store.load_rooms().await?, &self.weak());
            return Ok(stored.iter().map(RoomSnapshot::of).collect());
        }
        Ok(self.rooms_snapshot_now())
    }

    /// The cached list as it is now.
    fn rooms_snapshot_now(&self) -> Vec<RoomSnapshot> {
        self.rooms
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(|room| RoomSnapshot::of(room))
            .collect()
    }

    /// Note every room in which `rooms` differs from `before`.
    pub(crate) fn note_room_list_changes(&self, before: &[RoomSnapshot], rooms: &[Room]) {
        let after: Vec<RoomSnapshot> = rooms.iter().map(RoomSnapshot::of).collect();
        for id in changed_room_ids(before, &after) {
            self.note_room_changed(&id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{RoomSnapshot, changed_room_ids};
    use crate::rooms::{RoomMember, RoomMemberRole};

    fn member(user: &str, name: &str) -> RoomMember {
        RoomMember {
            user_id: user.into(),
            is_self: false,
            display_name: Some(name.into()),
            handle: None,
            role: RoomMemberRole::Member,
            avatar_thumbnail_key: None,
            avatar_full_key: None,
        }
    }

    fn room(id: &str, name: &str, members: Vec<RoomMember>) -> RoomSnapshot {
        RoomSnapshot {
            id: id.into(),
            name: name.into(),
            members,
            counterpart: None,
        }
    }

    #[test]
    fn the_same_list_in_another_order_is_no_change() {
        let a = room(
            "a@d",
            "A",
            vec![member("bob@d", "Bob"), member("eve@d", "Eve")],
        );
        let b = room("b@d", "B", vec![]);
        let reordered = room(
            "a@d",
            "A",
            vec![member("eve@d", "Eve"), member("bob@d", "Bob")],
        );

        assert_eq!(
            changed_room_ids(&[a, b.clone()], &[b, reordered]),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_name_a_member_a_new_and_a_gone_room_are_changes() {
        let before = [
            room("a@d", "A", vec![]),
            room("b@d", "B", vec![member("bob@d", "Bob")]),
            room("c@d", "C", vec![]),
            room("d@d", "D", vec![]),
        ];
        let after = [
            room("a@d", "A renamed", vec![]),
            room("b@d", "B", vec![member("bob@d", "Robert")]),
            room("c@d", "C", vec![]),
            room("e@d", "E", vec![]),
        ];

        assert_eq!(
            changed_room_ids(&before, &after),
            vec!["a@d", "b@d", "d@d", "e@d"]
        );
    }

    #[test]
    fn a_members_new_avatar_is_a_change() {
        let mut changed = member("bob@d", "Bob");
        changed.avatar_thumbnail_key = Some("3-1-new".into());

        assert_eq!(
            changed_room_ids(
                &[room("a@d", "A", vec![member("bob@d", "Bob")])],
                &[room("a@d", "A", vec![changed])]
            ),
            vec!["a@d"]
        );
    }
}
