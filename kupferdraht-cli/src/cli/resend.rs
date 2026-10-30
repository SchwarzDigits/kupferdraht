// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `kupferdraht-cli` failed-send recovery commands (Step 79): `failed-sends`
//! lists a room's `Failed` rows, `resend` re-drives one of them or the
//! whole room oldest-first — the CLI form of the explicit consumer
//! loop the reconnect contract prescribes (no scheduler in core).

use kupferdraht::KupferdrahtError;

use super::{resume_with_room, resume_with_room_for_message};

/// `resend --message <id>` (room auto-resolved when `--id` is absent)
/// or `resend --id <room> --all`.
pub(crate) async fn resend(
    room_id: Option<String>,
    message_id: Option<String>,
    all: bool,
) -> Result<(), KupferdrahtError> {
    if all {
        let room_id = room_id.ok_or_else(|| {
            KupferdrahtError::InvalidInput("resend --all needs --id <room>".into())
        })?;
        return resend_all(room_id).await;
    }
    let message_id = message_id.ok_or_else(|| {
        KupferdrahtError::InvalidInput("resend needs --message <id> or --all".into())
    })?;
    resend_one(room_id, message_id).await
}

async fn resend_one(room_id: Option<String>, message_id: String) -> Result<(), KupferdrahtError> {
    let (_client, room, room_id) = resume_with_room_for_message(room_id, &message_id).await?;
    let handle = room.timeline()?.resend(message_id).await?;
    println!("resend ok");
    println!("  room_id:    {room_id}");
    println!("  message_id: {}", handle.message_id);
    println!("  sent_at:    {}", super::fmt_ms(handle.sent_at_ms));
    Ok(())
}

async fn resend_all(room_id: String) -> Result<(), KupferdrahtError> {
    let (_client, room, room_id) = resume_with_room(room_id).await?;
    let summary = room.timeline()?.resend_failed().await?;
    println!("resend ok");
    println!("  room_id:   {room_id}");
    println!("  resent:    {}", summary.resent.len());
    for handle in &summary.resent {
        println!(
            "    {}  sent_at {}",
            handle.message_id,
            super::fmt_ms(handle.sent_at_ms)
        );
    }
    println!("  skipped:   {}", summary.skipped.len());
    for id in &summary.skipped {
        println!("    {id}  (snapshot row — re-send the current state instead)");
    }
    if let Some((id, error)) = &summary.stopped_at {
        println!("  stopped:   {id}: {error}");
    }
    println!("  remaining: {}", summary.remaining);
    Ok(())
}

/// `failed-sends --id <room>`: the rows `resend` can re-drive, oldest
/// first, in the `timeline` rendering.
pub(crate) async fn failed_sends(room_id: String) -> Result<(), KupferdrahtError> {
    let (_client, room, room_id) = resume_with_room(room_id).await?;
    let failed = room.failed_sends().await?;
    println!("failed-sends ok");
    println!("  room_id: {room_id}");
    println!("  failed:  {}", failed.len());
    for item in &failed {
        let (preview, reply_to, _) = super::timeline::item_preview(item);
        let reply = reply_to
            .map(|p| format!("  (reply to {p})"))
            .unwrap_or_default();
        println!(
            "    [{}] \u{2717} ({})  {preview}{reply}",
            super::fmt_ms(item.sent_at_ms),
            item.item_id
        );
    }
    Ok(())
}
