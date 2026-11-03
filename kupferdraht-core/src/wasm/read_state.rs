// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 138 — the browser projection of a room's read state: the three
//! fields `rooms()` carries per room and the payload of `startSync`'s
//! `onRoomsChanged`. A missing marker crosses as `null`, not `undefined`:
//! the hand-written types of the tabs module say `number | null`, and a
//! thin tab compares against it.

use serde::Serialize;
use wasm_bindgen::JsValue;

use crate::storage::ReadStateRecord;

/// Plain, JS-serializable view of a room — `{ roomId, name, unreadCount,
/// lastReadAtMs, markedUnread }`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoomView {
    pub room_id: String,
    pub name: String,
    pub unread_count: u32,
    pub last_read_at_ms: Option<i64>,
    pub marked_unread: bool,
}

/// The payload of `onRoomsChanged`: the room's row without its name —
/// `{ roomId, unreadCount, lastReadAtMs, markedUnread }`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadStateChange {
    pub room_id: String,
    pub unread_count: u32,
    pub last_read_at_ms: Option<i64>,
    pub marked_unread: bool,
}

/// A room's row of `rooms()`; `state` is its marker row, absent for a room
/// never read and never flagged.
#[must_use]
pub fn room_view(
    room_id: &str,
    name: &str,
    unread_count: u32,
    state: Option<&ReadStateRecord>,
) -> RoomView {
    RoomView {
        room_id: room_id.to_owned(),
        name: name.to_owned(),
        unread_count,
        last_read_at_ms: state.and_then(|s| s.last_read_at_ms),
        marked_unread: state.is_some_and(|s| s.marked_unread),
    }
}

/// The `onRoomsChanged` payload for `room_id` after a local change.
#[must_use]
pub fn read_state_change(
    room_id: &str,
    unread_count: u32,
    state: Option<&ReadStateRecord>,
) -> ReadStateChange {
    ReadStateChange {
        room_id: room_id.to_owned(),
        unread_count,
        last_read_at_ms: state.and_then(|s| s.last_read_at_ms),
        marked_unread: state.is_some_and(|s| s.marked_unread),
    }
}

/// Serialize a read-state view with `None` as `null`.
///
/// # Errors
/// An `Encoding` error when the value cannot be serialized (never for the
/// two views above).
pub fn to_js<T: Serialize>(view: &T) -> Result<JsValue, JsValue> {
    let serializer = serde_wasm_bindgen::Serializer::new().serialize_missing_as_null(true);
    view.serialize(&serializer)
        .map_err(|e| super::manager::named_error("Encoding", &e.to_string()))
}
