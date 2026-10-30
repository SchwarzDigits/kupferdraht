// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![allow(clippy::doc_markdown)]
//! `SyncService` — receive-side event drain.
//!
//! Two transports share one dispatch pipeline (MLS decrypt +
//! `EventHandler` fan-out + cursor persist):
//!
//! * **Pull (default)** — `GET /v14/notifications`, paged behind the
//!   persisted `since` cursor (`service.rs::drain_pending`; Step 5,
//!   loop surface Step 14b).
//! * **Push (opt-in, Steps 62/73)** — a long-lived WebSocket
//!   (`push/`, wire layer in `wire/websocket.rs`), selected via
//!   `SyncSettings.transport = SyncTransport::WebSocket`; native
//!   only. The concrete socket is auto-negotiated per connection
//!   (ADR-0019, `Client::server_supports_push_events()`): the
//!   `/v14/events` ConsumableNotifications dialect (per-frame ack
//!   strictly after the cursor persist) when the server enables it,
//!   the legacy `/await` route otherwise — i.e. on every current
//!   wire-server — where the socket is a doorbell for the REST drain
//!   (ADR-0030: no acks, only the drain advances the cursor).
//!
//! The browser reaches neither of these: `Client::sync_stream` needs
//! `tokio::spawn`, so the binding layer runs its own `spawn_local`
//! loop over `sync_once` instead (`wasm/live_sync.rs`). Step 115 gave
//! that loop the same doorbell over a `web_sys::WebSocket`
//! (`wasm/push_socket.rs`) — the module below stays native-only
//! because its transport is `tokio-tungstenite`.

mod dispatch;
mod drain;
mod event_handler;
mod follow_ups;
pub(crate) mod outbox;
mod page;
// Step 118: the pace the browser's and the native push loop share.
pub(crate) mod pace;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod push;
mod removal_flush;
mod sender;
mod service;
mod settings;
mod window;

pub use event_handler::{EventHandler, Message, MessageBody, TypingEvent};
pub(crate) use service::SyncService;
pub use settings::{SyncSettings, SyncSummary, SyncTransport};
