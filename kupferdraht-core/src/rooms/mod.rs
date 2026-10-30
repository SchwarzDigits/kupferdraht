// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `rooms` — matrix-rust-sdk-shaped room types and the Wire→Room
//! adapter.
//!
//! Public surface (cycle 2b):
//!
//! * [`Room`] — a single conversation, read-only snapshot. Richer
//!   behaviour (send / receive / timeline) comes in Steps 4 – 6.
//! * [`RoomType`] — `DirectMessage` / `Group` / `Channel` / `SelfRoom`.
//!   Cycle 2b maps Wire's numeric `ConvType` onto this enum.
//! * [`RoomMember`] — one participant. Cycle 2b populates the raw
//!   qualified user id; cycle 2c backfills `display_name` / `handle` via
//!   `POST /v14/list-users`.
//!
//! The Wire `wire::dto::OwnConversation` → `Room` adapter lives in
//! [`room::room_from_conversation`]; Wire-specific terminology does
//! not cross out of that function.

mod devices;
mod room;

pub use devices::{DeviceCredential, RoomDevice, RoomDevicesChange, RoomDevicesListener};
pub(crate) use devices::{device_set_change, room_device_from_leaf};
pub use room::{JoinLink, Room, RoomAccess, RoomMember, RoomMemberRole, RoomType};

pub(crate) use room::{
    apply_self_profile, apply_user_profiles, non_self_qualified_ids, parse_room_id,
    room_from_conversation, room_to_records, rooms_from_records, with_epoch, with_members,
};
