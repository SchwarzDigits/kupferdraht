// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! W-push — the browser's doorbell socket (Step 115).
//!
//! `wasm/live_sync.rs` drives `sync_once` and then sleeps `IDLE_BACKOFF`
//! unconditionally, so a message waits up to 2.5 s. This module gives that
//! sleep something to be interrupted by: a `web_sys::WebSocket` on Wire's
//! legacy `/await` route whose every frame is a **doorbell** (ADR-0030) — it
//! triggers the drain the loop would have run anyway, and never dispatches its
//! own payload.
//!
//! Deliberately not `tokio-tungstenite`: that dependency must stay out of the
//! wasm graph (`wire/websocket.rs`). And deliberately not a WebSocket ping for
//! keepalive — JS cannot send ping frames, so the keepalive is an application
//! text frame, which is what cannon needs to not close a healthy connection.
//!
//! `pub` rather than `pub(crate)` so the browser suite in `tests/` can drive
//! the bell directly: this crate's `#[cfg(test)]` unit tests do not build for
//! wasm at all (other modules' test blocks pull in native-only dev
//! dependencies), so a wasm test has to be an integration test. Plain `pub` is
//! Rust-visible only — nothing here is `#[wasm_bindgen]`, so the JS surface is
//! unchanged.

use tracing::debug;

use crate::error::KupferdrahtError;
use wasm_bindgen::prelude::*;

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};
use std::time::Duration;

/// Transient frames held for the loop. Live-only (typing), so beyond this many
/// the oldest are dropped: a flooding socket must not grow memory while a slow
/// drain runs. Same cap as the native loop.
const MAX_HELD_TRANSIENT_FRAMES: usize = 64;

/// Why the loop stopped waiting.
#[derive(Debug, PartialEq, Eq)]
pub enum Wake {
    /// A durable frame arrived: drain now.
    Rang,
    /// Only transient frames arrived. Dispatch them, but do **not** drain —
    /// nothing was stored for a drain to find (ADR-0030 d.4).
    Transient,
    /// Nothing arrived within the backoff: drain on the timer, as before.
    TimedOut,
}

/// One parsed `/await` frame, as much of it as the browser needs.
///
/// A newtype so the wire DTO stays crate-private while the browser suite can
/// still drive the queue.
pub struct PushFrame(crate::wire::notification_dto::QueuedNotification);

impl PushFrame {
    /// The notification id — the same id the REST drain pages by.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.0.id
    }

    /// Live-only frames (typing) dispatch as data; durable ones only ring.
    #[must_use]
    pub fn is_transient(&self) -> bool {
        self.0.transient
    }

    /// The wire notification, for the dispatch path. Crate-internal: the DTO is
    /// not part of any public surface.
    pub(crate) fn notification(&self) -> &crate::wire::notification_dto::QueuedNotification {
        &self.0
    }
}

/// Parse one frame's bytes.
///
/// cannon sends its JSON in **binary** WebSocket frames on this route
/// (observations §68), which is why the socket asks for `ArrayBuffer` and this
/// takes bytes rather than a string.
///
/// # Errors
/// `KupferdrahtError::Encoding` for bytes that are not UTF-8, malformed JSON, or a
/// frame with an empty notification id.
pub fn parse_push_frame(bytes: &[u8]) -> Result<PushFrame, KupferdrahtError> {
    let text = std::str::from_utf8(bytes)
        .map_err(|e| KupferdrahtError::Encoding(format!("push frame is not utf-8: {e}")))?;
    crate::wire::push_frame::parse_legacy_frame(text).map(PushFrame)
}

/// Shared state between the ringer (the socket's JS callback) and the waiter
/// (the sync loop). `Cell`/`RefCell` rather than a mutex: the browser is
/// single-threaded, and a lock here would only buy a way to deadlock.
struct Bell {
    /// A durable frame nobody has consumed yet. A flag, not a counter — see
    /// `ring`.
    drain_due: Cell<bool>,
    /// Transient frames waiting to be dispatched, oldest first.
    transients: RefCell<VecDeque<PushFrame>>,
    /// The waiting loop, so a ring wakes it instead of it polling.
    waker: RefCell<Option<Waker>>,
}

impl Bell {
    /// Wake whoever is waiting, if anyone is.
    fn wake(&self) {
        // Waking a waker whose future is already gone is a documented no-op, so
        // a ring after the loop stopped waiting costs nothing.
        if let Some(waker) = self.waker.borrow_mut().take() {
            waker.wake();
        }
    }

    fn ring_durable(&self) {
        self.drain_due.set(true);
        self.wake();
    }

    fn hold_transient(&self, frame: PushFrame) {
        let mut held = self.transients.borrow_mut();
        if held.len() >= MAX_HELD_TRANSIENT_FRAMES {
            held.pop_front();
        }
        held.push_back(frame);
        debug_assert!(held.len() <= MAX_HELD_TRANSIENT_FRAMES);
        drop(held);
        self.wake();
    }

    /// Route one raw frame: a transient one becomes data, everything else rings.
    ///
    /// A frame we cannot parse rings too. That is the doorbell contract, not
    /// laziness: the drain does not need the frame's content, so swallowing an
    /// unreadable frame would lose a message for nothing.
    fn ring_frame(&self, bytes: &[u8]) {
        match parse_push_frame(bytes) {
            Ok(frame) if frame.is_transient() => self.hold_transient(frame),
            Ok(_) => self.ring_durable(),
            Err(e) => {
                debug!(error = %e, "unreadable push frame; draining anyway");
                self.ring_durable();
            }
        }
    }
}

/// A bell the socket rings and the sync loop waits on.
///
/// Rings coalesce: several frames before one drain wake the loop once, because
/// the drain that follows covers all of them (ADR-0030 — the frame's payload is
/// never dispatched, the drain is). That is why `pending` is a flag: counting
/// rings would schedule one redundant drain per frame.
pub struct Doorbell {
    bell: Rc<Bell>,
}

impl Default for Doorbell {
    fn default() -> Self {
        Self::new()
    }
}

impl Doorbell {
    #[must_use]
    pub fn new() -> Self {
        Self {
            bell: Rc::new(Bell {
                drain_due: Cell::new(false),
                transients: RefCell::new(VecDeque::new()),
                waker: RefCell::new(None),
            }),
        }
    }

    /// Ring for a durable frame. Never blocks, never fails — it is called from a
    /// JS event callback, which has nowhere to report an error to.
    pub fn ring(&self) {
        self.bell.ring_durable();
    }

    /// Hand the loop a transient frame. Wakes it, but asks for no drain.
    pub fn ring_transient(&self, frame: PushFrame) {
        self.bell.hold_transient(frame);
    }

    /// Route one raw frame the way the socket does.
    pub fn ring_frame(&self, bytes: &[u8]) {
        self.bell.ring_frame(bytes);
    }

    /// Take every transient frame waiting, oldest first, emptying the queue.
    #[must_use]
    pub fn take_transients(&self) -> Vec<PushFrame> {
        self.bell.transients.borrow_mut().drain(..).collect()
    }

    /// A handle the socket keeps, so ringing outlives this `Doorbell` binding.
    fn handle(&self) -> Rc<Bell> {
        Rc::clone(&self.bell)
    }
}

/// Resolves once the bell has an unconsumed ring, consuming it.
struct Rung(Rc<Bell>);

impl Future for Rung {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        // Consumes nothing: the caller decides what the wake means, because a
        // durable frame and a transient one lead to different work.
        if self.0.drain_due.get() || !self.0.transients.borrow().is_empty() {
            return Poll::Ready(());
        }
        *self.0.waker.borrow_mut() = Some(cx.waker().clone());
        Poll::Pending
    }
}

/// `{base}/await?client={client_id}&access_token={token}` — the legacy
/// doorbell socket, authenticated the only way a browser can.
///
/// # Errors
/// `KupferdrahtError::InvalidInput` for a base that does not parse, or whose scheme
/// is not `ws`/`wss` — the token must never travel in the clear.
pub fn await_url_with_token(
    websocket_base: &str,
    client_id: &str,
    access_token: &str,
) -> Result<url::Url, KupferdrahtError> {
    debug_assert!(
        !client_id.is_empty(),
        "await_url_with_token: client_id must be non-empty"
    );
    if access_token.is_empty() {
        // Not an assertion: a resumed session can legitimately be between
        // refreshes, and a socket without a token connects to nothing useful.
        return Err(KupferdrahtError::InvalidInput(
            "await_url_with_token: empty access token".into(),
        ));
    }
    let trimmed = websocket_base.trim_end_matches('/');
    let mut url = url::Url::parse(&format!("{trimmed}/await")).map_err(|e| {
        KupferdrahtError::InvalidInput(format!(
            "await_url_with_token: bad websocket base `{websocket_base}`: {e}"
        ))
    })?;
    // Wire publishes this host as `https://nginz-ssl.…` in its backend config,
    // so that is what every consumer has and passes to `ServerConfig`. Upgrade
    // it rather than refuse it — found by Step 115's smoke, where the strict
    // check silently left the browser on its timer. The native `await_url`
    // leaves this to the caller; a browser has no reason to.
    let upgraded = match url.scheme() {
        "wss" | "ws" => None,
        "https" => Some("wss"),
        "http" => Some("ws"),
        other => {
            return Err(KupferdrahtError::InvalidInput(format!(
                "await_url_with_token: websocket base must be ws/wss or http(s), got `{other}`"
            )));
        }
    };
    if let Some(scheme) = upgraded {
        url.set_scheme(scheme).map_err(|()| {
            KupferdrahtError::InvalidInput(format!(
                "await_url_with_token: cannot upgrade `{websocket_base}` to {scheme}"
            ))
        })?;
    }
    url.query_pairs_mut()
        .append_pair("client", client_id)
        .append_pair("access_token", access_token);
    Ok(url)
}

/// A connected browser doorbell socket.
///
/// Rings the bell on every frame and never interprets one — ADR-0030 leaves the
/// cursor to the drain alone, so the payload is not even read. Dropping this
/// closes the socket and releases the JS callbacks.
pub struct BrowserPushSocket {
    socket: web_sys::WebSocket,
    /// The JS callbacks must outlive `connect`; dropping a `Closure` invalidates
    /// the function JS still holds.
    _on_message: Closure<dyn FnMut(web_sys::MessageEvent)>,
    _on_close: Closure<dyn FnMut(web_sys::CloseEvent)>,
}

impl BrowserPushSocket {
    /// Is the socket still usable? The loop checks this to decide whether to
    /// reconnect and how long to wait for the bell.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.socket.ready_state() == web_sys::WebSocket::OPEN
    }
}

impl Drop for BrowserPushSocket {
    fn drop(&mut self) {
        // Clear the handlers before closing: a close event firing into a
        // `Closure` that is about to be freed is exactly the use-after-free
        // `wasm-bindgen` warns about.
        self.socket.set_onmessage(None);
        self.socket.set_onclose(None);
        self.socket.set_onerror(None);
        // A failed close means it was already closing or closed.
        let _ = self.socket.close();
    }
}

/// Open Wire's legacy `/await` socket from the browser.
///
/// Both credentials go in the query string — `new WebSocket()` cannot set the
/// `Authorization` header `EventSocket::connect_with_bearer` relies on.
///
/// Every frame rings `doorbell`, and so does the socket's close. Returns once
/// the upgrade completed, so a caller that gets `Ok` can drain immediately: the
/// socket is registered, and anything stored before that registration comes
/// from that first drain (ADR-0030 step 1).
///
/// # Errors
/// `KupferdrahtError::InvalidInput` for a base that is not `ws`/`wss`;
/// `KupferdrahtError::Network` when the socket fails to open.
pub async fn connect(
    websocket_base: &str,
    client_id: &str,
    access_token: &str,
    doorbell: &Doorbell,
) -> Result<BrowserPushSocket, KupferdrahtError> {
    let url = await_url_with_token(websocket_base, client_id, access_token)?;
    let socket = web_sys::WebSocket::new(url.as_str()).map_err(|e| {
        KupferdrahtError::Network(format!("push socket construct: {}", js_text(&e)))
    })?;
    // Frames arrive as Blob by default, which would force an async read just to
    // learn that a frame arrived. We never read the payload, and ArrayBuffer
    // keeps the handler synchronous.
    socket.set_binary_type(web_sys::BinaryType::Arraybuffer);

    let bell = doorbell.handle();
    let on_message =
        Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |event: web_sys::MessageEvent| {
            // A durable frame only rings (ADR-0030 — the drain carries its
            // content); a transient one is handed over as data, because no drain
            // can ever return it. An unreadable frame rings too.
            bell.ring_frame(&frame_bytes(&event));
        });
    socket.set_onmessage(Some(on_message.as_ref().unchecked_ref()));

    // Step 118: a close rings too — the loop wakes at once, reconnects and
    // drains what the gap held, instead of finding the socket dead at its
    // next timeout.
    let closed = doorbell.handle();
    let on_close = Closure::<dyn FnMut(web_sys::CloseEvent)>::new(move |e: web_sys::CloseEvent| {
        debug!(code = e.code(), "browser push socket closed");
        closed.ring_durable();
    });
    socket.set_onclose(Some(on_close.as_ref().unchecked_ref()));

    // Owned before the upgrade completes: a failed upgrade drops it, which
    // detaches the callbacks before the browser's close event can reach them.
    let push_socket = BrowserPushSocket {
        socket,
        _on_message: on_message,
        _on_close: on_close,
    };
    await_open(&push_socket.socket).await?;
    Ok(push_socket)
}

/// Resolve once the upgrade completed, or fail if it did not.
///
/// A JS `Promise` is the bridge: `wasm-bindgen`'s `JsFuture` turns it into a
/// Rust future, and the two one-shot handlers are the only place the outcome can
/// be observed — `WebSocket::new` returns before the upgrade finishes.
async fn await_open(socket: &web_sys::WebSocket) -> Result<(), KupferdrahtError> {
    let promise = js_sys::Promise::new(&mut |resolve, reject| {
        let opened = Closure::once_into_js(move || {
            let _ = resolve.call0(&JsValue::NULL);
        });
        let failed = Closure::once_into_js(move || {
            let _ = reject.call0(&JsValue::NULL);
        });
        socket.set_onopen(Some(opened.unchecked_ref()));
        socket.set_onerror(Some(failed.unchecked_ref()));
    });
    let outcome = wasm_bindgen_futures::JsFuture::from(promise).await;
    // One-shot handlers: leaving them attached would fire into freed closures
    // once the socket is live.
    socket.set_onopen(None);
    socket.set_onerror(None);
    outcome
        .map(|_| ())
        // `onerror` carries no detail in any browser — the spec withholds it so a
        // page cannot probe the network. The close code is the only signal.
        .map_err(|_| KupferdrahtError::Network("push socket upgrade failed".into()))
}

/// The frame's bytes, whichever shape the browser handed us.
///
/// `ArrayBuffer` is what this socket asks for, and what cannon sends on this
/// route. A text frame is handled too — the route's contract is JSON, not a
/// particular frame type, and an echo or a future change should not crash us.
fn frame_bytes(event: &web_sys::MessageEvent) -> Vec<u8> {
    let data = event.data();
    if let Some(text) = data.as_string() {
        return text.into_bytes();
    }
    if data.has_type::<js_sys::ArrayBuffer>() {
        return js_sys::Uint8Array::new(&data).to_vec();
    }
    // A Blob would need an async read; we never ask for one (`binary_type` is
    // ArrayBuffer), so treat anything else as an empty frame — which still
    // rings, because an unreadable frame must drain.
    Vec::new()
}

/// A `JsValue` error rendered for a message.
fn js_text(value: &JsValue) -> String {
    value
        .as_string()
        .unwrap_or_else(|| format!("{:?}", value.clone()))
}

/// Wait for the bell, or give up after `timeout` and let the timer drive.
///
/// This is the seam that turns the polling loop into a push loop: the socket
/// stays a pure trigger, so a dead socket degrades to exactly today's
/// behaviour rather than stopping receive.
pub async fn wait_for_bell_or(bell: &Doorbell, timeout: Duration) -> Wake {
    use futures_util::future::{Either, select};

    // `crate::time::sleep` is the browser-safe one (W4): tokio's timer has no
    // driver here and would hang forever.
    match select(
        Box::pin(Rung(bell.handle())),
        Box::pin(crate::time::sleep(timeout)),
    )
    .await
    {
        // A durable frame outranks a transient: the drain it asks for covers
        // everything stored, and the transients are delivered either way.
        Either::Left(_) if bell.bell.drain_due.replace(false) => Wake::Rang,
        Either::Left(_) => Wake::Transient,
        Either::Right(_) => Wake::TimedOut,
    }
}
