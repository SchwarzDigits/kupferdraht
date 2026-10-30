// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `kupferdraht-cli` asset command handlers (send-file / download-asset).

use std::path::PathBuf;

use kupferdraht::KupferdrahtError;

use super::timeline::resolve_room_for_message;
use super::{resume_client, resume_with_room};

pub(crate) async fn send_file(
    room_id: String,
    path: PathBuf,
    mime: String,
) -> Result<(), KupferdrahtError> {
    let (_client, room, room_id) = resume_with_room(room_id).await?;
    let timeline = room.timeline()?;
    let handle = timeline.send_file(&path, &mime).await?;
    println!("send-file ok");
    println!("  room_id:    {room_id}");
    println!("  path:       {}", path.display());
    println!("  mime:       {mime}");
    println!("  message_id: {}", handle.message_id);
    println!("  sent_at:    {}", super::fmt_ms(handle.sent_at_ms));
    Ok(())
}

pub(crate) async fn download_asset(
    message_id: String,
    room_id: Option<String>,
    save_to: Option<PathBuf>,
) -> Result<(), KupferdrahtError> {
    let client = resume_client().await?;
    // Hydrate the in-memory rooms cache from the local store
    // — `Timeline::asset` needs the Room handle to be present.
    // No Wire round-trip needed: the attachment row lives in our
    // own store, not Wire's. Saves a /v14/conversations/list call
    // compared to `sync_metadata`.
    client.load_rooms_from_store().await?;
    let room_id = resolve_room_for_message(&client, room_id, &message_id).await?;
    let room = client
        .room(&room_id)
        .await
        .ok_or_else(|| KupferdrahtError::InvalidInput(format!("room {room_id} not in cache")))?;
    let timeline = room.timeline()?;
    let summary = timeline.asset(&message_id).await?.ok_or_else(|| {
        KupferdrahtError::InvalidInput(format!(
            "no asset row for message {message_id} in room {room_id} \
                 (run `sync-fetch` first; or the message isn't an asset)"
        ))
    })?;
    if let Some(dest) = save_to {
        summary.save_to(&dest).await?;
        println!("download-asset ok");
        println!("  message_id: {message_id}");
        println!("  room_id:    {room_id}");
        println!(
            "  filename:   {}",
            summary.filename.as_deref().unwrap_or("—")
        );
        println!("  mime:       {}", summary.mime_type);
        println!("  size_bytes: {}", summary.size_bytes);
        println!("  sender:     {}", summary.sender_user_id);
        println!("  saved_to:   {}", dest.display());
    } else {
        println!("download-asset (metadata only — pass --save-to <path> to decrypt)");
        println!("  message_id:  {message_id}");
        println!("  room_id:     {room_id}");
        println!(
            "  filename:    {}",
            summary.filename.as_deref().unwrap_or("—")
        );
        println!("  mime:        {}", summary.mime_type);
        println!("  size_bytes:  {}", summary.size_bytes);
        println!("  sender:      {}", summary.sender_user_id);
        println!("  asset_id:    {}", summary.asset_id);
        println!("  asset_dom:   {}", summary.asset_domain);
        println!("  encryption:  {}", summary.encryption_algo);
        // The cache_path field isn't pub on AssetSummary (it's
        // pub(crate)) so we don't surface it here. The download
        // path itself will hit cache on second run.
    }
    Ok(())
}
