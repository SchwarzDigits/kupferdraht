// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! The legacy `/await` connection cycle (Step 73, ADR-0019; Step 97b,
//! ADR-0030) — the socket every current wire-server actually pushes
//! through while ConsumableNotifications is upstream-disabled
//! (observations §68).
//!
//! The route is *passive*: no acks, no sync marker, no redelivery — a
//! push it does not deliver is gone. So the socket is only a
//! **doorbell** for the REST queue (ADR-0030):
//!
//! 1. **Connect, then drain** — the socket is registered first, one
//!    `since=<cursor>` drain follows: everything stored before the
//!    registration comes from that drain, everything after it rings.
//! 2. **A durable frame rings** — it triggers a drain from the cursor;
//!    frames during a running drain coalesce into one follow-up drain.
//!    The frame's own payload is never dispatched.
//! 3. **Only the drain advances the cursor** — never a frame.
//! 4. **Transient frames** (typing) dispatch directly without touching
//!    the cursor; read during a drain, they wait until it finished.
//!
//! The socket keeps being read while a drain runs, and a running drain
//! is never cancelled. The client sends periodic WebSocket pings —
//! cannon's idle timeout closes a silent connection otherwise (proven
//! in the Step-73 smoke's 100 s idle window). A ping finds a dead
//! connection only when its write fails, so a socket without a frame
//! for a minute is renewed when no drain runs (Step 118).

use std::collections::VecDeque;
use std::future::Future;
use std::ops::ControlFlow;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use futures_util::FutureExt;
use futures_util::future::{Fuse, FusedFuture};
use tracing::{debug, warn};

use crate::client::Client;
use crate::error::KupferdrahtError;
use crate::sync::service::SyncService;
use crate::sync::settings::SyncSummary;
use crate::wire::notification_dto::QueuedNotification;
use crate::wire::websocket::{EventSocket, await_url};

use super::{Ended, stamp_last_connected};
use crate::sync::pace::socket_silent;

/// Keepalive cadence on the legacy socket — the client sends no
/// application traffic, so periodic pings keep cannon's idle timeout
/// from closing a healthy connection (verified live in the step
/// smoke's idle window).
const LEGACY_PING_INTERVAL: Duration = Duration::from_secs(25);

/// Transient frames held back while a drain runs. They are live-only
/// (typing); beyond this many the oldest are dropped, so a flooding
/// socket cannot grow memory while a slow drain runs.
const MAX_HELD_TRANSIENT_FRAMES: usize = 64;

/// A connection's next move after one read, frame or drain:
/// `None` keeps reading, `Some(outcome)` ends the connection with the
/// cycle's outcome (`Break` = consumer stop, `Continue(_)` = reconnect).
type Next = Option<ControlFlow<(), Ended>>;

/// The outcome that ends a connection for a reconnect.
const RECONNECT: ControlFlow<(), Ended> = ControlFlow::Continue(Ended::Closed);

/// Per-connection doorbell state (ADR-0030).
struct Doorbell {
    /// A drain is owed: set on connect and by every durable frame.
    rung: bool,
    /// Transient frames read while a drain ran — dispatched after it.
    held: VecDeque<QueuedNotification>,
    /// When the socket last delivered a frame (or connected) — a socket
    /// silent for a minute is renewed (Step 118).
    last_frame: tokio::time::Instant,
}

/// One legacy `/await` connection cycle: Bearer-header connect, then
/// the doorbell loop (connect drain, frames, keepalive pings). A failed
/// connect is `Continue(NoSocket)` — the caller drains without it.
pub(super) async fn run_legacy_cycle<F>(
    client: &Arc<Client>,
    service: &SyncService,
    device_id: &str,
    on_frame: &mut F,
) -> Result<ControlFlow<(), Ended>, KupferdrahtError>
where
    F: FnMut(SyncSummary) -> ControlFlow<()> + Send,
{
    debug_assert!(!device_id.is_empty(), "run_push refuses an empty device id");
    let url = await_url(client.websocket_url(), device_id)?;
    let access_token = client.access_token().unwrap_or_default();
    let mut socket = match EventSocket::connect_with_bearer(url, &access_token).await {
        Ok(socket) => socket,
        Err(e) => {
            service.notify_error(&format!("push connect (legacy): {e}"));
            warn!(error = %e, "push: legacy connect failed; draining without a socket");
            return Ok(ControlFlow::Continue(Ended::NoSocket));
        }
    };
    stamp_last_connected(client).await?;
    debug!("push: connected to legacy /await (ADR-0030 doorbell)");
    drive_legacy_connection(client, service, &mut socket, on_frame).await
}

/// Read one legacy connection until it ends, running a REST drain
/// whenever the doorbell rang and none is running. Hard errors bubble.
async fn drive_legacy_connection<F>(
    client: &Arc<Client>,
    service: &SyncService,
    socket: &mut EventSocket,
    on_frame: &mut F,
) -> Result<ControlFlow<(), Ended>, KupferdrahtError>
where
    F: FnMut(SyncSummary) -> ControlFlow<()> + Send,
{
    let mut bell = Doorbell::connected();
    let mut ping = tokio::time::interval(LEGACY_PING_INTERVAL);
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ping.tick().await; // the first tick fires immediately — consume it
    // Boxed: the drain future is large, the connection loop lives long.
    let mut drain = Box::pin(Fuse::terminated());
    loop {
        if bell.rung && drain.is_terminated() {
            bell.rung = false;
            drain.set(service.drain_pending().fuse());
            debug_assert!(!drain.is_terminated(), "the rung drain is running");
        }
        // `biased`: a finished drain settles before the next frame is
        // read. Frame safety rests on `next_legacy_frame`'s
        // cancel-safety contract (see its doc), not on this order; a
        // terminated `Fuse` stays pending, so no drain arm fires then.
        let next = tokio::select! {
            biased;
            drained = &mut drain => {
                settle_drain(client, service, drained, &mut bell, on_frame).await?
            }
            frame = socket.next_legacy_frame() => {
                settle_read(client, service, frame, drain.as_mut(), &mut bell, on_frame).await?
            }
            _ = ping.tick() => send_keepalive(service, socket, &bell, drain.is_terminated()).await,
        };
        if let Some(outcome) = next {
            return Ok(outcome);
        }
    }
}

impl Doorbell {
    /// A fresh connection owes its connect drain (ADR-0030 d.1).
    fn connected() -> Self {
        Self {
            rung: true,
            held: VecDeque::new(),
            last_frame: tokio::time::Instant::now(),
        }
    }

    /// Hold a transient frame until the running drain finished,
    /// dropping the oldest one beyond the cap.
    fn hold(&mut self, notif: QueuedNotification) {
        if self.held.len() >= MAX_HELD_TRANSIENT_FRAMES {
            debug!("push: too many transient frames during a drain; dropping the oldest");
            self.held.pop_front();
        }
        self.held.push_back(notif);
        debug_assert!(self.held.len() <= MAX_HELD_TRANSIENT_FRAMES);
    }
}

/// Settle one socket read; when the socket ended, a running drain
/// finishes first ([`finish_drain`]).
async fn settle_read<D, F>(
    client: &Arc<Client>,
    service: &SyncService,
    frame: Result<Option<QueuedNotification>, KupferdrahtError>,
    drain: Pin<&mut Fuse<D>>,
    bell: &mut Doorbell,
    on_frame: &mut F,
) -> Result<Next, KupferdrahtError>
where
    D: Future<Output = Result<u32, KupferdrahtError>>,
    F: FnMut(SyncSummary) -> ControlFlow<()> + Send,
{
    let draining = !drain.is_terminated();
    match read_frame(client, service, frame, bell, draining, on_frame).await? {
        Some(ControlFlow::Continue(_)) => {
            finish_drain(client, service, drain, bell, on_frame).await
        }
        other => Ok(other),
    }
}

/// Handle one socket read: a durable frame rings, a transient one
/// dispatches (or waits for the running drain); a closed or failed
/// socket ends the connection (the caller finishes a running drain).
async fn read_frame<F>(
    client: &Arc<Client>,
    service: &SyncService,
    frame: Result<Option<QueuedNotification>, KupferdrahtError>,
    bell: &mut Doorbell,
    draining: bool,
    on_frame: &mut F,
) -> Result<Next, KupferdrahtError>
where
    F: FnMut(SyncSummary) -> ControlFlow<()> + Send,
{
    let notif = match frame {
        Ok(Some(notif)) => notif,
        Ok(None) => {
            debug!("push: legacy connection closed by peer; reconnecting");
            return Ok(Some(RECONNECT));
        }
        Err(e) => {
            service.notify_error(&format!("push read (legacy): {e}"));
            warn!(error = %e, "push: legacy read failed; reconnecting");
            return Ok(Some(RECONNECT));
        }
    };
    debug_assert!(!notif.id.is_empty(), "parse_legacy_frame refuses empty ids");
    bell.last_frame = tokio::time::Instant::now();
    if !notif.transient {
        debug!(notif_id = %notif.id, "push: durable legacy frame; draining");
        bell.rung = true;
        return Ok(None);
    }
    if draining {
        bell.hold(notif);
        return Ok(None);
    }
    let events_dispatched = dispatch_transient(client, service, &notif).await?;
    Ok(tick(on_frame, events_dispatched))
}

/// Settle a finished drain: dispatch the transient frames held during
/// it, then tick the consumer. A transport-class drain failure
/// reconnects (the next connect drains again); hard errors bubble.
async fn settle_drain<F>(
    client: &Arc<Client>,
    service: &SyncService,
    drained: Result<u32, KupferdrahtError>,
    bell: &mut Doorbell,
    on_frame: &mut F,
) -> Result<Next, KupferdrahtError>
where
    F: FnMut(SyncSummary) -> ControlFlow<()> + Send,
{
    let mut events_dispatched = match drained {
        Ok(n) => n,
        Err(e @ (KupferdrahtError::Network(_) | KupferdrahtError::BackendRejected(_))) => {
            service.notify_error(&format!("push drain (legacy): {e}"));
            warn!(error = %e, "push: legacy drain failed; reconnecting");
            return Ok(Some(RECONNECT));
        }
        Err(e) => return Err(e),
    };
    while let Some(notif) = bell.held.pop_front() {
        let n = dispatch_transient(client, service, &notif).await?;
        events_dispatched = events_dispatched.saturating_add(n);
    }
    debug_assert!(bell.held.is_empty(), "every held frame was dispatched");
    Ok(tick(on_frame, events_dispatched))
}

/// The socket ended: let a running drain finish (never cancelled
/// mid-way, ADR-0030), settle it, then reconnect — unless the settled
/// drain stopped the consumer.
async fn finish_drain<D, F>(
    client: &Arc<Client>,
    service: &SyncService,
    mut drain: Pin<&mut Fuse<D>>,
    bell: &mut Doorbell,
    on_frame: &mut F,
) -> Result<Next, KupferdrahtError>
where
    D: Future<Output = Result<u32, KupferdrahtError>>,
    F: FnMut(SyncSummary) -> ControlFlow<()> + Send,
{
    if !drain.is_terminated() {
        let drained = drain.as_mut().await;
        debug_assert!(drain.is_terminated(), "a finished drain is terminated");
        if let Some(ControlFlow::Break(())) =
            settle_drain(client, service, drained, bell, on_frame).await?
        {
            return Ok(Some(ControlFlow::Break(())));
        }
    }
    Ok(Some(RECONNECT))
}

/// Send one keepalive ping; a failed ping reconnects. Without a frame for
/// a minute, and no drain running, renew the socket instead (Step 118): a
/// ping only fails on a dead write, a socket whose frames are held back
/// answers it.
async fn send_keepalive(
    service: &SyncService,
    socket: &mut EventSocket,
    bell: &Doorbell,
    idle: bool,
) -> Next {
    if idle && socket_silent(bell.last_frame.elapsed()) {
        debug!("push: no frame for a minute; renewing the legacy socket");
        return Some(RECONNECT);
    }
    if let Err(e) = socket.send_ping().await {
        service.notify_error(&format!("push ping (legacy): {e}"));
        warn!(error = %e, "push: keepalive ping failed; reconnecting");
        return Some(RECONNECT);
    }
    None
}

/// Tick the consumer after one settled drain or transient frame —
/// one iteration, like a polling round; `Break` stops the cycle.
fn tick<F>(on_frame: &mut F, events_dispatched: u32) -> Next
where
    F: FnMut(SyncSummary) -> ControlFlow<()> + Send,
{
    let summary = SyncSummary {
        rooms_updated: 0,
        events_dispatched,
    };
    on_frame(summary)
        .is_break()
        .then_some(ControlFlow::Break(()))
}

/// Dispatch one transient frame as its own dispatch cycle, without a
/// cursor advance (ADR-0030 d.4 — transient events are never stored,
/// no drain can return them).
async fn dispatch_transient(
    client: &Arc<Client>,
    service: &SyncService,
    notif: &QueuedNotification,
) -> Result<u32, KupferdrahtError> {
    // Step 115b moved the body to `SyncService` so the browser socket shares it.
    service.dispatch_transient_notification(client, notif).await
}
