// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `kupferdraht-cli` timeline command handlers (send / reply / edit / react / …).

use kupferdraht::timeline::{DeliveryStatus, MessagesOptions, TimelineItemKind};
use kupferdraht::{KupferdrahtError, OutgoingContent};

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
        TimelineItemKind::MembershipChange(change) => (membership_line(change), None, Vec::new()),
        TimelineItemKind::RoomNameChange { name } => (
            format!("* {} renamed the room to \"{name}\"", item.sender_user_id),
            None,
            Vec::new(),
        ),
        TimelineItemKind::RoomCreated { name } => (
            format!(
                "* {} created the room{}",
                item.sender_user_id,
                name.as_deref()
                    .map(|n| format!(" \"{n}\""))
                    .unwrap_or_default(),
            ),
            None,
            Vec::new(),
        ),
    }
}

/// The system line of a membership item (Step 145).
fn membership_line(change: &kupferdraht::timeline::RoomMembershipChange) -> String {
    use kupferdraht::timeline::MembershipChange as Change;
    let users = change.users.join(", ");
    let actor = &change.actor;
    match change.change {
        Change::Joined => format!("* {users} joined"),
        Change::Added => format!("* {actor} added {users}"),
        Change::Left => format!("* {users} left"),
        Change::Kicked => format!("* {actor} removed {users}"),
    }
}

pub(crate) async fn send(
    room_id: String,
    body: String,
    ephemeral_secs: Option<u64>,
    idempotency_key: Option<String>,
) -> Result<(), KupferdrahtError> {
    let (client, room, room_id) = resume_with_room(room_id).await?;
    let timeline = room.timeline()?;
    // Step 89: a self-deleting text rides Wire's `Ephemeral` wrapper;
    // the wrapper carries no mentions, so handle parsing is skipped.
    if let Some(secs) = ephemeral_secs {
        let ttl = std::time::Duration::from_secs(secs);
        let handle = timeline.send_ephemeral_text(body, ttl).await?;
        print_send_handle(&room_id, &handle);
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
    let handle = match idempotency_key.clone() {
        // Step 130a: a keyed send answers a repeat of its key from the store.
        Some(key) => {
            let content = OutgoingContent::Text { body, mentions };
            timeline.send_keyed(key, content).await?
        }
        None if mentions.is_empty() => timeline.send_text(body).await?,
        None => timeline.send_text_with_mentions(body, mentions).await?,
    };
    print_send_handle(&room_id, &handle);
    if resolved_count > 0 {
        println!("  mentions:   {resolved_count}");
    }
    if let Some(key) = idempotency_key {
        println!("  idempotency_key: {key}");
    }
    Ok(())
}

/// The `send ok` block every `send` variant prints first.
fn print_send_handle(room_id: &str, handle: &kupferdraht::MessageHandle) {
    println!("send ok");
    println!("  room_id:    {room_id}");
    println!("  message_id: {}", handle.message_id);
    println!("  sent_at:    {}", super::fmt_ms(handle.sent_at_ms));
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
    no_receipt: bool,
) -> Result<(), KupferdrahtError> {
    let (_client, room, room_id) = resume_with_room_for_message(room_id, &message_id).await?;
    let timeline = room.timeline()?;
    let receipt = if no_receipt {
        kupferdraht::ReceiptType::ReadPrivate
    } else {
        kupferdraht::ReceiptType::Read
    };
    let outcome = timeline.mark_as_read(message_id.clone(), receipt).await?;
    let marker = room
        .read_marker()
        .await?
        .map_or_else(|| "none".to_string(), super::fmt_ms);
    match outcome {
        Some(handle) => {
            println!("mark-read ok");
            println!("  room_id:           {room_id}");
            println!("  target:            {message_id}");
            println!("  confirmation_event:{}", handle.message_id);
            println!("  confirmed_at:      {}", super::fmt_ms(handle.sent_at_ms));
        }
        None if no_receipt => {
            // Step 138: the position moved, the room learns nothing.
            println!("mark-read private");
            println!("  room_id:           {room_id}");
            println!("  target:            {message_id}");
        }
        None => {
            // Step 10f gate: WIRE_RECEIPT_MODE = 0 → no Confirmation
            // sent. Other Wire clients respect the same preference,
            // so this is intentional — not a failure. Step 145: neither
            // is a room event's item, which names no message.
            let items = timeline.items().await?;
            let target = items.iter().find(|item| item.item_id == message_id);
            println!("mark-read skipped");
            println!("  room_id:           {room_id}");
            println!("  target:            {message_id}");
            println!(
                "  reason:            {}",
                skipped_reason(target.map(|item| &item.kind))
            );
        }
    }
    println!("  read_marker:       {marker}");
    Ok(())
}

/// Why `mark_as_read` told the room nothing, a private read aside.
fn skipped_reason(target: Option<&TimelineItemKind>) -> &'static str {
    match target {
        Some(
            TimelineItemKind::MembershipChange(_)
            | TimelineItemKind::RoomNameChange { .. }
            | TimelineItemKind::RoomCreated { .. },
        ) => "a room event, not a message",
        _ => "WIRE_RECEIPT_MODE = Off",
    }
}

/// Step 138: flag a room unread by hand; the room must be known locally.
pub(crate) async fn mark_unread(room_id: String) -> Result<(), KupferdrahtError> {
    let client = super::resume_client().await?;
    let room = client.room_or_load(&room_id).await.ok_or_else(|| {
        KupferdrahtError::InvalidInput(format!(
            "mark-unread: unknown room {room_id}; run `kupferdraht-cli sync` first"
        ))
    })?;
    room.set_unread_flag(true).await?;
    println!("mark-unread ok");
    println!("  room_id:           {room_id}");
    println!("  unread:            {}", room.unread_count().await?);
    println!("  marked_unread:     {}", room.is_marked_unread().await?);
    Ok(())
}

pub(crate) async fn react(
    room_id: Option<String>,
    message_id: String,
    emoji: String,
    idempotency_key: Option<String>,
) -> Result<(), KupferdrahtError> {
    let (_client, room, room_id) = resume_with_room_for_message(room_id, &message_id).await?;
    let timeline = room.timeline()?;
    let handle = match idempotency_key.clone() {
        // Step 130a: a repeat of the key answers the first toggle's
        // handle and never flips the reaction back.
        Some(key) => {
            let content = OutgoingContent::Reaction {
                item_id: message_id.clone(),
                emoji: emoji.clone(),
            };
            timeline.send_keyed(key, content).await?
        }
        None => {
            timeline
                .toggle_reaction(message_id.clone(), emoji.clone())
                .await?
        }
    };
    println!("react ok");
    println!("  room_id:        {room_id}");
    println!("  target:         {message_id}");
    println!("  emoji_toggled:  {emoji}");
    println!("  reaction_event: {}", handle.message_id);
    println!("  applied_at:     {}", super::fmt_ms(handle.sent_at_ms));
    if let Some(key) = idempotency_key {
        println!("  idempotency_key: {key}");
    }
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

#[cfg(test)]
mod tests {
    use kupferdraht::timeline::{
        DeliveryStatus, MembershipChange, RoomMembershipChange, TimelineItem, TimelineItemKind,
    };

    use super::{item_preview, skipped_reason};

    fn preview(kind: TimelineItemKind) -> String {
        item_preview(&TimelineItem {
            item_id: "i".into(),
            room_id: "r@d".into(),
            sender_user_id: "alice@d".into(),
            sender_device_id: String::new(),
            sent_at_ms: 0,
            kind,
            edited_at_ms: None,
            redacted: false,
            reactions: Vec::new(),
            read_by: Vec::new(),
            delivered_to: Vec::new(),
            delivery_status: DeliveryStatus::Received,
            expires_at_ms: None,
        })
        .0
    }

    fn change(change: MembershipChange, users: &[&str]) -> TimelineItemKind {
        TimelineItemKind::MembershipChange(RoomMembershipChange {
            change,
            actor: "alice@d".into(),
            users: users.iter().map(ToString::to_string).collect(),
        })
    }

    #[test]
    fn a_skipped_read_names_its_real_reason() {
        let room_event = TimelineItemKind::RoomNameChange { name: "New".into() };
        let message = TimelineItemKind::Knock { hot_knock: false };

        assert_eq!(
            skipped_reason(Some(&room_event)),
            "a room event, not a message"
        );
        assert_eq!(skipped_reason(Some(&message)), "WIRE_RECEIPT_MODE = Off");
        assert_eq!(skipped_reason(None), "WIRE_RECEIPT_MODE = Off");
    }

    #[test]
    fn a_room_event_prints_a_system_line() {
        let lines = [
            (
                change(MembershipChange::Added, &["bob@d", "carol@d"]),
                "* alice@d added bob@d, carol@d",
            ),
            (
                change(MembershipChange::Joined, &["alice@d"]),
                "* alice@d joined",
            ),
            (
                change(MembershipChange::Left, &["alice@d"]),
                "* alice@d left",
            ),
            (
                change(MembershipChange::Kicked, &["bob@d"]),
                "* alice@d removed bob@d",
            ),
            (
                TimelineItemKind::RoomNameChange { name: "New".into() },
                "* alice@d renamed the room to \"New\"",
            ),
            (
                TimelineItemKind::RoomCreated {
                    name: Some("Work".into()),
                },
                "* alice@d created the room \"Work\"",
            ),
            (
                TimelineItemKind::RoomCreated { name: None },
                "* alice@d created the room",
            ),
        ];
        for (kind, line) in lines {
            assert_eq!(preview(kind), line);
        }
    }
}
