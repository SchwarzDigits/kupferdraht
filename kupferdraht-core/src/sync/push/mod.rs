// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![allow(clippy::doc_markdown)]
//! The push loop behind `SyncTransport::WebSocket` — since Step 73
//! (ADR-0019) an **auto-negotiated** pair of dialects:
//!
//! * every connection cycle first asks
//!   `Client::server_supports_push_events()`;
//! * `true` → the `/v14/events` ConsumableNotifications loop below
//!   (Step 62b, ADR-0017 — acked frames, sync marker);
//! * `false` (every current wire-server, observations §68) or a
//!   failed feature read → the legacy `/await` cycle in [`legacy`]
//!   (Bearer connect, then the socket as a doorbell for the REST
//!   drain — ADR-0030; client keepalive pings).
//!
//! Everything below this paragraph describes the `/v14/events`
//! dialect. One connection cycle = fresh `sync_marker` →
//! conversations-metadata refresh when the room list is stale (Step 118)
//! → WebSocket upgrade → frame loop.
//! Per `event` frame the order is load-bearing and MUST NOT be
//! reordered:
//!
//! 1. dispatch through the shared [`SyncService`] pipeline (which
//!    persists the `last_event_id` cursor),
//! 2. ack (§11.3 — the ack *consumes* the notification server-side;
//!    an ack before the durable write could lose an MLS
//!    `Commit`/`Welcome` forever, backend-spec §11.1),
//! 3. callback tick.
//!
//! A crash between 1 and 2 leaves the frame unacked; the backend
//! redelivers it on the next connection (same notification `id`, new
//! `delivery_tag`) and the dedup below acks it without a second
//! dispatch.
//!
//! Orderly closes and socket-transport errors reconnect, at most once
//! per pause (`SyncSettings::idle_pause` — `poll_interval`, 10 s
//! when unset; Step 118): a long-lived connection reconnects at once, a
//! flapping one waits out the pace. A connect that fails drains anyway
//! — receive never depends on the socket opening. A `429` waits its
//! `Retry-After` (Step 103); hard errors (storage, metadata refresh,
//! missing device, bad `websocket_url`) abort with `Err` — the same
//! failure classes that abort the polling loop.

mod legacy;

use std::ops::ControlFlow;
use std::sync::Arc;

use tracing::{debug, warn};

use crate::client::Client;
use crate::error::KupferdrahtError;
use crate::storage::SyncStateRecord;
use crate::sync::service::{NotificationOutcome, SyncService};
use crate::sync::settings::{SyncSettings, SyncSummary};
use crate::time::now_ms;
use crate::wire::notification_dto::QueuedNotification;
use crate::wire::websocket::{ConsumableNotification, EventSocket, events_url};

use legacy::run_legacy_cycle;

/// How a connection cycle ended, when the consumer did not stop it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Ended {
    /// The socket did not open: the cycle drains without one (Step 118).
    NoSocket,
    /// The connection closed, failed or was renewed after it opened.
    Closed,
}

/// Drive the push loop until `on_frame` breaks (→ `Ok(())`) or a hard
/// error aborts it. See the module docs for the cycle anatomy.
///
/// # Errors
/// `KupferdrahtError::InvalidInput` when no device is registered or the
/// `websocket_url` is not `ws`/`wss`; `Storage` on cursor/marker
/// persistence failure; metadata-refresh failures pass through
/// unchanged. Socket-transport failures never surface here — they
/// reconnect.
pub(crate) async fn run_push<F>(
    client: &Arc<Client>,
    service: &SyncService,
    settings: &SyncSettings,
    mut on_frame: F,
) -> Result<(), KupferdrahtError>
where
    F: FnMut(SyncSummary) -> ControlFlow<()> + Send,
{
    // Step 95c: the device id is registration / persisted data — an
    // empty one is refused like a missing one, never asserted.
    let device_id = client
        .device_id()
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            KupferdrahtError::InvalidInput(
                "sync push: device_id missing — register a device first".into(),
            )
        })?;
    loop {
        let started = tokio::time::Instant::now();
        match run_cycle(client, service, &device_id, &mut on_frame).await {
            Ok(ControlFlow::Break(())) => return Ok(()),
            Ok(ControlFlow::Continue(())) => {}
            // Step 103: a `429` is waited out instead of aborting; the wait
            // replaces this round's pace.
            Err(KupferdrahtError::RateLimited { retry_after_secs }) => {
                let pause = settings.rate_limit_pause(retry_after_secs);
                warn!(
                    ?pause,
                    "push: rate-limited by the backend; waiting to reconnect"
                );
                tokio::time::sleep(pause).await;
                continue;
            }
            Err(e) => return Err(e),
        }
        // Step 118: at most one connect per pace — a long-lived connection
        // reconnects at once, one that closed at once waits out the rest.
        tokio::time::sleep(settings.idle_pause().saturating_sub(started.elapsed())).await;
    }
}

/// One cycle: a connection until it ends — or, when the socket does not
/// open, one drain without it (Step 118).
async fn run_cycle<F>(
    client: &Arc<Client>,
    service: &SyncService,
    device_id: &str,
    on_frame: &mut F,
) -> Result<ControlFlow<()>, KupferdrahtError>
where
    F: FnMut(SyncSummary) -> ControlFlow<()> + Send,
{
    match run_connection(client, service, device_id, on_frame).await? {
        ControlFlow::Break(()) => Ok(ControlFlow::Break(())),
        ControlFlow::Continue(Ended::Closed) => Ok(ControlFlow::Continue(())),
        ControlFlow::Continue(Ended::NoSocket) => drain_without_socket(service, on_frame).await,
    }
}

/// Drain once without a socket, as the polling loop would, and tick the
/// consumer (Step 118). A transport-class failure is reported and retried
/// at the next pace; hard errors bubble.
async fn drain_without_socket<F>(
    service: &SyncService,
    on_frame: &mut F,
) -> Result<ControlFlow<()>, KupferdrahtError>
where
    F: FnMut(SyncSummary) -> ControlFlow<()> + Send,
{
    match service.drain_pending().await {
        Ok(events_dispatched) => Ok(on_frame(SyncSummary {
            rooms_updated: 0,
            events_dispatched,
        })),
        Err(e @ (KupferdrahtError::Network(_) | KupferdrahtError::BackendRejected(_))) => {
            service.notify_error(&format!("push drain without a socket: {e}"));
            warn!(error = %e, "push: drain without a socket failed; retrying at the pace");
            Ok(ControlFlow::Continue(()))
        }
        Err(e) => Err(e),
    }
}

/// One `/v14/events` connection cycle (ADR-0017 dialect): connect
/// with the per-connection sync marker and drive until close/Break.
/// A failed connect is `Continue(NoSocket)` — the caller drains without it.
async fn run_events_cycle<F>(
    client: &Arc<Client>,
    service: &SyncService,
    device_id: &str,
    on_frame: &mut F,
) -> Result<ControlFlow<(), Ended>, KupferdrahtError>
where
    F: FnMut(SyncSummary) -> ControlFlow<()> + Send,
{
    // Fresh per-connection sentinel (§11.5); the backend echoes it
    // in the synchronization frame once the backlog has drained.
    let sync_marker = uuid::Uuid::new_v4().to_string();
    let access_token = client.access_token().unwrap_or_default();
    let url = events_url(
        client.websocket_url(),
        client.api_version(),
        device_id,
        &sync_marker,
        &access_token,
    )?;
    let mut socket = match EventSocket::connect(url).await {
        Ok(socket) => socket,
        Err(e) => {
            service.notify_error(&format!("push connect: {e}"));
            warn!(error = %e, "push: connect failed; draining without a socket");
            return Ok(ControlFlow::Continue(Ended::NoSocket));
        }
    };
    stamp_last_connected(client).await?;
    debug!(sync_marker = %sync_marker, "push: connected to /events");
    drive_connection(client, service, &mut socket, &sync_marker, on_frame).await
}

/// Read one connection until it ends. `Break` = the consumer stopped
/// the loop; `Continue(Closed)` = the connection dropped — reconnect.
/// Hard errors bubble.
async fn drive_connection<F>(
    client: &Arc<Client>,
    service: &SyncService,
    socket: &mut EventSocket,
    sync_marker: &str,
    on_frame: &mut F,
) -> Result<ControlFlow<(), Ended>, KupferdrahtError>
where
    F: FnMut(SyncSummary) -> ControlFlow<()> + Send,
{
    loop {
        match socket.next_frame().await {
            Ok(Some(ConsumableNotification::Event {
                delivery_tag,
                event,
            })) => {
                let Some(events_dispatched) = dispatch_deduped(client, service, &event).await?
                else {
                    // Step 97d (ADR-0031): the frame awaits an own pending
                    // commit — leave it unacked; the next connection gets
                    // it redelivered (§11.3), after the merge.
                    debug!(notif_id = %event.id, "push: frame deferred; reconnecting unacked");
                    return Ok(ControlFlow::Continue(Ended::Closed));
                };
                // Ack strictly after the dispatch above persisted the
                // cursor — see the module docs for why this order is
                // the one correctness obligation of the push path.
                if let Err(e) = socket.send_ack(delivery_tag, false).await {
                    service.notify_error(&format!("push ack: {e}"));
                    warn!(error = %e, "push: ack failed; reconnecting");
                    return Ok(ControlFlow::Continue(Ended::Closed));
                }
                let summary = SyncSummary {
                    rooms_updated: 0,
                    events_dispatched,
                };
                if on_frame(summary).is_break() {
                    return Ok(ControlFlow::Break(()));
                }
            }
            Ok(Some(ConsumableNotification::Synchronization {
                marker_id,
                delivery_tag,
            })) => {
                // Current backends queue marker events durably: they
                // carry a delivery_tag and MUST be acked, or the
                // broker's prefetch window stalls the stream. Stale
                // markers from earlier connections replay first —
                // ack those too, but only our own echo means
                // "caught up" and is worth persisting.
                if let Some(tag) = delivery_tag
                    && let Err(e) = socket.send_ack(tag, false).await
                {
                    service.notify_error(&format!("push marker ack: {e}"));
                    warn!(error = %e, "push: marker ack failed; reconnecting");
                    return Ok(ControlFlow::Continue(Ended::Closed));
                }
                if marker_id == sync_marker {
                    persist_sync_marker(client, &marker_id).await?;
                    debug!(marker = %marker_id, "push: caught up (synchronization frame)");
                } else {
                    debug!(
                        expected = %sync_marker,
                        received = %marker_id,
                        "push: acked stale synchronization marker from an earlier connection"
                    );
                }
            }
            Ok(Some(ConsumableNotification::Unknown { frame_type })) => {
                // §11.2 tolerance: log + discard, never tear down.
                warn!(frame_type = %frame_type, "push: unknown frame type; skipping");
            }
            Ok(None) => {
                debug!("push: connection closed by peer; reconnecting");
                return Ok(ControlFlow::Continue(Ended::Closed));
            }
            Err(e) => {
                service.notify_error(&format!("push read: {e}"));
                warn!(error = %e, "push: read failed; reconnecting");
                return Ok(ControlFlow::Continue(Ended::Closed));
            }
        }
    }
}

/// Dispatch one notification unless it is the §11.3 redelivery of the
/// frame we already processed (same `id` as the persisted cursor —
/// exactly the crash window between persist and ack). Returns the
/// dispatched-event count; `Some(0)` for the deduped case, which the
/// caller still acks so the redelivery stops; `None` when the frame
/// awaits an own pending commit (Step 97d) — the caller must not ack it.
/// The dispatch runs under the account's drain lock (ADR-0031).
async fn dispatch_deduped(
    client: &Arc<Client>,
    service: &SyncService,
    event: &QueuedNotification,
) -> Result<Option<u32>, KupferdrahtError> {
    debug_assert!(!event.id.is_empty(), "notification id must be non-empty");
    let outcome = {
        let _lock = client.lock_drain().await?;
        let last = load_sync_state(client).await?.and_then(|s| s.last_event_id);
        if last.as_deref() == Some(event.id.as_str()) {
            debug!(notif_id = %event.id, "push: redelivered notification already processed; ack-only");
            return Ok(Some(0));
        }
        service.begin_dispatch_cycle();
        service.dispatch_notification(client, event).await?
    };
    let count = match outcome {
        NotificationOutcome::Dispatched(count) => Some(count),
        NotificationOutcome::Deferred { room_id } => {
            // Wait for this process's flow (or clear an abandoned commit)
            // so the redelivery on the next connection goes through.
            let _ = service.resolve_deferral(client, &room_id).await;
            None
        }
    };
    // Step 74: on the push path one frame = one dispatch cycle —
    // flush queued external-removal commits before the ack releases
    // the frame (mid-backlog rejections are silent and retry on the
    // next frame's flush). Step 89: the ephemeral sweep rides the
    // same hook.
    service.end_dispatch_cycle(client).await;
    Ok(count)
}

/// One connection: the metadata refresh when the room list is stale
/// (Step 118 — at start; afterwards the dispatch cycles refresh it when an
/// event changed it, so a reconnect alone fetches nothing), then the
/// ADR-0019 socket negotiation and the chosen cycle. Its failures abort the push loop (the same class
/// that aborts the polling loop), a `429` excepted (see `run_push`).
async fn run_connection<F>(
    client: &Arc<Client>,
    service: &SyncService,
    device_id: &str,
    on_frame: &mut F,
) -> Result<ControlFlow<(), Ended>, KupferdrahtError>
where
    F: FnMut(SyncSummary) -> ControlFlow<()> + Send,
{
    debug_assert!(!device_id.is_empty(), "run_push refuses an empty device id");
    client.refresh_rooms_if_stale().await?;
    // A failed feature read falls back to the legacy route — every
    // current wire-server lives there, and the choice re-evaluates on
    // the next cycle.
    let use_events = match client.server_supports_push_events().await {
        Ok(supported) => supported,
        Err(e) => {
            warn!(error = %e, "push: feature read failed; assuming legacy /await");
            false
        }
    };
    if use_events {
        run_events_cycle(client, service, device_id, on_frame).await
    } else {
        run_legacy_cycle(client, service, device_id, on_frame).await
    }
}

async fn load_sync_state(
    client: &Arc<Client>,
) -> Result<Option<SyncStateRecord>, KupferdrahtError> {
    let Some(store) = client.store_handle() else {
        return Ok(None);
    };
    store.load_sync_state().await
}

/// Persist the backend's `synchronization` echo (§11.2) so the next
/// process start can tell how far this device had caught up.
async fn persist_sync_marker(
    client: &Arc<Client>,
    marker_id: &str,
) -> Result<(), KupferdrahtError> {
    debug_assert!(!marker_id.is_empty(), "marker echo must be non-empty");
    let Some(store) = client.store_handle() else {
        return Ok(());
    };
    let mut state = store.load_sync_state().await?.unwrap_or_default();
    state.sync_marker_uuid = Some(marker_id.to_string());
    state.last_connected_at_ms = Some(now_ms());
    store.save_sync_state(&state).await
}

/// Stamp the successful upgrade — `sync_state.last_connected_at_ms`
/// feeds the backend-spec §11.6 inactivity accounting client-side.
async fn stamp_last_connected(client: &Arc<Client>) -> Result<(), KupferdrahtError> {
    let Some(store) = client.store_handle() else {
        return Ok(());
    };
    let mut state = store.load_sync_state().await?.unwrap_or_default();
    state.last_connected_at_ms = Some(now_ms());
    store.save_sync_state(&state).await
}
