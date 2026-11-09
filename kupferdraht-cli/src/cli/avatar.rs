// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `kupferdraht-cli` avatar command handlers (Step 144).

use std::path::{Path, PathBuf};

use kupferdraht::{Avatar, AvatarSize, KupferdrahtError, UserId};

use super::resume_client;

fn parse_size(size: &str) -> Result<AvatarSize, KupferdrahtError> {
    match size {
        "thumbnail" => Ok(AvatarSize::Thumbnail),
        "full" => Ok(AvatarSize::Full),
        other => Err(KupferdrahtError::InvalidInput(format!(
            "--size is thumbnail or full, not {other}"
        ))),
    }
}

fn write_image(avatar: Option<Avatar>, out: &Path) -> Result<(), KupferdrahtError> {
    println!("avatar get ok");
    let Some(avatar) = avatar else {
        println!("  avatar: —");
        return Ok(());
    };
    std::fs::write(out, &avatar.bytes)
        .map_err(|e| KupferdrahtError::Storage(format!("write {}: {e}", out.display())))?;
    println!("  mime:   {}", avatar.mime_type);
    println!("  bytes:  {}", avatar.bytes.len());
    println!("  out:    {}", out.display());
    Ok(())
}

pub(crate) async fn get_user(
    user: String,
    size: String,
    out: PathBuf,
) -> Result<(), KupferdrahtError> {
    let size = parse_size(&size)?;
    let user = UserId::parse(&user)?;
    let client = resume_client().await?;
    write_image(client.avatar(&user, size).await?, &out)
}

pub(crate) async fn get_team(out: PathBuf) -> Result<(), KupferdrahtError> {
    let client = resume_client().await?;
    write_image(client.team_avatar().await?, &out)
}

pub(crate) async fn upload(path: PathBuf) -> Result<(), KupferdrahtError> {
    let image = std::fs::read(&path)
        .map_err(|e| KupferdrahtError::InvalidInput(format!("read {}: {e}", path.display())))?;
    let client = resume_client().await?;
    client.upload_avatar(image).await?;
    let own = client.account().user_id().clone();
    println!("avatar upload ok");
    println!(
        "  avatar_key: {}",
        client.avatar_key(&own).await?.as_deref().unwrap_or("—")
    );
    Ok(())
}

pub(crate) async fn remove() -> Result<(), KupferdrahtError> {
    let client = resume_client().await?;
    client.remove_avatar().await?;
    println!("avatar remove ok");
    Ok(())
}
