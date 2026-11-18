// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `kupferdraht-cli` room-access + join-link command handlers.

use kupferdraht::{JoinRule, Joiner, KupferdrahtError, RoomAccess};

use super::{parse_csv, resume_client};

/// The CLI's names of the join rules, as `room-access set --join` takes
/// them and `room-access show` prints them.
const JOIN_RULES: [(&str, JoinRule); 4] = [
    ("private", JoinRule::Private),
    ("invite", JoinRule::Invite),
    ("link", JoinRule::Link),
    ("join-link", JoinRule::JoinLink),
];

/// The CLI's names of the joiners, as `--allow` takes them.
const JOINERS: [(&str, Joiner); 4] = [
    ("team-members", Joiner::TeamMembers),
    ("non-team-members", Joiner::NonTeamMembers),
    ("guests", Joiner::Guests),
    ("services", Joiner::Services),
];

/// The values `names` lists, by the CLI's `table`; an unknown one or none
/// at all is `InvalidInput` naming `flag`.
fn parse_names<T: Copy>(
    names: &str,
    table: &[(&str, T)],
    flag: &str,
) -> Result<Vec<T>, KupferdrahtError> {
    let values = parse_csv(names)
        .iter()
        .map(|name| {
            table
                .iter()
                .find(|(known, _)| known == name)
                .map(|(_, value)| *value)
                .ok_or_else(|| {
                    let known: Vec<&str> = table.iter().map(|(known, _)| *known).collect();
                    KupferdrahtError::InvalidInput(format!(
                        "room-access set: {flag} {name:?} is none of {}",
                        known.join(", ")
                    ))
                })
        })
        .collect::<Result<Vec<T>, _>>()?;
    if values.is_empty() {
        return Err(KupferdrahtError::InvalidInput(format!(
            "room-access set: {flag} must name at least one value"
        )));
    }
    Ok(values)
}

/// `values` by the CLI's names in `table`, comma-separated.
fn names_of<T: PartialEq>(values: &[T], table: &[(&str, T)]) -> String {
    let names: Vec<&str> = values
        .iter()
        .filter_map(|value| table.iter().find(|(_, known)| known == value))
        .map(|(name, _)| *name)
        .collect();
    if names.is_empty() {
        "(none)".into()
    } else {
        names.join(", ")
    }
}

fn print_access(heading: &str, room_id: &str, access: &RoomAccess) {
    println!("{heading}");
    println!("  room_id:    {room_id}");
    println!(
        "  join_rules: [{}]",
        names_of(&access.join_rules, &JOIN_RULES)
    );
    println!("  allowed:    [{}]", names_of(&access.allowed, &JOINERS));
}

pub(crate) async fn room_access_show(room_id: String) -> Result<(), KupferdrahtError> {
    let client = resume_client().await?;
    let room = client.room(&room_id).await.ok_or_else(|| {
        KupferdrahtError::InvalidInput(format!("room-access show: {room_id} not in cache"))
    })?;
    let access = room.access().await?;
    print_access("room-access", &room_id, &access);
    Ok(())
}

pub(crate) async fn room_access_set(
    room_id: String,
    join: String,
    allow: String,
) -> Result<(), KupferdrahtError> {
    let access = RoomAccess {
        join_rules: parse_names(&join, &JOIN_RULES, "--join")?,
        allowed: parse_names(&allow, &JOINERS, "--allow")?,
    };
    let client = resume_client().await?;
    client.set_room_access(&room_id, &access).await?;
    print_access("room-access updated", &room_id, &access);
    Ok(())
}

pub(crate) async fn room_access_allow_guests(room_id: String) -> Result<(), KupferdrahtError> {
    let client = resume_client().await?;
    let room = client.set_room_access(&room_id, &guests()).await?;
    print_access(
        "room-access open to guests",
        &room_id,
        &room.cached_access(),
    );
    Ok(())
}

/// What `Room::allow_guests` sets: members invite, and the join link
/// admits team members, other users and guests.
fn guests() -> RoomAccess {
    RoomAccess {
        join_rules: vec![JoinRule::Invite, JoinRule::JoinLink],
        allowed: vec![Joiner::TeamMembers, Joiner::NonTeamMembers, Joiner::Guests],
    }
}

pub(crate) async fn join_link_generate(
    room_id: String,
    password: Option<String>,
) -> Result<(), KupferdrahtError> {
    // Bypass `client.room()` (which reads from the in-memory cache
    // that resume_client doesn't hydrate) and call the Client method
    // directly — `create_join_link` only needs the qualified id.
    let client = resume_client().await?;
    let link = client
        .create_join_link(&room_id, password.as_deref())
        .await?;
    println!("join-link ok");
    println!("  room_id:            {room_id}");
    println!("  url:                {}", link.url);
    println!(
        "  password_protected: {}",
        if link.password_protected { "yes" } else { "no" }
    );
    Ok(())
}

pub(crate) async fn join_link_revoke(room_id: String) -> Result<(), KupferdrahtError> {
    let client = resume_client().await?;
    client.revoke_join_link(&room_id).await?;
    println!("join-link revoked");
    println!("  room_id: {room_id}");
    Ok(())
}

pub(crate) async fn join_link_current(room_id: String) -> Result<(), KupferdrahtError> {
    let client = resume_client().await?;
    match client.current_join_link(&room_id).await? {
        Some(link) => {
            println!("join-link present");
            println!("  room_id:            {room_id}");
            println!("  url:                {}", link.url);
            println!(
                "  password_protected: {}",
                if link.password_protected { "yes" } else { "no" }
            );
        }
        None => println!("(no link issued)"),
    }
    Ok(())
}

pub(crate) async fn join_via_link(
    url: String,
    password: Option<String>,
) -> Result<(), KupferdrahtError> {
    let client = resume_client().await?;
    let room = client.join_via_link(url, password).await?;
    println!("join-via-link ok");
    println!("  room_id: {}", room.id());
    Ok(())
}
