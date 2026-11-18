// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Who may join a room, and how (Step 14d, typed by Step 163c).
//!
//! The backend keeps two sets per room: the ways a user can join and the
//! kinds of users allowed to. [`RoomAccess`] holds both as plain names —
//! join rules after matrix-rust-sdk's `JoinRule` where it has one, joiners
//! where it has none. Both sets are closed in the backend's API, so every
//! state it can hold has a value here; its strings stay in `wire/`.

use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::wire::dto::{Access, AccessRole};

/// How a user can join a room.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, uniffi::Enum)]
#[serde(rename_all = "camelCase")]
pub enum JoinRule {
    /// Nobody joins: the room keeps the members it has (a personal room,
    /// a 1:1).
    Private,
    /// A member adds the user.
    Invite,
    /// By a link the backend defines; Kupferdraht never sets it and reads
    /// it back as it is.
    Link,
    /// By the room's join link ([`crate::rooms::JoinLink`], issued by
    /// `Room::create_join_link`).
    JoinLink,
}

/// Which users may join a room.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, uniffi::Enum)]
#[serde(rename_all = "camelCase")]
pub enum Joiner {
    /// Members of the room's team.
    TeamMembers,
    /// Users with an account outside the room's team.
    NonTeamMembers,
    /// Users without an account, by a join link.
    Guests,
    /// Services (bots).
    Services,
}

/// Who may join a room, and how. Step 14d; typed by Step 163c.
/// matrix-rust-sdk keeps a single join rule in room state; the backend keeps
/// both sets on the room itself, so a room can permit several join rules.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct RoomAccess {
    /// The ways a user can join.
    pub join_rules: Vec<JoinRule>,
    /// The users allowed to join.
    pub allowed: Vec<Joiner>,
}

impl From<Access> for JoinRule {
    fn from(access: Access) -> Self {
        match access {
            Access::Private => Self::Private,
            Access::Invite => Self::Invite,
            Access::Link => Self::Link,
            Access::Code => Self::JoinLink,
        }
    }
}

impl From<JoinRule> for Access {
    fn from(rule: JoinRule) -> Self {
        match rule {
            JoinRule::Private => Self::Private,
            JoinRule::Invite => Self::Invite,
            JoinRule::Link => Self::Link,
            JoinRule::JoinLink => Self::Code,
        }
    }
}

impl From<AccessRole> for Joiner {
    fn from(role: AccessRole) -> Self {
        match role {
            AccessRole::TeamMember => Self::TeamMembers,
            AccessRole::NonTeamMember => Self::NonTeamMembers,
            AccessRole::Guest => Self::Guests,
            AccessRole::Service => Self::Services,
        }
    }
}

impl From<Joiner> for AccessRole {
    fn from(joiner: Joiner) -> Self {
        match joiner {
            Joiner::TeamMembers => Self::TeamMember,
            Joiner::NonTeamMembers => Self::NonTeamMember,
            Joiner::Guests => Self::Guest,
            Joiner::Services => Self::Service,
        }
    }
}

impl RoomAccess {
    /// The access that lets guests join by the room's join link: members
    /// invite, and the link admits team members, other users and guests —
    /// what `Room::allow_guests` sets.
    #[must_use]
    pub(crate) fn for_guests() -> Self {
        Self {
            join_rules: vec![JoinRule::Invite, JoinRule::JoinLink],
            allowed: vec![Joiner::TeamMembers, Joiner::NonTeamMembers, Joiner::Guests],
        }
    }

    /// The access the backend reports as its two string sets. A value this
    /// client does not know is left out and logged — never an invented
    /// permission.
    pub(crate) fn from_wire(access: &[String], access_role: &[String]) -> Self {
        Self {
            join_rules: access
                .iter()
                .filter_map(|raw| known(raw, Access::from_wire(raw)))
                .map(JoinRule::from)
                .collect(),
            allowed: access_role
                .iter()
                .filter_map(|raw| known(raw, AccessRole::from_wire(raw)))
                .map(Joiner::from)
                .collect(),
        }
    }

    /// The two string sets the backend takes for this access.
    pub(crate) fn to_wire(&self) -> (Vec<String>, Vec<String>) {
        let access = self
            .join_rules
            .iter()
            .map(|rule| Access::from(*rule).as_wire().to_owned());
        let roles = self
            .allowed
            .iter()
            .map(|joiner| AccessRole::from(*joiner).as_wire().to_owned());
        (access.collect(), roles.collect())
    }
}

/// `value` when the backend's `raw` was known; logs it otherwise.
fn known<T>(raw: &str, value: Option<T>) -> Option<T> {
    if value.is_none() {
        warn!(
            value = raw,
            "room access: a value this client does not know, left out"
        );
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every subset of `all`, in `all`'s order.
    fn subsets<T: Copy>(all: &[T]) -> Vec<Vec<T>> {
        (0..1u32 << all.len())
            .map(|bits| {
                all.iter()
                    .enumerate()
                    .filter(|(i, _)| bits & (1 << i) != 0)
                    .map(|(_, value)| *value)
                    .collect()
            })
            .collect()
    }

    /// Step 163c: every combination of the backend's two sets — 16 x 16 —
    /// reads into a typed access and writes back unchanged.
    #[test]
    fn every_combination_the_backend_accepts_round_trips() {
        for access in subsets(&Access::ALL) {
            for roles in subsets(&AccessRole::ALL) {
                let access: Vec<String> = access.iter().map(|a| a.as_wire().to_owned()).collect();
                let roles: Vec<String> = roles.iter().map(|r| r.as_wire().to_owned()).collect();
                let typed = RoomAccess::from_wire(&access, &roles);
                assert_eq!(typed.to_wire(), (access.clone(), roles.clone()));
                assert_eq!(typed.join_rules.len(), access.len());
                assert_eq!(typed.allowed.len(), roles.len());
            }
        }
    }

    #[test]
    fn a_value_the_client_does_not_know_is_left_out() {
        let typed = RoomAccess::from_wire(
            &[Access::Invite.as_wire().to_owned(), "everyone".to_owned()],
            &["robots".to_owned()],
        );
        assert_eq!(
            typed,
            RoomAccess {
                join_rules: vec![JoinRule::Invite],
                allowed: Vec::new(),
            }
        );
    }

    #[test]
    fn guests_join_by_the_join_link() {
        let (access, roles) = RoomAccess::for_guests().to_wire();
        assert_eq!(
            access,
            [Access::Invite, Access::Code].map(|a| a.as_wire().to_owned())
        );
        assert_eq!(
            roles,
            [
                AccessRole::TeamMember,
                AccessRole::NonTeamMember,
                AccessRole::Guest
            ]
            .map(|r| r.as_wire().to_owned())
        );
    }
}
