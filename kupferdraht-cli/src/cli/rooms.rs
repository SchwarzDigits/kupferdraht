// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `kupferdraht-cli` room + DM command handlers.

use kupferdraht::{KupferdrahtError, SyncSettings};

use super::{resume_client, resume_client_and_store};

pub(crate) async fn rooms() -> Result<(), KupferdrahtError> {
    // resume_client_and_store stages the per-account DEK on the
    // returned store handle; without that, `load_rooms` cannot
    // decrypt `rooms.name_ct` / `room_members.display_name_ct`.
    let (client, store) = resume_client_and_store().await?;
    let rooms = store.load_rooms().await?;
    if rooms.is_empty() {
        println!("no rooms — run `kupferdraht-cli sync` first");
        return Ok(());
    }
    // Step 67: one grouped query for the whole list; missing = 0.
    let unread = client.unread_counts().await?;
    for (room, members) in rooms {
        println!("- {}", room.room_id);
        println!("    type:   {}", room.room_type);
        println!(
            "    unread: {}",
            unread.get(&room.room_id).copied().unwrap_or(0)
        );
        println!(
            "    name:   {}",
            if room.name.is_empty() {
                "—"
            } else {
                &room.name
            }
        );
        println!("    epoch:  {}", room.epoch);
        if let Some(cp_id) = room.counterpart_user_id.as_deref() {
            let display = members
                .iter()
                .find(|m| m.user_id == cp_id)
                .and_then(|m| m.display_name.as_deref())
                .unwrap_or("—");
            println!("    counterpart: {cp_id} ({display})");
        }
        println!("    members ({}):", members.len());
        for m in &members {
            let marker = if m.is_self { "*" } else { " " };
            let name = m.display_name.as_deref().unwrap_or("—");
            let handle = m
                .handle
                .as_deref()
                .map(|h| format!(" @{h}"))
                .unwrap_or_default();
            println!("      {marker} {} — {name}{handle}", m.user_id);
        }
    }
    Ok(())
}

pub(crate) async fn room_join(room_id: String) -> Result<(), KupferdrahtError> {
    let client = resume_client().await?;
    client.sync_once(SyncSettings::default()).await?;
    client.join_room(&room_id).await?;
    let after = client.room(&room_id).await.ok_or_else(|| {
        KupferdrahtError::InvalidInput(format!("room {room_id} missing after join"))
    })?;
    println!("room-join ok");
    println!("  room_id: {room_id}");
    println!("  epoch:   {}", after.epoch());
    Ok(())
}

pub(crate) async fn rooms_join_all() -> Result<(), KupferdrahtError> {
    let client = resume_client().await?;
    client.sync_once(SyncSettings::default()).await?;
    let summary = client.join_all_rooms().await?;
    println!("rooms-join-all");
    println!("  attempted:      {}", summary.attempted);
    println!("  joined:         {}", summary.joined);
    println!("  already_joined: {}", summary.already_joined);
    if summary.failed.is_empty() {
        println!("  failed:         0");
    } else {
        println!("  failed ({}):", summary.failed.len());
        for (id, err) in &summary.failed {
            println!("    - {id}: {err}");
        }
    }
    Ok(())
}

pub(crate) async fn rejoin_all() -> Result<(), KupferdrahtError> {
    let client = resume_client().await?;
    client.sync_metadata().await?;
    let summary = client.resync_existing_rooms().await?;
    println!("rejoin-all");
    println!("  attempted:       {}", summary.attempted);
    println!("  already_current: {}", summary.already_current);
    println!("  resynced:        {}", summary.resynced);
    if summary.failed.is_empty() {
        println!("  failed:          0");
    } else {
        println!("  failed ({}):", summary.failed.len());
        for (id, err) in &summary.failed {
            println!("    - {id}: {err}");
        }
    }
    Ok(())
}

pub(crate) async fn room_members(room_id: String) -> Result<(), KupferdrahtError> {
    let client = resume_client().await?;
    let members = client.sync_members(&room_id).await?;
    println!("room-members");
    println!("  room_id: {room_id}");
    println!("  count:   {}", members.len());
    for m in &members {
        let marker = if m.is_self { "*" } else { " " };
        let name = m.display_name.as_deref().unwrap_or("—");
        let handle = m
            .handle
            .as_deref()
            .map(|h| format!(" @{h}"))
            .unwrap_or_default();
        // Step 72a: surface the conversation role — the case-fold
        // gate (ADR-0018) keys off it.
        let role = match m.role {
            kupferdraht::RoomMemberRole::Administrator => "  [admin]",
            kupferdraht::RoomMemberRole::Member => "",
        };
        println!("  {marker} {} — {name}{handle}{role}", m.user_id);
    }
    Ok(())
}

pub(crate) async fn room_devices(room_id: String) -> Result<(), KupferdrahtError> {
    let client = resume_client().await?;
    let room = client.room_or_load(&room_id).await.ok_or_else(|| {
        KupferdrahtError::InvalidInput(format!("room-devices: unknown room {room_id} — run `sync`"))
    })?;
    let devices = room.devices().await?;
    let own = client.own_device_thumbprint().ok();
    println!("room-devices");
    println!("  room_id: {room_id}");
    println!("  count:   {}", devices.len());
    for device in &devices {
        let marker = if device.thumbprint.is_some() && device.thumbprint == own {
            "*"
        } else {
            " "
        };
        let who = match (&device.user_id, &device.device_id) {
            (Some(user), Some(dev)) => format!("{user} / {dev}"),
            _ => "(identity not parsed)".to_owned(),
        };
        let kind = match device.credential {
            kupferdraht::DeviceCredential::Basic => "basic",
            kupferdraht::DeviceCredential::X509 => "x509",
        };
        println!("  {marker} {who}  [{kind}]");
        println!(
            "      mls thumbprint: {}",
            device
                .thumbprint
                .as_deref()
                .map_or_else(|| "—".to_owned(), grouped_thumbprint)
        );
    }
    Ok(())
}

/// A thumbprint in 2-character groups — how the Wire apps render the
/// "MLS Thumbprint", so the two are easy to compare. Pure, unit-tested.
pub(crate) fn grouped_thumbprint(thumbprint: &str) -> String {
    thumbprint
        .as_bytes()
        .chunks(2)
        .map(|pair| String::from_utf8_lossy(pair).into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) async fn room_create(name: String) -> Result<(), KupferdrahtError> {
    let client = resume_client().await?;
    let room = client.create_room(&name).await?;
    println!("room-create ok");
    println!("  room_id: {}", room.id());
    println!("  name:    {}", room.name());
    println!("  epoch:   {}", room.epoch());
    Ok(())
}

pub(crate) async fn dm_create(peer: String) -> Result<(), KupferdrahtError> {
    let client = resume_client().await?;
    let room = client.create_dm(&peer).await?;
    print_dm_result("dm-create", &peer, &room);
    Ok(())
}

pub(crate) async fn dm_show(peer: String) -> Result<(), KupferdrahtError> {
    let client = resume_client().await?;
    // `Client::dm_room` reads the in-memory rooms cache. Each CLI
    // invocation is a fresh process, so the cache starts empty —
    // hydrate it from the persisted store first. No network. The
    // store mirror is the last `sync_metadata` snapshot, populated
    // by `kupferdraht-cli sync`.
    client.load_rooms_from_store().await?;
    let Some(room) = client.dm_room(&peer) else {
        eprintln!("dm-show: no cached DM with {peer}");
        std::process::exit(2);
    };
    print_dm_result("dm-show", &peer, &room);
    Ok(())
}

pub(crate) async fn dm_with(peer: String) -> Result<(), KupferdrahtError> {
    let client = resume_client().await?;
    // Hydrate the rooms cache from disk so `dm_with`'s fast-path
    // (cached DM → no Wire call) is actually reachable across CLI
    // invocations. On a cache miss `dm_with` falls through to
    // `create_dm` which hits Wire's idempotent GET.
    client.load_rooms_from_store().await?;
    let room = client.dm_with(&peer).await?;
    print_dm_result("dm-with", &peer, &room);
    Ok(())
}

pub(crate) async fn self_room() -> Result<(), KupferdrahtError> {
    let client = resume_client().await?;
    // `Client::self_room` reads the in-memory rooms cache — hydrate
    // it from the persisted snapshot first (no network), exactly like
    // `dm-show`. `kupferdraht-cli sync` populates the snapshot.
    client.load_rooms_from_store().await?;
    let Some(room) = client.self_room() else {
        eprintln!(
            "self-room: no self conversation in the cache — run `kupferdraht-cli sync` first"
        );
        std::process::exit(2);
    };
    println!("self-room ok");
    println!("  room_id: {}", room.id());
    println!("  epoch:   {}", room.epoch());
    if room.epoch() == 0 {
        println!(
            "  note:    not established yet — `rooms-join-all` establishes it from this device"
        );
    }
    Ok(())
}

pub(crate) fn print_dm_result(cmd: &str, peer: &str, room: &kupferdraht::Room) {
    println!("{cmd} ok");
    println!("  peer:    {peer}");
    println!("  room_id: {}", room.id());
    println!("  epoch:   {}", room.epoch());
}

pub(crate) async fn room_invite(
    room_id: String,
    users: Vec<String>,
) -> Result<(), KupferdrahtError> {
    let client = resume_client().await?;
    // Full drain, not just metadata (Step 75): the invite hook
    // re-broadcasts the inviter's CURRENT fold (case / marker /
    // tasks) — a one-shot CLI process must fold pending events
    // first, or the late joiner permanently misses them (MLS has
    // no history). Live-caught 2026-08-26.
    client.sync_once(SyncSettings::default()).await?;
    let room = client.invite_members(&room_id, &users).await?;
    let members = client.sync_members(&room_id).await?;
    println!("room-invite ok");
    println!("  room_id: {room_id}");
    println!("  invited: {}", users.len());
    println!("  epoch:   {}", room.epoch());
    println!("  members ({}):", members.len());
    for m in &members {
        let marker = if m.is_self { "*" } else { " " };
        let name = m.display_name.as_deref().unwrap_or("—");
        let handle = m
            .handle
            .as_deref()
            .map(|h| format!(" @{h}"))
            .unwrap_or_default();
        println!("    {marker} {} — {name}{handle}", m.user_id);
    }
    Ok(())
}

pub(crate) async fn room_kick(room_id: String, users: Vec<String>) -> Result<(), KupferdrahtError> {
    let client = resume_client().await?;
    client.sync_metadata().await?;
    let room = client.kick_members(&room_id, &users).await?;
    let members = client.sync_members(&room_id).await?;
    println!("room-kick ok");
    println!("  room_id: {room_id}");
    println!("  kicked:  {}", users.len());
    println!("  epoch:   {}", room.epoch());
    println!("  members ({}):", members.len());
    for m in &members {
        let marker = if m.is_self { "*" } else { " " };
        let name = m.display_name.as_deref().unwrap_or("—");
        let handle = m
            .handle
            .as_deref()
            .map(|h| format!(" @{h}"))
            .unwrap_or_default();
        println!("    {marker} {} — {name}{handle}", m.user_id);
    }
    Ok(())
}

pub(crate) async fn room_rename(room_id: String, name: String) -> Result<(), KupferdrahtError> {
    let client = resume_client().await?;
    client.rename_room(&room_id, &name).await?;
    println!("room-rename ok");
    println!("  room_id: {room_id}");
    println!("  name:    {name}");
    Ok(())
}

pub(crate) async fn room_leave(room_id: String) -> Result<(), KupferdrahtError> {
    let client = resume_client().await?;
    client.leave_room(&room_id).await?;
    println!("room-leave ok");
    println!("  room_id: {room_id}");
    Ok(())
}

pub(crate) async fn room_delete(room_id: String) -> Result<(), KupferdrahtError> {
    let client = resume_client().await?;
    client.delete_room(&room_id).await?;
    println!("room-delete ok");
    println!("  room_id: {room_id}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::grouped_thumbprint;

    #[test]
    fn thumbprints_print_in_two_character_groups() {
        assert_eq!(grouped_thumbprint("oKIywvG"), "oK Iy wv G");
        assert_eq!(grouped_thumbprint(""), "");
    }
}
