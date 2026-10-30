// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `kupferdraht-cli` sync command handlers (sync / sync-metadata / sync-stream).

use std::sync::Arc;

use kupferdraht::storage::SessionRecord;
use kupferdraht::{EventHandler, KupferdrahtError, Message, MessageBody, now_ms};

use super::{resume_client, resume_client_and_store};

pub(crate) async fn sync() -> Result<(), KupferdrahtError> {
    let (client, store) = resume_client_and_store().await?;
    // The resume may have refreshed the access token; persist the current
    // tokens so a follow-up `status` shows them — load-then-mutate, so the
    // row keeps what the login recorded (`cookie_label`, Step 93).
    let user_id = client
        .user_id()
        .map_or_else(String::new, |u| u.as_str().to_string());
    let mut session: SessionRecord = store
        .load_session(&user_id)
        .await?
        .ok_or_else(|| KupferdrahtError::Storage("sync: no persisted session to update".into()))?;
    session.device_id = client.device_id().unwrap_or_default().to_string();
    session.access_token = client.access_token().unwrap_or_default();
    session.refresh_token = client.refresh_token().unwrap_or_default();
    session.access_token_issued_at_ms = now_ms();
    store.save_session(&session).await?;

    client.add_event_handler(Arc::new(CliEventHandler));
    let _devices = client.subscribe_device_changes(Arc::new(CliDevicesListener));
    let summary = client
        .sync_once(kupferdraht::SyncSettings::default())
        .await?;
    println!("sync ok");
    println!("  rooms synced:           {}", summary.rooms_updated);
    println!("  notifications drained:  {}", summary.events_dispatched);
    Ok(())
}

pub(crate) async fn sync_metadata() -> Result<(), KupferdrahtError> {
    let client = resume_client().await?;
    client.sync_metadata().await?;
    println!("sync-metadata ok");
    println!("  rooms cached: {}", client.rooms().await.len());
    Ok(())
}

/// Bare-bones [`EventHandler`] for `kupferdraht-cli sync-stream` — prints
/// inbound text messages to stdout in a human-readable form. Wired
/// in via `Client::add_event_handler` before the `SyncService` starts.
struct CliEventHandler;

impl EventHandler for CliEventHandler {
    fn on_message(&self, message: Message) {
        let body: String = match &message.body {
            MessageBody::Text(b) => b.clone(),
            MessageBody::Asset(a) => format!(
                "<asset {} ({} bytes, mime={}){}>",
                a.message_id,
                a.size_bytes,
                a.mime_type,
                a.filename
                    .as_deref()
                    .map(|n| format!(", filename={n}"))
                    .unwrap_or_default(),
            ),
            MessageBody::Knock { hot_knock } => {
                format!("<knock{}>", if *hot_knock { " (hot)" } else { "" })
            }
            MessageBody::Location {
                latitude,
                longitude,
                name,
                zoom,
            } => format!(
                "<location {latitude}, {longitude}{}{}>",
                name.as_deref()
                    .map(|n| format!(" \"{n}\""))
                    .unwrap_or_default(),
                zoom.map(|z| format!(", zoom={z}")).unwrap_or_default(),
            ),
            MessageBody::Task(task) => {
                format!("<task {}>", super::task::task_preview(task))
            }
            MessageBody::Case {
                snapshot,
                sender_authorized,
            } => format!(
                "<case {} rev {}{}>",
                snapshot.case_ref,
                snapshot.revision,
                if *sender_authorized {
                    ""
                } else {
                    " (unauthorized)"
                }
            ),
            MessageBody::TaskListMarker { marked } => format!(
                "<task-list marker: {}>",
                if *marked { "marked" } else { "cleared" }
            ),
        };
        let device = message
            .sender_device_id
            .as_deref()
            .map(|d| format!(" / device {d}"))
            .unwrap_or_default();
        println!(
            "[recv] {} → {} (msg {}){}: {body}",
            message.sender_user_id, message.room_id, message.message_id, device,
        );
    }
    fn on_room_invited(&self, room_id: String) {
        println!("[invite] {room_id}");
    }
    fn on_sync_error(&self, description: String) {
        eprintln!("[sync error] {description}");
    }
    // Typing is transient (socket-only, backend-spec §11.1) — this
    // line is the visible proof that the Step-62 push transport
    // delivers what polling never could.
    fn on_typing(&self, event: kupferdraht::sync::TypingEvent) {
        if event.typing_users.is_empty() {
            println!("[typing] {} — nobody typing", event.room_id);
        } else {
            println!(
                "[typing] {}: {}",
                event.room_id,
                event.typing_users.join(", ")
            );
        }
    }
}

/// Prints a room's device-set change (Step 95d) — the signal for a device
/// nobody expected; compare its thumbprint with the Wire app.
struct CliDevicesListener;

impl kupferdraht::RoomDevicesListener for CliDevicesListener {
    fn on_devices_changed(&self, change: kupferdraht::RoomDevicesChange) {
        println!("devices changed in {}", change.room_id);
        for (sign, devices) in [("+", &change.added), ("-", &change.removed)] {
            for device in devices {
                println!(
                    "  {sign} {} / {}  mls thumbprint: {}",
                    device.user_id.as_deref().unwrap_or("(identity not parsed)"),
                    device.device_id.as_deref().unwrap_or("—"),
                    device
                        .thumbprint
                        .as_deref()
                        .map_or_else(|| "—".to_owned(), super::rooms::grouped_thumbprint)
                );
            }
        }
    }
}

pub(crate) async fn sync_stream(
    idle_backoff_secs: Option<u64>,
    transport: String,
) -> Result<(), KupferdrahtError> {
    use std::time::Duration;
    let transport = match transport.as_str() {
        "polling" => kupferdraht::SyncTransport::Polling,
        "websocket" => kupferdraht::SyncTransport::WebSocket,
        other => {
            return Err(KupferdrahtError::InvalidInput(format!(
                "--transport must be `polling` or `websocket`, got `{other}`"
            )));
        }
    };
    let client = resume_client().await?;
    client.add_event_handler(Arc::new(CliEventHandler));
    let _devices = client.subscribe_device_changes(Arc::new(CliDevicesListener));
    let settings = kupferdraht::SyncSettings {
        poll_interval: idle_backoff_secs.map(Duration::from_secs),
        transport,
    };
    let pace = idle_backoff_secs.map_or_else(|| "default".to_owned(), |s| format!("{s}s"));
    match transport {
        kupferdraht::SyncTransport::Polling => {
            println!("sync-stream — polling /v14/notifications continuously");
            println!("  idle backoff:  {pace}");
        }
        kupferdraht::SyncTransport::WebSocket => {
            // Step 73 (ADR-0019): the concrete socket is negotiated
            // per connection — surface the choice so operators see
            // which dialect a session runs.
            let negotiated = if client.server_supports_push_events().await? {
                "/v14/events (ConsumableNotifications)"
            } else {
                "legacy /await (consumableNotifications disabled; frames trigger a REST drain)"
            };
            println!("sync-stream — live WebSocket (push)");
            println!("  negotiated socket:      {negotiated}");
            println!("  push pace:              {pace}");
        }
    }
    println!("  press Ctrl-C to stop");
    tokio::select! {
        result = client.sync(settings) => {
            // Client::sync returns only on hard error; success-path is Ctrl-C below.
            result?;
            println!("sync-stream loop ended");
            Ok(())
        }
        _ = tokio::signal::ctrl_c() => {
            println!("\nsync-stream stopped");
            Ok(())
        }
    }
}
