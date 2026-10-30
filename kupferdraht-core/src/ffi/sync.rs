// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! FFI wrappers for sync + observers (Step 20c-5).
//!
//! The core observer traits (`EventHandler`, `TimelineListener`) carry
//! `Box<AssetSummary>`-bearing types, so they can't be exported directly.
//! Instead each gets a foreign-implementable `#[uniffi::export(with_foreign)]`
//! twin here, plus a private Rust **adapter** that implements the core
//! trait and forwards core→ffi-converted values. The continuous sync loop
//! is exposed as `Client::start_sync` returning a `TaskHandle` (abort on
//! `cancel()` / drop), rather than the non-FFI-able core `sync` /
//! `sync_with_callback` / `sync_stream`. See
//! `docs/plan/20c-full-ffi-surface.md`.
//!
//! **All foreign callbacks here fire synchronously on the tokio sync
//! task.** A callback that blocks (disk, network, a held lock) stalls
//! the sync loop and ties up a runtime worker for every consumer of
//! this `Client`. Implementations must return fast — record the value
//! and dispatch heavy work to their own executor / queue.

use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use tokio::sync::Notify;

use super::Client;
use super::timeline::{AssetMeta, TimelineDiff};
use crate::error::KupferdrahtError;
use crate::sync::{
    EventHandler as CoreEventHandler, Message as CoreMessage, MessageBody as CoreBody,
    SyncSettings, SyncSummary, TypingEvent as CoreTypingEvent,
};
use crate::timeline::{Subscription as CoreSubscription, TimelineListener as CoreTimelineListener};

/// Decoded body of an inbound [`Message`].
// FFI enums cross the boundary by serialization, not by in-memory
// layout, so the `Text`/`Asset` size gap is irrelevant — and a boxed
// variant field would not marshal through UniFFI.
#[allow(clippy::large_enum_variant)]
// No `Eq`: the `Location` variant carries `f32` coords (Step 26).
#[derive(Debug, Clone, PartialEq, uniffi::Enum)]
pub enum MessageBody {
    Text {
        text: String,
    },
    Asset {
        asset: AssetMeta,
    },
    /// A nudge / "ping". Step 26.
    Knock {
        hot_knock: bool,
    },
    /// A shared map point. Step 26.
    Location {
        latitude: f32,
        longitude: f32,
        name: Option<String>,
        zoom: Option<i32>,
    },
    /// Full state of one task (ADR-0014). Step 60a.
    Task {
        task: super::timeline::TaskMeta,
    },
    /// Full case snapshot (ADR-0018). Step 72b. `sender_authorized`
    /// = the sender was a conversation admin at receive time.
    Case {
        case: crate::timeline::CaseSnapshot,
        sender_authorized: bool,
    },
    /// Full task-list-marker state (ADR-0020) — the audit trail of
    /// the any-member flag; the fold is `Room::is_task_list()`.
    /// Step 75.
    TaskListMarker {
        marked: bool,
    },
}

impl From<&CoreBody> for MessageBody {
    fn from(body: &CoreBody) -> Self {
        match body {
            CoreBody::Text(text) => Self::Text { text: text.clone() },
            CoreBody::Asset(summary) => Self::Asset {
                asset: AssetMeta::from(summary.as_ref()),
            },
            CoreBody::Knock { hot_knock } => Self::Knock {
                hot_knock: *hot_knock,
            },
            CoreBody::Location {
                latitude,
                longitude,
                name,
                zoom,
            } => Self::Location {
                latitude: *latitude,
                longitude: *longitude,
                name: name.clone(),
                zoom: *zoom,
            },
            CoreBody::Task(item) => Self::Task {
                task: super::timeline::TaskMeta::from(item.as_ref()),
            },
            CoreBody::Case {
                snapshot,
                sender_authorized,
            } => Self::Case {
                case: snapshot.as_ref().clone(),
                sender_authorized: *sender_authorized,
            },
            CoreBody::TaskListMarker { marked } => Self::TaskListMarker { marked: *marked },
        }
    }
}

/// One inbound application message delivered to an [`EventHandler`].
// `message_id` mirrors the core field name; the prefix is intentional.
#[allow(clippy::struct_field_names)]
// No `Eq`: `MessageBody::Location` carries `f32` coords (Step 26).
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct Message {
    pub room_id: String,
    pub sender_user_id: String,
    pub sender_device_id: Option<String>,
    pub body: MessageBody,
    pub received_at_ms: i64,
    pub message_id: String,
    pub reply_to_id: Option<String>,
    /// Wall-clock (ms since Unix epoch) at which the message's
    /// ephemeral timer elapses — set only for a self-deleting message
    /// (`received_at_ms + ttl`). Step 89.
    pub expires_at_ms: Option<i64>,
}

impl From<&CoreMessage> for Message {
    fn from(message: &CoreMessage) -> Self {
        Self {
            room_id: message.room_id.clone(),
            sender_user_id: message.sender_user_id.clone(),
            sender_device_id: message.sender_device_id.clone(),
            body: MessageBody::from(&message.body),
            received_at_ms: message.received_at_ms,
            message_id: message.message_id.clone(),
            reply_to_id: message.reply_to_id.clone(),
            expires_at_ms: message.expires_at_ms,
        }
    }
}

/// The current set of users typing in a room, delivered to
/// [`EventHandler::on_typing`]. Carries the *current* set — an empty
/// `typing_users` means nobody is typing (clear the indicator).
/// `typing_users` are qualified id strings, matching
/// [`Message::sender_user_id`]'s flat form.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct TypingEvent {
    pub room_id: String,
    pub typing_users: Vec<String>,
}

impl From<&CoreTypingEvent> for TypingEvent {
    fn from(event: &CoreTypingEvent) -> Self {
        Self {
            room_id: event.room_id.clone(),
            typing_users: event.typing_users.clone(),
        }
    }
}

/// Consumer notification surface for inbound events. Implemented in
/// Kotlin / Swift. Callbacks fire from the tokio sync task, so the
/// implementation must be `Send + Sync` (enforced by `with_foreign`).
///
/// **Return fast.** Callbacks run synchronously on the sync task — a
/// blocking implementation stalls the sync loop. Dispatch heavy work
/// to your own executor / queue.
#[uniffi::export(with_foreign)]
pub trait EventHandler: Send + Sync {
    fn on_message(&self, message: Message);
    fn on_room_invited(&self, room_id: String);
    fn on_sync_error(&self, description: String);
    /// The set of users typing in a room changed (an inbound
    /// `conversation.typing` event). `event` carries the current set;
    /// render it verbatim. Step 21.
    ///
    /// Fires only on the push transport (`SyncSettings.transport =
    /// SyncTransport::WebSocket`, Steps 62/73): Wire delivers typing
    /// as a transient event over the live socket — whichever dialect
    /// the connection negotiated (`/v14/events` or the legacy
    /// `/await` route, ADR-0019). Polling consumers never receive
    /// it. The send path works on every transport.
    fn on_typing(&self, event: TypingEvent);
}

struct EventHandlerAdapter {
    foreign: Arc<dyn EventHandler>,
}

impl CoreEventHandler for EventHandlerAdapter {
    fn on_message(&self, message: CoreMessage) {
        self.foreign.on_message(Message::from(&message));
    }
    fn on_room_invited(&self, room_id: String) {
        self.foreign.on_room_invited(room_id);
    }
    fn on_sync_error(&self, description: String) {
        self.foreign.on_sync_error(description);
    }
    fn on_typing(&self, event: CoreTypingEvent) {
        self.foreign.on_typing(TypingEvent::from(&event));
    }
}

/// Listener for a room's timeline diffs (register via
/// `Timeline::subscribe` / `Client::add_timeline_listener`).
///
/// **Return fast.** `on_update` runs synchronously on the sync task — a
/// blocking implementation stalls the sync loop. Dispatch heavy work
/// to your own executor / queue.
#[uniffi::export(with_foreign)]
pub trait TimelineListener: Send + Sync {
    fn on_update(&self, diff: TimelineDiff);
}

struct TimelineListenerAdapter {
    foreign: Arc<dyn TimelineListener>,
}

impl CoreTimelineListener for TimelineListenerAdapter {
    fn on_update(&self, diff: crate::timeline::TimelineDiff) {
        self.foreign.on_update(TimelineDiff::from(&diff));
    }
}

pub(super) fn adapt_timeline_listener(
    foreign: Arc<dyn TimelineListener>,
) -> Arc<dyn CoreTimelineListener> {
    Arc::new(TimelineListenerAdapter { foreign })
}

/// Listener for device-set changes across all rooms (Step 95d; register
/// via `Client::subscribe_device_changes`). One call per room whose MLS
/// device set changed against the last persisted set — the signal for a
/// device nobody expected (under `BasicCredential`s the backend
/// distributes the keys).
///
/// **Return fast.** Runs synchronously on the task that observed the
/// change (the sync loop or a membership call).
#[uniffi::export(with_foreign)]
pub trait RoomDevicesListener: Send + Sync {
    fn on_devices_changed(&self, change: crate::rooms::RoomDevicesChange);
}

struct RoomDevicesListenerAdapter {
    foreign: Arc<dyn RoomDevicesListener>,
}

impl crate::rooms::RoomDevicesListener for RoomDevicesListenerAdapter {
    fn on_devices_changed(&self, change: crate::rooms::RoomDevicesChange) {
        self.foreign.on_devices_changed(change);
    }
}

/// Per-iteration sink for [`Client::start_sync`]. Returning `false` from
/// `on_summary` stops the loop.
///
/// **Return fast.** Callbacks run synchronously on the sync loop's own
/// task — a blocking implementation delays the next iteration. Dispatch
/// heavy work to your own executor / queue.
#[uniffi::export(with_foreign)]
pub trait SyncListener: Send + Sync {
    fn on_summary(&self, summary: SyncSummary) -> bool;
    /// Called after a failed iteration. Return `true` to keep the loop
    /// alive (it retries after a backoff — see [`Client::start_sync`]),
    /// `false` to stop it. A transient `SyncAlreadyRunning` overlap
    /// (e.g. a manual `sync_once` racing the loop) surfaces here too
    /// and recovers on the next retry.
    fn on_error(&self, description: String) -> bool;
    /// The session is permanently invalid — the refresh token (`zuid`) is dead
    /// (`KupferdrahtError::SessionExpired`), so no refresh can recover it. The loop
    /// has already **stopped**; this fires at most once instead of `on_error`.
    /// The consumer must drive a re-login — the device's MLS state + chats are
    /// preserved and reused on the next login. Step 39.
    fn on_session_expired(&self);
}

/// Handle to a running background sync loop.
///
/// Call `cancel()` to stop it. Cancellation is **cooperative**: the loop
/// finishes the iteration in flight (so the notifications cursor / MLS
/// state never tears mid-`sync_once`), then exits — at most one iteration
/// late. Dropping the handle also cancels — but on the JVM, object
/// finalization is GC-timed, so **call `cancel()` explicitly** for prompt
/// shutdown; relying on drop can leave the loop running until the next GC.
#[derive(uniffi::Object)]
pub struct TaskHandle {
    cancelled: Arc<AtomicBool>,
    notify: Arc<Notify>,
}

impl TaskHandle {
    fn signal_cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        // Wake a loop parked in its inter-iteration sleep; a loop inside
        // `sync_once` sees the flag at the next boundary check.
        self.notify.notify_one();
    }
}

#[uniffi::export]
impl TaskHandle {
    /// Stop the running sync loop after the current iteration. Idempotent;
    /// also happens on drop.
    pub fn cancel(&self) {
        self.signal_cancel();
    }
}

impl Drop for TaskHandle {
    fn drop(&mut self) {
        self.signal_cancel();
    }
}

/// Handle for a registered [`TimelineListener`].
///
/// Call `unsubscribe()` to stop receiving updates. Dropping the handle
/// also unsubscribes — but on the JVM, object finalization is GC-timed,
/// so **call `unsubscribe()` explicitly** for prompt removal; relying on
/// drop can keep the listener live until the next GC.
#[derive(uniffi::Object)]
pub struct Subscription {
    inner: Mutex<Option<CoreSubscription>>,
}

impl Subscription {
    pub(super) fn wrap(sub: CoreSubscription) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(Some(sub)),
        })
    }

    fn unsubscribe_inner(&self) {
        let mut guard = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(sub) = guard.take() {
            sub.unsubscribe();
        }
    }
}

#[uniffi::export]
impl Subscription {
    /// Stop receiving updates. Idempotent; also happens on drop.
    pub fn unsubscribe(&self) {
        self.unsubscribe_inner();
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        self.unsubscribe_inner();
    }
}

#[uniffi::export]
impl Client {
    /// Register an event handler for inbound messages / invites / sync
    /// errors. Lives for the `Client`'s lifetime (no unsubscribe).
    pub fn add_event_handler(&self, handler: Arc<dyn EventHandler>) {
        self.inner
            .add_event_handler(Arc::new(EventHandlerAdapter { foreign: handler }));
    }

    /// Register a listener for timeline diffs across all rooms.
    #[must_use]
    pub fn add_timeline_listener(&self, listener: Arc<dyn TimelineListener>) -> Arc<Subscription> {
        let sub = self
            .inner
            .add_timeline_listener(adapt_timeline_listener(listener));
        Subscription::wrap(sub)
    }

    /// Register a listener for device-set changes across all rooms
    /// (Step 95d). Drop the returned `Subscription` to stop.
    #[must_use]
    pub fn subscribe_device_changes(
        &self,
        listener: Arc<dyn RoomDevicesListener>,
    ) -> Arc<Subscription> {
        Subscription::wrap(
            self.inner
                .subscribe_device_changes(Arc::new(RoomDevicesListenerAdapter {
                    foreign: listener,
                })),
        )
    }

    /// This device's MLS thumbprint (Step 95d) — Wire's "MLS
    /// Thumbprint", comparable with the official apps.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub fn own_device_thumbprint(&self) -> Result<String, KupferdrahtError> {
        self.inner.own_device_thumbprint()
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl Client {
    /// One sync iteration: a metadata refresh plus one notifications
    /// drain, dispatching events to the registered handlers.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn sync_once(&self, settings: SyncSettings) -> Result<SyncSummary, KupferdrahtError> {
        self.inner.sync_once(settings).await
    }

    /// Start a background sync loop, reporting each iteration's summary to
    /// `listener`; returns a [`TaskHandle`] to cancel it. The loop stops
    /// when `listener.on_summary` / `listener.on_error` returns `false`,
    /// or when the handle is cancelled / dropped. A failed iteration does
    /// **not** kill the loop by itself — when `on_error` returns `true`
    /// the loop retries after `poll_interval` (2 s when unset), after a
    /// `KupferdrahtError::RateLimited` after the backend's `Retry-After`
    /// (bounded to 1–60 s). Between successful iterations it pauses
    /// `poll_interval`, 10 s when unset (Steps 103, 118). Retry
    /// policy beyond that (give-up counters, exponential growth) belongs
    /// to the listener, which sees every failure. `sync_once` always runs
    /// to completion — cancellation takes effect at iteration boundaries
    /// (see [`TaskHandle`]).
    // `async` is required even though we only spawn: `tokio::spawn` needs
    // a runtime context, which the UniFFI tokio runtime provides for
    // async exports but not for sync ones.
    #[allow(unknown_lints, clippy::unused_async_trait_impl)]
    #[allow(clippy::unused_async)]
    pub async fn start_sync(
        &self,
        settings: SyncSettings,
        listener: Arc<dyn SyncListener>,
    ) -> Arc<TaskHandle> {
        // Drop typing indicators left over from a prior sync session
        // before this loop begins (Step 21 orphan safety net).
        self.inner.reset_typing_on_sync_start();
        let inner = self.inner.clone();
        let cancelled = Arc::new(AtomicBool::new(false));
        let notify = Arc::new(Notify::new());
        // Step 62b: the WebSocket transport is a continuous push
        // session, not an iteration step — it gets its own runner
        // instead of the per-iteration loop below.
        if settings.transport == crate::sync::SyncTransport::WebSocket {
            let runner = run_push_sync(
                inner,
                settings,
                listener.clone(),
                cancelled.clone(),
                notify.clone(),
            );
            tokio::spawn(report_runner_panic(runner, listener));
            return Arc::new(TaskHandle { cancelled, notify });
        }
        let step_settings = settings.clone();
        let step = move || {
            let inner = inner.clone();
            let settings = step_settings.clone();
            async move { inner.sync_once(settings).await }
        };
        let runner = run_sync_loop(
            step,
            settings,
            listener.clone(),
            cancelled.clone(),
            notify.clone(),
        );
        tokio::spawn(report_runner_panic(runner, listener));
        Arc::new(TaskHandle { cancelled, notify })
    }
}

/// Error-retry pause when `SyncSettings.poll_interval` is unset. Matches
/// the documented 2-second idle-backoff precedent (Step 5d,
/// `sync-stream --idle-backoff 2`).
const ERROR_RETRY_BACKOFF: std::time::Duration = std::time::Duration::from_secs(2);

fn error_backoff(settings: &SyncSettings) -> std::time::Duration {
    settings.poll_interval.unwrap_or(ERROR_RETRY_BACKOFF)
}

/// The pause after a failed iteration: the rate-limit wait after a `429`
/// (Step 103), else [`error_backoff`].
fn error_pause(settings: &SyncSettings, error: &KupferdrahtError) -> std::time::Duration {
    match error {
        KupferdrahtError::RateLimited { retry_after_secs } => {
            settings.rate_limit_pause(*retry_after_secs)
        }
        _ => error_backoff(settings),
    }
}

/// Step 95c: run a detached sync runner so a panic inside it reaches the
/// listener's `on_error` instead of dying silently with the spawned task.
/// The runner is over after a panic — its state is suspect, so the
/// listener's retry answer is not consulted. wasm cannot get here: a
/// panic there traps the module (`docs/bindings.md`).
async fn report_runner_panic<Fut>(runner: Fut, listener: Arc<dyn SyncListener>)
where
    Fut: Future<Output = ()> + Send,
{
    use futures_util::FutureExt as _;
    if let Err(payload) = std::panic::AssertUnwindSafe(runner).catch_unwind().await {
        let reason = panic_message(payload.as_ref());
        tracing::error!(%reason, "sync runner panicked; sync stopped");
        let _ = listener.on_error(format!("sync stopped after an internal panic: {reason}"));
    }
}

/// The text of a panic payload (`&str` or `String`), else a placeholder.
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "non-string panic payload".to_string())
}

/// The `start_sync` body for `SyncTransport::WebSocket` (Step 62b):
/// one summary per event frame via `Client::sync_with_callback`, the
/// same listener bool-contract as the polling loop. Cancellation
/// drops the push future outright — the socket closes, and a frame
/// caught between cursor-persist and ack is redelivered and deduped
/// on the next session (the push loop's designed crash window).
/// Hard errors consult `listener.on_error` and retry after
/// [`error_backoff`], `SessionExpired` stops the loop via
/// `on_session_expired` — both mirroring the polling runner below.
async fn run_push_sync(
    client: Arc<crate::client::Client>,
    settings: SyncSettings,
    listener: Arc<dyn SyncListener>,
    cancelled: Arc<AtomicBool>,
    notify: Arc<Notify>,
) {
    loop {
        let cb_listener = listener.clone();
        let cb_cancelled = cancelled.clone();
        let push = client.sync_with_callback(settings.clone(), move |summary| {
            if cb_cancelled.load(Ordering::SeqCst) || !cb_listener.on_summary(summary) {
                std::ops::ControlFlow::Break(())
            } else {
                std::ops::ControlFlow::Continue(())
            }
        });
        tokio::select! {
            () = notify.notified() => return,
            result = push => match result {
                // Ok = the listener (or the cancel flag) broke the loop.
                Ok(()) => return,
                Err(KupferdrahtError::SessionExpired) => {
                    listener.on_session_expired();
                    return;
                }
                Err(error) => {
                    if !listener.on_error(error.to_string()) {
                        return;
                    }
                }
            }
        }
        if cancelled.load(Ordering::SeqCst) {
            return;
        }
        tokio::select! {
            () = notify.notified() => return,
            () = tokio::time::sleep(error_backoff(&settings)) => {}
        }
    }
}

/// The `start_sync` loop body, generic over the sync step so tests can
/// script results. Pauses `poll_interval` (10 s when unset, Steps 103, 118)
/// after an Ok iteration and [`error_pause`] after an Err one; both
/// pauses wake instantly on cancel via `notify`.
async fn run_sync_loop<F, Fut>(
    mut sync_once: F,
    settings: SyncSettings,
    listener: Arc<dyn SyncListener>,
    cancelled: Arc<AtomicBool>,
    notify: Arc<Notify>,
) where
    F: FnMut() -> Fut + Send,
    Fut: Future<Output = Result<SyncSummary, KupferdrahtError>> + Send,
{
    loop {
        let pause = match sync_once().await {
            Ok(summary) => {
                if !listener.on_summary(summary) {
                    break;
                }
                settings.idle_pause()
            }
            // Terminal: the refresh token is dead. Stop the loop (retrying just
            // re-401s → re-403s forever) and signal a re-login. Step 39.
            Err(KupferdrahtError::SessionExpired) => {
                listener.on_session_expired();
                break;
            }
            Err(error) => {
                let pause = error_pause(&settings, &error);
                if !listener.on_error(error.to_string()) {
                    break;
                }
                pause
            }
        };
        if cancelled.load(Ordering::SeqCst) {
            break;
        }
        // Wait out the pause, but wake instantly on cancel.
        tokio::select! {
            () = notify.notified() => break,
            () = tokio::time::sleep(pause) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicU32;
    use std::time::Duration;

    use super::*;

    struct ScriptedListener {
        on_summary_returns: bool,
        on_error_returns: bool,
        summaries: AtomicU32,
        errors: AtomicU32,
        expirations: AtomicU32,
    }

    impl ScriptedListener {
        fn new(on_summary_returns: bool, on_error_returns: bool) -> Arc<Self> {
            Arc::new(Self {
                on_summary_returns,
                on_error_returns,
                summaries: AtomicU32::new(0),
                errors: AtomicU32::new(0),
                expirations: AtomicU32::new(0),
            })
        }
    }

    impl SyncListener for ScriptedListener {
        fn on_summary(&self, _summary: SyncSummary) -> bool {
            self.summaries.fetch_add(1, Ordering::SeqCst);
            self.on_summary_returns
        }
        fn on_error(&self, _description: String) -> bool {
            self.errors.fetch_add(1, Ordering::SeqCst);
            self.on_error_returns
        }
        fn on_session_expired(&self) {
            self.expirations.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn counting_step(
        calls: Arc<AtomicU32>,
        results: impl Fn(u32) -> Result<SyncSummary, KupferdrahtError> + Send + 'static,
    ) -> impl FnMut() -> std::future::Ready<Result<SyncSummary, KupferdrahtError>> + Send {
        move || {
            let n = calls.fetch_add(1, Ordering::SeqCst);
            std::future::ready(results(n))
        }
    }

    #[tokio::test(start_paused = true)]
    async fn sync_loop_retries_after_error_when_listener_returns_true() {
        let calls = Arc::new(AtomicU32::new(0));
        // First iteration fails, second succeeds; the listener keeps
        // the loop alive through the error and stops after the Ok.
        let step = counting_step(calls.clone(), |n| {
            if n == 0 {
                Err(KupferdrahtError::Network("offline".into()))
            } else {
                Ok(SyncSummary::default())
            }
        });
        let listener = ScriptedListener::new(false, true);
        run_sync_loop(
            step,
            SyncSettings::default(),
            listener.clone(),
            Arc::new(AtomicBool::new(false)),
            Arc::new(Notify::new()),
        )
        .await;

        assert_eq!(calls.load(Ordering::SeqCst), 2, "error must retry once");
        assert_eq!(listener.errors.load(Ordering::SeqCst), 1);
        assert_eq!(listener.summaries.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn sync_loop_stops_when_on_error_returns_false() {
        let calls = Arc::new(AtomicU32::new(0));
        let step = counting_step(calls.clone(), |_| {
            Err(KupferdrahtError::Network("offline".into()))
        });
        let listener = ScriptedListener::new(true, false);
        run_sync_loop(
            step,
            SyncSettings::default(),
            listener.clone(),
            Arc::new(AtomicBool::new(false)),
            Arc::new(Notify::new()),
        )
        .await;

        assert_eq!(calls.load(Ordering::SeqCst), 1, "false must stop the loop");
        assert_eq!(listener.errors.load(Ordering::SeqCst), 1);
        assert_eq!(listener.summaries.load(Ordering::SeqCst), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn sync_loop_stops_and_signals_on_session_expired() {
        // A terminal SessionExpired stops the loop immediately and fires
        // on_session_expired exactly once — never on_error, never a retry,
        // even though the listener would keep ordinary errors looping. Step 39.
        let calls = Arc::new(AtomicU32::new(0));
        let step = counting_step(calls.clone(), |_| Err(KupferdrahtError::SessionExpired));
        let listener = ScriptedListener::new(true, true);
        run_sync_loop(
            step,
            SyncSettings::default(),
            listener.clone(),
            Arc::new(AtomicBool::new(false)),
            Arc::new(Notify::new()),
        )
        .await;

        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "a dead session must not be retried"
        );
        assert_eq!(listener.expirations.load(Ordering::SeqCst), 1);
        assert_eq!(
            listener.errors.load(Ordering::SeqCst),
            0,
            "session expiry is signalled via on_session_expired, not on_error"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn sync_loop_error_backoff_is_cancellable() {
        let calls = Arc::new(AtomicU32::new(0));
        let step = counting_step(calls.clone(), |_| {
            Err(KupferdrahtError::Network("offline".into()))
        });
        let listener = ScriptedListener::new(true, true);
        let cancelled = Arc::new(AtomicBool::new(false));
        let notify = Arc::new(Notify::new());
        let handle = tokio::spawn(run_sync_loop(
            step,
            SyncSettings::default(),
            listener,
            cancelled.clone(),
            notify.clone(),
        ));
        // Let the loop reach its error backoff, then cancel into it.
        tokio::task::yield_now().await;
        cancelled.store(true, Ordering::SeqCst);
        notify.notify_one();
        handle.await.expect("loop task");

        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "cancel during backoff must prevent a second iteration"
        );
    }

    /// Step 95c: a panic inside the sync step (a defect, or foreign data
    /// that still reaches one) used to kill the spawned loop task
    /// silently — the consumer saw sync stop with no signal. It reaches
    /// `on_error` now, and the loop stops: after a panic the client's
    /// state is suspect, so it is not retried even when the listener
    /// asks for a retry.
    #[tokio::test(start_paused = true)]
    async fn a_panicking_sync_step_reports_on_error_and_stops() {
        let calls = Arc::new(AtomicU32::new(0));
        let step_calls = calls.clone();
        let step = move || {
            let n = step_calls.fetch_add(1, Ordering::SeqCst);
            async move {
                assert!(n > 0, "hostile page");
                Ok(SyncSummary::default())
            }
        };
        let listener = ScriptedListener::new(true, true);
        report_runner_panic(
            run_sync_loop(
                step,
                SyncSettings::default(),
                listener.clone(),
                Arc::new(AtomicBool::new(false)),
                Arc::new(Notify::new()),
            ),
            listener.clone(),
        )
        .await;

        assert_eq!(calls.load(Ordering::SeqCst), 1, "a panic is not retried");
        assert_eq!(listener.errors.load(Ordering::SeqCst), 1);
        assert_eq!(listener.summaries.load(Ordering::SeqCst), 0);
    }

    /// Run the loop over `results` with default settings; returns the
    /// (paused-clock) time it took.
    async fn timed_loop(
        results: impl Fn(u32) -> Result<SyncSummary, KupferdrahtError> + Send + 'static,
        listener: Arc<ScriptedListener>,
    ) -> Duration {
        let start = tokio::time::Instant::now();
        run_sync_loop(
            counting_step(Arc::new(AtomicU32::new(0)), results),
            SyncSettings::default(),
            listener,
            Arc::new(AtomicBool::new(false)),
            Arc::new(Notify::new()),
        )
        .await;
        start.elapsed()
    }

    #[tokio::test(start_paused = true)]
    async fn an_ok_iteration_pauses_ten_seconds_by_default() {
        // Ok, then an error the listener stops on: exactly one idle pause.
        let elapsed = timed_loop(
            |n| {
                if n == 0 {
                    Ok(SyncSummary::default())
                } else {
                    Err(KupferdrahtError::Network("stop".into()))
                }
            },
            ScriptedListener::new(true, false),
        )
        .await;
        // Step 118: one value without a socket on every target.
        assert_eq!(elapsed, Duration::from_secs(10));
    }

    #[tokio::test(start_paused = true)]
    async fn a_rate_limited_iteration_waits_its_retry_after() {
        let elapsed = timed_loop(
            |n| {
                if n == 0 {
                    Err(KupferdrahtError::RateLimited {
                        retry_after_secs: Some(7),
                    })
                } else {
                    Ok(SyncSummary::default())
                }
            },
            ScriptedListener::new(false, true),
        )
        .await;
        assert_eq!(elapsed, Duration::from_secs(7));
    }

    #[tokio::test(start_paused = true)]
    async fn a_rate_limit_wait_is_capped_at_a_minute() {
        let elapsed = timed_loop(
            |n| {
                if n == 0 {
                    Err(KupferdrahtError::RateLimited {
                        retry_after_secs: Some(3600),
                    })
                } else {
                    Ok(SyncSummary::default())
                }
            },
            ScriptedListener::new(false, true),
        )
        .await;
        assert_eq!(elapsed, Duration::from_secs(60));
    }

    #[test]
    fn error_backoff_prefers_poll_interval() {
        let settings = SyncSettings {
            poll_interval: Some(Duration::from_millis(250)),
            ..SyncSettings::default()
        };
        assert_eq!(error_backoff(&settings), Duration::from_millis(250));
    }

    #[test]
    fn error_backoff_defaults_to_two_seconds() {
        assert_eq!(
            error_backoff(&SyncSettings::default()),
            Duration::from_secs(2)
        );
    }
}
