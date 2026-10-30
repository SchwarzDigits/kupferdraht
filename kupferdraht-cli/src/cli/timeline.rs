// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `kupferdraht-cli` timeline command handlers (send / reply / edit / react / …).

use kupferdraht::KupferdrahtError;
use kupferdraht::timeline::{DeliveryStatus, MessagesOptions, TimelineItemKind};

use super::{resume_client, resume_with_room, resume_with_room_for_message};

pub(crate) async fn timeline(
    room_id: String,
    limit: Option<u32>,
    before: Option<String>,
) -> Result<(), KupferdrahtError> {
    let client = resume_client().await?;
    // The library's `Client::messages` is local-only by design
    // (matrix-rust-sdk shape — Timeline::items reads the cache,
    // a separate sync task pushes into it). For the one-shot CLI
    // we drain pending notifications first so the user sees
    // inbound messages they may not have explicitly sync-fetched.
    // Long-running consumers should keep `sync-stream` alive
    // separately and treat `timeline` as a pure cache read.
    let summary = client
        .sync_once(kupferdraht::SyncSettings::default())
        .await?;
    let drained = summary.events_dispatched;
    let mut opts = MessagesOptions::all();
    if let Some(limit) = limit {
        opts = opts.with_limit(limit);
    }
    if let Some(b) = before {
        opts = opts.before(&b);
    }
    let items = client.messages(&room_id, &opts).await?;
    println!("timeline ok");
    println!("  room_id:  {room_id}");
    println!("  drained:  {drained}");
    println!("  items:    {}", items.len());
    for item in &items {
        let when = super::fmt_ms(item.sent_at_ms);
        let sender = &item.sender_user_id;
        let marker = match item.delivery_status {
            DeliveryStatus::Sending => "…",
            DeliveryStatus::Submitted => "→",
            DeliveryStatus::Received => "←",
            DeliveryStatus::Failed => "✗",
        };
        let (body_preview, reply_to, mentions) = item_preview(item);
        // Step 89: an expired ephemeral is a tombstone that still
        // carries its expiry stamp — label it distinctly from a
        // delete-for-everyone; a live one shows its deadline.
        let mut suffix = if item.redacted {
            if item.expires_at_ms.is_some() {
                "  (expired)".to_string()
            } else {
                "  (deleted)".to_string()
            }
        } else if let Some(ts) = item.edited_at_ms {
            format!("  (edited at {ts})")
        } else {
            String::new()
        };
        if !item.redacted
            && let Some(at) = item.expires_at_ms
        {
            suffix.push_str(&format!("  ⏱ until {}", super::fmt_ms(at)));
        }
        if !item.reactions.is_empty() {
            let reactions_str: String = item
                .reactions
                .iter()
                .map(|g| format!("{}×{}", g.emoji, g.senders.len()))
                .collect::<Vec<_>>()
                .join(" ");
            suffix.push_str(&format!("  [{reactions_str}]"));
        }
        if !item.delivered_to.is_empty() {
            suffix.push_str(&format!("  ↓×{}", item.delivered_to.len()));
        }
        if !item.read_by.is_empty() {
            suffix.push_str(&format!("  ✓×{}", item.read_by.len()));
        }
        if !mentions.is_empty() {
            let mention_str: String = mentions
                .iter()
                .map(|m| format!("@{}", m.user_id))
                .collect::<Vec<_>>()
                .join(" ");
            suffix.push_str(&format!("  [mentions: {mention_str}]"));
        }
        if let Some(parent_id) = reply_to.as_deref() {
            println!("    [{when}] {marker} {sender} ({}){suffix}", item.item_id);
            println!("           ↳ in reply to {parent_id}");
            println!("             {body_preview}");
        } else {
            println!(
                "    [{when}] {marker} {sender} ({})  {body_preview}{suffix}",
                item.item_id
            );
        }
    }
    Ok(())
}

/// One-line rendering of an item's content — the body for text, a
/// `<kind …>` preview otherwise — plus the reply / mention decorations
/// text rows carry. Shared by `timeline` and `failed-sends` (Step 79).
pub(super) fn item_preview(
    item: &kupferdraht::timeline::TimelineItem,
) -> (String, Option<String>, Vec<kupferdraht::timeline::Mention>) {
    match &item.kind {
        TimelineItemKind::Text {
            body,
            reply_to,
            mentions,
        } => (body.clone(), reply_to.clone(), mentions.clone()),
        TimelineItemKind::Redacted => ("<redacted>".into(), None, Vec::new()),
        TimelineItemKind::Asset(a) => (
            format!(
                "<asset {} bytes mime={}{}>",
                a.size_bytes,
                a.mime_type,
                a.filename
                    .as_deref()
                    .map(|n| format!(" filename={n}"))
                    .unwrap_or_default(),
            ),
            None,
            Vec::new(),
        ),
        TimelineItemKind::Knock { hot_knock } => (
            format!("<knock{}>", if *hot_knock { " (hot)" } else { "" }),
            None,
            Vec::new(),
        ),
        TimelineItemKind::Location {
            latitude,
            longitude,
            name,
            zoom,
        } => (
            format!(
                "<location {latitude}, {longitude}{}{}>",
                name.as_deref()
                    .map(|n| format!(" \"{n}\""))
                    .unwrap_or_default(),
                zoom.map(|z| format!(", zoom={z}")).unwrap_or_default(),
            ),
            None,
            Vec::new(),
        ),
        TimelineItemKind::Task(task) => (
            format!("<task {}>", super::task::task_preview(task)),
            None,
            Vec::new(),
        ),
        TimelineItemKind::Case(case) => (
            format!(
                "<case {} rev {} {}>",
                case.case_ref,
                case.revision,
                case.title.as_deref().unwrap_or("\u{2014}")
            ),
            None,
            Vec::new(),
        ),
        TimelineItemKind::TaskListMarker { marked } => (
            format!(
                "<task-list marker: {}>",
                if *marked { "marked" } else { "cleared" }
            ),
            None,
            Vec::new(),
        ),
    }
}

pub(crate) async fn send(
    room_id: String,
    body: String,
    ephemeral_secs: Option<u64>,
) -> Result<(), KupferdrahtError> {
    let (client, room, room_id) = resume_with_room(room_id).await?;
    let timeline = room.timeline()?;
    // Step 89: a self-deleting text rides Wire's `Ephemeral` wrapper;
    // the wrapper carries no mentions, so handle parsing is skipped.
    if let Some(secs) = ephemeral_secs {
        let ttl = std::time::Duration::from_secs(secs);
        let handle = timeline.send_ephemeral_text(body, ttl).await?;
        println!("send ok");
        println!("  room_id:    {room_id}");
        println!("  message_id: {}", handle.message_id);
        println!("  sent_at:    {}", super::fmt_ms(handle.sent_at_ms));
        println!("  ephemeral:  {secs}s");
        let expires_at_ms = handle.sent_at_ms + i64::try_from(secs * 1_000).unwrap_or(i64::MAX);
        println!("  expires_at: {}", super::fmt_ms(expires_at_ms));
        return Ok(());
    }
    // Step 6-II-g: auto-parse `@<handle>` tokens in the body and
    // emit Wire `Mention` entries pointing at the resolved
    // qualified user-ids. Unknown handles are left as plain text.
    let mentions = client.resolve_mentions_in_body(&body).await?;
    let resolved_count = mentions.len();
    let handle = if mentions.is_empty() {
        timeline.send_text(body).await?
    } else {
        timeline.send_text_with_mentions(body, mentions).await?
    };
    println!("send ok");
    println!("  room_id:    {room_id}");
    println!("  message_id: {}", handle.message_id);
    println!("  sent_at:    {}", super::fmt_ms(handle.sent_at_ms));
    if resolved_count > 0 {
        println!("  mentions:   {resolved_count}");
    }
    Ok(())
}

/// Step 26 — send a `Knock` (nudge) to a room.
pub(crate) async fn knock(room_id: String, hot: bool) -> Result<(), KupferdrahtError> {
    let (_client, room, room_id) = resume_with_room(room_id).await?;
    let timeline = room.timeline()?;
    let handle = timeline.send_knock(hot).await?;
    println!("knock ok");
    println!("  room_id:    {room_id}");
    println!("  hot_knock:  {hot}");
    println!("  message_id: {}", handle.message_id);
    println!("  sent_at:    {}", super::fmt_ms(handle.sent_at_ms));
    Ok(())
}

/// Step 26 — share a `Location` (map point) with a room.
pub(crate) async fn location(
    room_id: String,
    lat: f32,
    lon: f32,
    name: Option<String>,
    zoom: Option<i32>,
) -> Result<(), KupferdrahtError> {
    let (_client, room, room_id) = resume_with_room(room_id).await?;
    let timeline = room.timeline()?;
    let handle = timeline.send_location(lat, lon, name.clone(), zoom).await?;
    println!("location ok");
    println!("  room_id:    {room_id}");
    println!("  lat/lon:    {lat}, {lon}");
    if let Some(name) = name {
        println!("  name:       {name}");
    }
    if let Some(zoom) = zoom {
        println!("  zoom:       {zoom}");
    }
    println!("  message_id: {}", handle.message_id);
    println!("  sent_at:    {}", super::fmt_ms(handle.sent_at_ms));
    Ok(())
}

/// Resolve the room id for a message-scoped CLI command. When the
/// caller passed `--id <room>` explicitly, use that. Otherwise look
/// up the message's owning room in the local `timeline_events`
/// table — covering the common case where the user just listed the
/// timeline and wants to act on a printed message id without
/// re-typing the room.
///
/// Errors out with a clear hint when the message id is not in the
/// local store (run `timeline --id <room>` first or pass `--id`),
/// or when it ambiguously matches multiple rooms (UUIDv4 collisions
/// don't happen in practice but the schema permits it).
pub(crate) async fn resolve_room_for_message(
    client: &kupferdraht::Client,
    explicit: Option<String>,
    message_id: &str,
) -> Result<String, KupferdrahtError> {
    if let Some(r) = explicit {
        return Ok(r);
    }
    let rooms = client.find_message_room(message_id).await?;
    match rooms.len() {
        0 => Err(KupferdrahtError::InvalidInput(format!(
            "message {message_id} not in local timeline_events; \
             run `kupferdraht-cli timeline --id <room>@<domain>` first \
             to populate it, or pass --id <room>@<domain> explicitly"
        ))),
        1 => Ok(rooms.into_iter().next().expect("len == 1")),
        n => {
            let list = rooms.join(", ");
            Err(KupferdrahtError::InvalidInput(format!(
                "message {message_id} ambiguous: matches {n} rooms ({list}); \
                 pass --id <room>@<domain> explicitly"
            )))
        }
    }
}

pub(crate) async fn mark_read(
    room_id: Option<String>,
    message_id: String,
) -> Result<(), KupferdrahtError> {
    let (_client, room, room_id) = resume_with_room_for_message(room_id, &message_id).await?;
    let timeline = room.timeline()?;
    match timeline.mark_as_read(message_id.clone()).await? {
        Some(handle) => {
            println!("mark-read ok");
            println!("  room_id:           {room_id}");
            println!("  target:            {message_id}");
            println!("  confirmation_event:{}", handle.message_id);
            println!("  confirmed_at:      {}", super::fmt_ms(handle.sent_at_ms));
        }
        None => {
            // Step 10f gate: WIRE_RECEIPT_MODE = 0 → no Confirmation
            // sent. Other Wire clients respect the same preference,
            // so this is intentional — not a failure.
            println!("mark-read skipped");
            println!("  room_id:           {room_id}");
            println!("  target:            {message_id}");
            println!("  reason:            WIRE_RECEIPT_MODE = Off");
        }
    }
    Ok(())
}

pub(crate) async fn react(
    room_id: Option<String>,
    message_id: String,
    emoji: String,
) -> Result<(), KupferdrahtError> {
    let (_client, room, room_id) = resume_with_room_for_message(room_id, &message_id).await?;
    let timeline = room.timeline()?;
    let handle = timeline
        .toggle_reaction(message_id.clone(), emoji.clone())
        .await?;
    println!("react ok");
    println!("  room_id:        {room_id}");
    println!("  target:         {message_id}");
    println!("  emoji_toggled:  {emoji}");
    println!("  reaction_event: {}", handle.message_id);
    println!("  applied_at:     {}", super::fmt_ms(handle.sent_at_ms));
    Ok(())
}

pub(crate) async fn delete_message(
    room_id: Option<String>,
    message_id: String,
) -> Result<(), KupferdrahtError> {
    let (_client, room, room_id) = resume_with_room_for_message(room_id, &message_id).await?;
    let timeline = room.timeline()?;
    let handle = timeline.redact(&message_id).await?;
    println!("delete-message ok");
    println!("  room_id:      {room_id}");
    println!("  target:       {message_id}");
    println!("  delete_event: {}", handle.message_id);
    println!("  redacted_at:  {}", super::fmt_ms(handle.sent_at_ms));
    Ok(())
}

pub(crate) async fn edit(
    room_id: Option<String>,
    message_id: String,
    new_body: String,
) -> Result<(), KupferdrahtError> {
    let (_client, room, room_id) = resume_with_room_for_message(room_id, &message_id).await?;
    let timeline = room.timeline()?;
    let handle = timeline.edit(message_id.clone(), new_body.clone()).await?;
    println!("edit ok");
    println!("  room_id:    {room_id}");
    println!("  replacing:  {message_id}");
    println!("  new_body:   {new_body}");
    println!("  edit_event: {}", handle.message_id);
    println!("  edited_at:  {}", super::fmt_ms(handle.sent_at_ms));
    Ok(())
}

pub(crate) async fn reply(
    room_id: Option<String>,
    parent_id: String,
    body: String,
) -> Result<(), KupferdrahtError> {
    let (_client, room, room_id) = resume_with_room_for_message(room_id, &parent_id).await?;
    let timeline = room.timeline()?;
    let handle = timeline.send_reply(parent_id.clone(), body).await?;
    println!("reply ok");
    println!("  room_id:    {room_id}");
    println!("  reply_to:   {parent_id}");
    println!("  message_id: {}", handle.message_id);
    println!("  sent_at:    {}", super::fmt_ms(handle.sent_at_ms));
    Ok(())
}
