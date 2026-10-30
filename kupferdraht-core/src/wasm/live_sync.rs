// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! W4 — browser live sync: a background `spawn_local` loop that drives
//! `sync_once` and pushes decrypted messages to a JS callback via a real
//! `EventHandler`, replacing the prototype's `setInterval` poll.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use send_wrapper::SendWrapper;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::client::Client;
use crate::error::KupferdrahtError;
use crate::sync::{EventHandler, Message, MessageBody, SyncSettings, SyncSummary, TypingEvent};
use crate::wasm::push_socket::{self, BrowserPushSocket, Doorbell, Wake, wait_for_bell_or};
use crate::wasm::{push_state, sync_pace};

/// Plain, JS-serializable projection of an incoming [`Message`].
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct MessageView {
    room_id: String,
    sender_user_id: String,
    message_id: String,
    body: Option<String>,
    reply_to_id: Option<String>,
    received_at_ms: i64,
    /// Step 89: set only for a self-deleting message.
    expires_at_ms: Option<i64>,
}

impl From<&Message> for MessageView {
    fn from(m: &Message) -> Self {
        let body = match &m.body {
            MessageBody::Text(t) => Some(t.clone()),
            _ => None,
        };
        Self {
            room_id: m.room_id.clone(),
            sender_user_id: m.sender_user_id.clone(),
            message_id: m.message_id.clone(),
            body,
            reply_to_id: m.reply_to_id.clone(),
            received_at_ms: m.received_at_ms,
            expires_at_ms: m.expires_at_ms,
        }
    }
}

/// Plain, JS-serializable projection of a [`TypingEvent`].
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct TypingView {
    room_id: String,
    typing_users: Vec<String>,
}

impl From<&TypingEvent> for TypingView {
    fn from(e: &TypingEvent) -> Self {
        Self {
            room_id: e.room_id.clone(),
            typing_users: e.typing_users.clone(),
        }
    }
}

/// A JS callback surfaced as a core [`EventHandler`]. `SendWrapper` lets the
/// `!Send` `js_sys::Function` satisfy the trait's `Send + Sync` bound — the
/// browser is single-threaded, so the handler is only ever invoked on the
/// origin thread and the wrapper never panics.
struct JsEventHandler {
    on_message: SendWrapper<js_sys::Function>,
    on_typing: Option<SendWrapper<js_sys::Function>>,
}

impl EventHandler for JsEventHandler {
    fn on_message(&self, message: Message) {
        if let Ok(val) = serde_wasm_bindgen::to_value(&MessageView::from(&message)) {
            let _ = self.on_message.call1(&JsValue::NULL, &val);
        }
    }

    fn on_room_invited(&self, _room_id: String) {}

    fn on_sync_error(&self, description: String) {
        web_sys::console::error_1(&JsValue::from_str(&format!(
            "kupferdraht sync: {description}"
        )));
    }

    fn on_typing(&self, event: TypingEvent) {
        if let Some(cb) = &self.on_typing
            && let Ok(val) = serde_wasm_bindgen::to_value(&TypingView::from(&event))
        {
            let _ = cb.call1(&JsValue::NULL, &val);
        }
    }
}

/// Handle to the running background sync loop; call [`SyncHandle::stop`] to end it.
#[wasm_bindgen]
pub struct SyncHandle {
    stop: Rc<Cell<bool>>,
    /// Step 122: the account whose push state `stop` clears.
    account: Option<String>,
    /// Step 95d: the `onDevicesChanged` registration, ended by `stop`.
    devices: std::cell::RefCell<Option<crate::timeline::Subscription>>,
    /// Step 113: the `onStorage` registration (account, observer), ended by
    /// `stop`.
    storage: std::cell::RefCell<Option<(String, crate::storage::remote::health::ObserverId)>>,
}

#[wasm_bindgen]
impl SyncHandle {
    /// Stop the background sync loop and its callbacks (idempotent).
    pub fn stop(&self) {
        self.stop.set(true);
        if let Some(account) = &self.account {
            push_state::clear_push_state(account);
        }
        self.devices.borrow_mut().take();
        if let Some((account, id)) = self.storage.borrow_mut().take() {
            crate::storage::unsubscribe_storage_state(&account, id);
        }
    }
}

/// Plain, JS-serializable projection of a per-pass [`SyncSummary`] — the
/// `onSync` payload (Step 94, ADR-0026: the one signal a thin tab has that
/// the room list may have changed).
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SyncSummaryView {
    rooms_updated: u32,
    events_dispatched: u32,
}

impl From<SyncSummary> for SyncSummaryView {
    fn from(s: SyncSummary) -> Self {
        Self {
            rooms_updated: s.rooms_updated,
            events_dispatched: s.events_dispatched,
        }
    }
}

/// The JavaScript callbacks of `startSync`, one per event.
pub(crate) struct Callbacks {
    pub(crate) message: js_sys::Function,
    pub(crate) typing: Option<js_sys::Function>,
    pub(crate) expired: Option<js_sys::Function>,
    pub(crate) sync: Option<js_sys::Function>,
    pub(crate) devices_changed: Option<js_sys::Function>,
    pub(crate) storage: Option<js_sys::Function>,
}

/// Register the handlers and start the background sync loop for `client`.
/// `on_typing` fires per inbound typing event; `on_expired` fires once if the
/// loop stops because the refresh token died (the UI should prompt a re-login);
/// `on_sync` fires after every successful pass with `{ roomsUpdated,
/// eventsDispatched }` (Step 94 — what the leader tab broadcasts as `sync`);
/// `on_devices_changed` fires per room whose device set changed, with
/// `{ roomId, added, removed }` (Step 95d — broadcast as `devices`);
/// `on_storage` fires with the account's storage state on each change
/// (Step 113 — broadcast as `storage`).
pub(crate) fn start(client: Arc<Client>, callbacks: Callbacks) -> SyncHandle {
    let Callbacks {
        message: on_message,
        typing: on_typing,
        expired: on_expired,
        sync: on_sync,
        devices_changed: on_devices_changed,
        storage: on_storage,
    } = callbacks;
    let storage = on_storage.and_then(|cb| watch_storage(&client, cb));
    let devices = on_devices_changed.map(|cb| {
        client.subscribe_device_changes(Arc::new(super::devices::JsDevicesListener {
            on_devices_changed: SendWrapper::new(cb),
        }))
    });
    client.add_event_handler(Arc::new(JsEventHandler {
        on_message: SendWrapper::new(on_message),
        on_typing: on_typing.map(SendWrapper::new),
    }));
    let stop = Rc::new(Cell::new(false));
    let account = client.user_id().map(|user| user.as_str().to_owned());
    spawn_local(run_sync_loop(
        client,
        Rc::clone(&stop),
        on_expired.map(SendWrapper::new),
        on_sync.map(SendWrapper::new),
    ));
    SyncHandle {
        stop,
        account,
        devices: std::cell::RefCell::new(devices),
        storage: std::cell::RefCell::new(storage),
    }
}

/// The background loop (Steps 115, 118, 122): drain, then wait for the
/// doorbell or an own send.
///
/// With a live socket the loop waits for the bell — a frame, or the socket's
/// close — up to [`sync_pace::PUSH_RENEWAL`]; a socket silent that long is
/// renewed, because a browser cannot ping and a silently dead one looks open.
/// Without a socket it polls every [`sync_pace::NO_SOCKET_POLL`] and retries
/// the socket. A successful send wakes it too (Step 122): the backend sends
/// the sender no frame, and the pass's `onSync` is the embedder's cue.
async fn run_sync_loop(
    client: Arc<Client>,
    stop: Rc<Cell<bool>>,
    on_expired: Option<SendWrapper<js_sys::Function>>,
    on_sync: Option<SendWrapper<js_sys::Function>>,
) {
    // The socket is a doorbell, never a receive path (ADR-0030).
    let doorbell = Doorbell::new();
    let mut socket: Option<BrowserPushSocket> = None;
    let mut last_connect_ms: Option<i64> = None;
    // The first pass always drains: a session that just started has whatever
    // arrived while it was away waiting in the queue.
    let mut drain_needed = true;
    // Step 119: an evicted instance stops — its socket would close the new
    // browser's (both are the account's Wire client).
    while !stop.get() && !crate::storage::browser_storage_evicted() {
        reconnect_if_due(&client, &doorbell, &mut socket, &mut last_connect_ms).await;
        let socket_open = socket.as_ref().is_some_and(BrowserPushSocket::is_open);
        record_push_state(&client, &stop, socket_open);
        if drain_needed {
            match drain_once(&client, on_expired.as_deref(), on_sync.as_deref()).await {
                Pass::End => break,
                Pass::Again => continue,
                // Step 122: the pass's `onSync` came after every send before
                // it — its own delivery receipts included.
                Pass::Wait => _ = client.sent_signal().take(),
            }
        }
        let wake =
            wait_for_bell_or_send(&client, &doorbell, sync_pace::idle_wait(socket_open)).await;
        dispatch_transients(&client, &doorbell).await;
        if wake == Wake::TimedOut {
            // Renew a silent socket: the next pass reconnects, then drains.
            drop(socket.take());
        }
        // A transient-only wake stored nothing to drain (ADR-0030 d.4).
        drain_needed = wake != Wake::Transient;
    }
    if let Some(user) = client.user_id() {
        push_state::clear_push_state(user.as_str());
    }
}

/// Record whether the running loop has an open socket (Step 122) — unless
/// the consumer stopped it, which cleared the state already.
fn record_push_state(client: &Client, stop: &Cell<bool>, socket_open: bool) {
    if let Some(user) = client.user_id()
        && !stop.get()
    {
        push_state::set_push_state(user.as_str(), socket_open);
    }
}

/// Wait for the bell or a successful own send (Step 122), whichever comes
/// first, up to `timeout`. A send wakes the loop like a durable frame.
async fn wait_for_bell_or_send(client: &Client, doorbell: &Doorbell, timeout: Duration) -> Wake {
    use futures_util::future::{Either, select};

    match select(
        Box::pin(wait_for_bell_or(doorbell, timeout)),
        Box::pin(client.sent_signal().wait()),
    )
    .await
    {
        Either::Left((wake, _)) => wake,
        Either::Right(((), _)) => Wake::Rang,
    }
}

/// What one drain pass asks of the loop.
enum Pass {
    /// Wait for the bell.
    Wait,
    /// Drain again at once: a `429` was waited out (Step 103).
    Again,
    /// Stop: the refresh token died (`SessionExpired`, JS was told), or the
    /// instance was evicted (`onEvicted` told it, Step 119).
    End,
}

/// One `sync_once` pass. A dead refresh token is terminal; every other
/// error retries after the wait.
async fn drain_once(
    client: &Client,
    on_expired: Option<&js_sys::Function>,
    on_sync: Option<&js_sys::Function>,
) -> Pass {
    match client.sync_once(SyncSettings::default()).await {
        Ok(summary) => {
            report_pass(on_sync, summary);
            Pass::Wait
        }
        Err(KupferdrahtError::SessionExpired) => {
            if let Some(cb) = on_expired {
                let _ = cb.call0(&JsValue::NULL);
            }
            Pass::End
        }
        Err(KupferdrahtError::RateLimited { retry_after_secs }) => {
            crate::time::sleep(SyncSettings::default().rate_limit_pause(retry_after_secs)).await;
            Pass::Again
        }
        // Step 119: a pass that met a takeover ends the loop.
        Err(_) if crate::storage::evicted_by_takeover() => Pass::End,
        Err(_) => Pass::Wait,
    }
}

/// Open the socket when there is no live one and a connect is due (at most
/// one per [`sync_pace::NO_SOCKET_POLL`] — a server that accepts and closes
/// at once cannot spin the loop). A dead socket is dropped first, so its close cannot ring
/// for the successor.
async fn reconnect_if_due(
    client: &Client,
    doorbell: &Doorbell,
    socket: &mut Option<BrowserPushSocket>,
    last_connect_ms: &mut Option<i64>,
) {
    let now_ms = crate::time::now_ms();
    if socket.as_ref().is_some_and(BrowserPushSocket::is_open)
        || !sync_pace::connect_due(*last_connect_ms, now_ms)
    {
        return;
    }
    drop(socket.take());
    *last_connect_ms = Some(now_ms);
    *socket = open_doorbell_socket(client, doorbell).await;
}

/// Call `on_storage` with the account's storage state on each change
/// (Step 113); `None` without an account on the storage server.
fn watch_storage(
    client: &Client,
    on_storage: js_sys::Function,
) -> Option<(String, crate::storage::remote::health::ObserverId)> {
    let account = client.user_id()?.as_str().to_owned();
    let id = crate::storage::subscribe_storage_state(
        &account,
        Box::new(move |state| {
            if let Err(e) = on_storage.call1(&JsValue::NULL, &JsValue::from_str(state.as_str())) {
                web_sys::console::error_1(&e);
            }
        }),
    )?;
    Some((account, id))
}

/// Dispatch every transient frame the socket handed over, oldest first.
///
/// Typing exists only on the socket — a REST drain can never return it — so this
/// is the one place a frame's payload reaches the `EventHandler` in the browser.
/// A failure is logged and dropped: a lost typing indicator must never stop
/// receive.
async fn dispatch_transients(client: &Arc<Client>, doorbell: &Doorbell) {
    for frame in doorbell.take_transients() {
        if let Err(e) = client.dispatch_transient(frame.notification()).await {
            tracing::debug!(error = %e, id = frame.id(), "transient frame not dispatched");
        }
    }
}

/// Open the doorbell socket for this session, or `None` if it cannot be opened.
///
/// Every failure here is survivable: the caller falls back to its timer. A
/// session without a device id has no socket to open — the queue is
/// device-scoped — and a session without an access token cannot authenticate
/// one, because the browser can only pass it in the query string.
async fn open_doorbell_socket(client: &Client, doorbell: &Doorbell) -> Option<BrowserPushSocket> {
    let device_id = client.device_id()?.to_owned();
    let token = client.access_token()?;
    match push_socket::connect(client.websocket_url(), &device_id, &token, doorbell).await {
        Ok(socket) => Some(socket),
        Err(e) => {
            // Not an error the consumer needs: receive continues on the timer.
            tracing::debug!(error = %e, "push socket unavailable; syncing on the timer");
            None
        }
    }
}

/// Hand one pass's summary to the optional `onSync` callback.
fn report_pass(on_sync: Option<&js_sys::Function>, summary: SyncSummary) {
    if let Some(cb) = on_sync
        && let Ok(val) = serde_wasm_bindgen::to_value(&SyncSummaryView::from(summary))
    {
        let _ = cb.call1(&JsValue::NULL, &val);
    }
}
