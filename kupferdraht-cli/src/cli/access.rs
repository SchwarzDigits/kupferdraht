// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `kupferdraht-cli` room-access + guest-link command handlers.

use kupferdraht::KupferdrahtError;

use super::{parse_csv, resume_client};

pub(crate) async fn room_access_show(room_id: String) -> Result<(), KupferdrahtError> {
    let client = resume_client().await?;
    let room = client.room(&room_id).await.ok_or_else(|| {
        KupferdrahtError::InvalidInput(format!("room-access show: {room_id} not in cache"))
    })?;
    let access = room.access().await?;
    println!("room-access");
    println!("  room_id:     {room_id}");
    println!(
        "  access:      [{}]",
        if access.modes.is_empty() {
            String::from("(empty)")
        } else {
            access.modes.join(", ")
        }
    );
    println!(
        "  access_role: [{}]",
        if access.roles.is_empty() {
            String::from("(empty)")
        } else {
            access.roles.join(", ")
        }
    );
    Ok(())
}

pub(crate) async fn room_access_set(
    room_id: String,
    modes: String,
    roles: String,
) -> Result<(), KupferdrahtError> {
    let client = resume_client().await?;
    let modes = parse_csv(&modes);
    let roles = parse_csv(&roles);
    if modes.is_empty() {
        return Err(KupferdrahtError::InvalidInput(
            "room-access set: --modes must contain at least one value".into(),
        ));
    }
    if roles.is_empty() {
        return Err(KupferdrahtError::InvalidInput(
            "room-access set: --roles must contain at least one value".into(),
        ));
    }
    client.set_room_access(&room_id, &modes, &roles).await?;
    println!("room-access updated");
    println!("  room_id:     {room_id}");
    println!("  access:      [{}]", modes.join(", "));
    println!("  access_role: [{}]", roles.join(", "));
    Ok(())
}

pub(crate) async fn room_access_allow_guests(room_id: String) -> Result<(), KupferdrahtError> {
    let client = resume_client().await?;
    let modes = vec!["invite".to_string(), "code".to_string()];
    let roles = vec![
        "team_member".to_string(),
        "non_team_member".to_string(),
        "guest".to_string(),
    ];
    client.set_room_access(&room_id, &modes, &roles).await?;
    println!("room-access opened to guests");
    println!("  room_id:     {room_id}");
    println!("  access:      [{}]", modes.join(", "));
    println!("  access_role: [{}]", roles.join(", "));
    Ok(())
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
    code: String,
    key: String,
    password: Option<String>,
) -> Result<(), KupferdrahtError> {
    let client = resume_client().await?;
    let room = client.join_via_link(code, key, password).await?;
    println!("join-via-link ok");
    println!("  room_id: {}", room.id());
    Ok(())
}
