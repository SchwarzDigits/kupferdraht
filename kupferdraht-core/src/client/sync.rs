// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `Client` — sync orchestration + rooms-cache persistence.
//!
//! Split out of `client/runtime.rs` (Step 29): the
//! matrix-rust-sdk-shaped `sync*` family (`sync`, `sync_once`,
//! `sync_with_callback`, `sync_stream`), the conversation-metadata
//! refresh (`sync_metadata`, `collect_conversation_ids`,
//! `backfill_user_profiles`), the `SqliteStore` rooms-cache
//! round-trip (`persist_rooms`, `load_rooms_from_store`), event-handler
//! registration, and the inbound external-removal commit driver.
//! Descends from [`super`] where the `Client` struct is defined, so the
//! moved `impl Client` block reaches its private fields.

#![allow(clippy::doc_markdown)]

use std::collections::HashMap;
use std::sync::Arc;

use crate::encryption::ExternalRemovalOutcome;
use crate::error::KupferdrahtError;
use crate::rooms::{
    Room, apply_self_profile, apply_user_profiles, non_self_qualified_ids, room_from_conversation,
    room_to_records, rooms_from_records,
};
use crate::storage::{RoomMemberRecord, RoomRecord};
use crate::time::now_ms;
use crate::wire::dto::{QualifiedId, UserProfile};

use super::Client;

/// One polling iteration: `Ok(None)` after a `429` that was waited out
/// (the caller loops again), every other error passes through. Step 103.
async fn sync_once_waiting_out_rate_limits(
    client: &Client,
    settings: &crate::sync::SyncSettings,
) -> Result<Option<crate::sync::SyncSummary>, KupferdrahtError> {
    match client.sync_once(settings.clone()).await {
        Ok(summary) => Ok(Some(summary)),
        Err(KupferdrahtError::RateLimited { retry_after_secs }) => {
            wait_out_rate_limit(settings, retry_after_secs).await;
            Ok(None)
        }
        Err(e) => Err(e),
    }
}

/// Sleep the rate-limit pause after a `429`. Step 103.
async fn wait_out_rate_limit(settings: &crate::sync::SyncSettings, retry_after_secs: Option<u64>) {
    let pause = settings.rate_limit_pause(retry_after_secs);
    tracing::warn!(?pause, "sync: rate-limited by the backend; waiting");
    crate::time::sleep(pause).await;
}

impl Client {
    /// Step 14b — drive [`Self::sync_once`] in a loop, sleeping
    /// `settings.poll_interval` (5 s when unset) between consecutive
    /// empty drains; a `429` is waited out (Step 103).
    /// Returns only on a hard error from the iteration; the caller
    /// drives termination by dropping the awaiting task (e.g.
    /// racing against `tokio::signal::ctrl_c`).
    ///
    /// This is the matrix-rust-sdk-shaped sync entry point.
    /// Replaces the Step-5d `Client::sync().await?.run_polling(...)`
    /// pattern: callers that previously held an `Arc<SyncService>`
    /// migrate to `client.sync(SyncSettings { poll_interval: ... })`.
    ///
    /// # Errors
    /// First transport / storage failure aborts the loop with the
    /// originating error variant; per-event dispatch failures fold
    /// into `EventHandler::on_sync_error` and the loop continues.
    /// `KupferdrahtError::SyncAlreadyRunning` if another `sync*` call is
    /// already driving the same `Client`'s cursor.
    pub async fn sync(&self, settings: crate::sync::SyncSettings) -> Result<(), KupferdrahtError> {
        // Drop any typing indicators left over from a prior sync session
        // before this one begins (Step 21 orphan safety net).
        self.reset_typing_on_sync_start();
        if settings.transport == crate::sync::SyncTransport::WebSocket {
            return self
                .sync_push(settings, |_| std::ops::ControlFlow::Continue(()))
                .await;
        }
        loop {
            let Some(summary) = sync_once_waiting_out_rate_limits(self, &settings).await? else {
                continue;
            };
            if summary.events_dispatched == 0 {
                // `crate::time::sleep` bridges to the browser timer on wasm
                // (tokio has no timer driver there — it would hang); native maps
                // straight to `tokio::time::sleep` (W4).
                crate::time::sleep(settings.idle_pause()).await;
            }
        }
    }

    /// Register an [`crate::sync::EventHandler`] to receive
    /// per-event callbacks once a `sync*` iteration is running.
    /// Mirrors `matrix_sdk::Client::add_event_handler`.
    ///
    /// Handlers added before `sync()` are honored on every future
    /// service; handlers added after are picked up by the next
    /// frame the running service dispatches (the underlying
    /// snapshot is reloaded per frame).
    ///
    /// # Panics
    /// Panics if the internal handler-registry mutex has been
    /// poisoned by a previous handler-registration panic. Practically
    /// not reachable — the lock scope is two lines and never panics.
    pub fn add_event_handler(&self, handler: Arc<dyn crate::sync::EventHandler>) {
        self.event_handlers
            .lock()
            .expect("event_handlers mutex poisoned")
            .push(handler);
    }

    /// Fetch the user's conversation list from Wire and cache it in
    /// memory. Mirrors `matrix_sdk::Client::sync_once` (one-shot;
    /// the event-stream-based live sync is Step 5).
    ///
    /// Pages through `POST /v14/conversations/list-ids` until
    /// `has_more` is `false`, then bulk-fetches metadata via
    /// `POST /v14/conversations/list`. `proteus`-only conversations
    /// are filtered out at the `wire::dto` → `Room` boundary
    /// (ADR-0001: Kupferdraht is MLS-only). Cycle 2d wires this cache
    /// through `SqliteStore` so a process restart can skip the
    /// round trip; cycle 2c backfills member display names.
    ///
    /// # Errors
    /// Transport / backend errors from either endpoint bubble up as
    /// `KupferdrahtError::Network` / `BackendRejected` / `Encoding`.
    ///
    /// # Panics
    /// Panics if the in-memory rooms-cache `RwLock` is poisoned —
    /// only possible if another task panicked while holding the
    /// write guard, which is itself a bug.
    pub async fn sync_metadata(&self) -> Result<(), KupferdrahtError> {
        let all_ids = self.collect_conversation_ids().await?;
        let conversations = self.list_conversations_chunked(&all_ids).await?;
        let rooms = if conversations.is_empty() {
            Vec::new()
        } else {
            let weak = self.weak();
            let raw: Vec<Room> = conversations
                .into_iter()
                .filter_map(|c| room_from_conversation(c, weak.clone()))
                .collect();
            self.backfill_user_profiles(raw).await?
        };
        self.persist_rooms(&rooms).await?;
        *self.rooms.write().expect("rooms lock poisoned") =
            rooms.into_iter().map(Arc::new).collect();
        Ok(())
    }

    /// Dispatch a transient (live-only) notification's events, without touching
    /// the cursor (ADR-0030 d.4).
    ///
    /// Step 115b: the browser push socket's typing path. Typing can only arrive
    /// on a socket — a REST drain never returns it — so unlike a durable frame,
    /// a transient one has to be dispatched rather than merely ring.
    ///
    /// # Errors
    /// Whatever dispatching the notification's events returns.
    #[cfg(target_arch = "wasm32")]
    pub(crate) async fn dispatch_transient(
        self: &std::sync::Arc<Self>,
        notif: &crate::wire::notification_dto::QueuedNotification,
    ) -> Result<u32, KupferdrahtError> {
        let service = crate::sync::SyncService::new(self.weak(), self.event_handlers.clone());
        service.dispatch_transient_notification(self, notif).await
    }

    /// Step 14b — one iteration of the matrix-rust-sdk-shaped sync
    /// surface. Composes a conversations-metadata refresh + one
    /// `/v14/notifications` drain + cursor advance, dispatching every
    /// queued event through the registered `EventHandler`s. Since
    /// Step 118 the metadata refresh runs only when the room list is
    /// stale — at start, and after a drain that handed over an event
    /// changing it (`client/room_freshness.rs`); an idle pass is the
    /// drain alone. Returns
    /// a [`SyncSummary`] aggregating the counts (rooms touched,
    /// events dispatched).
    ///
    /// `SyncSettings::poll_interval` is **ignored** here — one-shot
    /// iterations never sleep. The loop variants (`sync`,
    /// `sync_with_callback`, `sync_stream`) honour it between
    /// iterations.
    ///
    /// # Errors
    /// Same error surface as today's `sync_metadata` +
    /// `SyncService::drain_pending` combined: transport,
    /// backend-rejected, encoding, storage. Per-event dispatch
    /// failures fold into `EventHandler::on_sync_error` and the
    /// iteration continues.
    ///
    /// # Panics
    /// Panics if the in-memory rooms-cache `RwLock` is poisoned.
    pub async fn sync_once(
        &self,
        _settings: crate::sync::SyncSettings,
    ) -> Result<crate::sync::SyncSummary, KupferdrahtError> {
        // At-most-one-iteration guard. `try_lock` returns `Err` when
        // another `sync*` call already holds it; `sync_once` cannot
        // queue because callers may race two concurrent iterations
        // expecting parallelism (the `concurrent_sync_returns_…`
        // test pins the contract).
        let _guard = self
            .sync_lock
            .try_lock()
            .map_err(|_| KupferdrahtError::SyncAlreadyRunning)?;

        // Step 63: throttled key-package pool self-check (at most one
        // attempt per 24 h). Runs inside the sync guard, but its
        // outcome never affects the iteration — warn-and-continue.
        self.maybe_replenish_key_packages().await;

        // Phase 1: the conversations-metadata refresh, when the room list
        // is stale (Step 118).
        self.refresh_rooms_if_stale().await?;
        let rooms_updated = u32::try_from(self.rooms.read().expect("rooms lock poisoned").len())
            .unwrap_or(u32::MAX);
        // Step 95d: every room gets its device baseline before the drain
        // can change its tree — every pass, a local read per room.
        self.ensure_device_baselines().await;

        // Phase 2: notifications drain. The internal `SyncService`
        // owns the per-event MLS-decrypt + EventHandler dispatch; its
        // dispatch cycle refreshes a room list the drain marked stale.
        let service = crate::sync::SyncService::new(self.weak(), self.event_handlers.clone());
        let events_dispatched = service.drain_pending().await?;

        Ok(crate::sync::SyncSummary {
            rooms_updated,
            events_dispatched,
        })
    }

    // NOTE: `pub async fn sync(&self, settings: SyncSettings) ->
    // Result<()>` is intentionally NOT added in the red-phase — it
    // collides with the existing `Client::sync() -> Arc<SyncService>`
    // factory. The new loop method REPLACES the old factory in the
    // green-phase commit, atomically with CLI / integration-test
    // call-site migration. Red-phase tests cover the other three
    // new methods (`sync_once`, `sync_with_callback`,
    // `sync_stream`); the loop-semantics behaviour they assert is
    // adequately covered by `sync_with_callback`.

    /// Step 14b — loop variant with a per-iteration callback. The
    /// callback receives the [`SyncSummary`] of each iteration and
    /// returns `ControlFlow::Continue(())` to keep looping or
    /// `ControlFlow::Break(())` to exit. UDL-friendly: UniFFI
    /// exports `Fn` closures but not `impl Stream`, so this is the
    /// binding-friendly entry point for the loop family.
    ///
    /// # Errors
    /// Same surface as [`Self::sync`].
    pub async fn sync_with_callback<F>(
        &self,
        settings: crate::sync::SyncSettings,
        callback: F,
    ) -> Result<(), KupferdrahtError>
    where
        F: Fn(crate::sync::SyncSummary) -> std::ops::ControlFlow<()> + Send + 'static,
    {
        if settings.transport == crate::sync::SyncTransport::WebSocket {
            return self.sync_push(settings, callback).await;
        }
        loop {
            let Some(summary) = sync_once_waiting_out_rate_limits(self, &settings).await? else {
                continue;
            };
            match callback(summary) {
                std::ops::ControlFlow::Break(()) => return Ok(()),
                std::ops::ControlFlow::Continue(()) => {}
            }
            if summary.events_dispatched == 0 {
                crate::time::sleep(settings.idle_pause()).await;
            }
        }
    }

    /// Step 14b — loop variant surfaced as an async `Stream` of
    /// per-iteration [`SyncSummary`]s. Dropping the returned stream
    /// cancels the underlying polling task. Rust-side ergonomic;
    /// UDL surface deferred (UniFFI cannot expose `impl Stream` —
    /// callers from foreign code use [`Self::sync_with_callback`]
    /// instead).
    ///
    /// # Errors
    /// First failure surfaces as an `Err` item on the stream; the
    /// stream terminates after yielding the error.
    /// `KupferdrahtError::SyncAlreadyRunning` arrives as the first stream
    /// item when another `sync*` call is already in flight.
    // wasm ignores `settings` — its arm is a stub until W4's `spawn_local` loop.
    #[cfg_attr(target_arch = "wasm32", allow(clippy::needless_pass_by_value))]
    pub fn sync_stream(
        &self,
        settings: crate::sync::SyncSettings,
    ) -> futures_util::stream::BoxStream<'static, Result<crate::sync::SyncSummary, KupferdrahtError>>
    {
        use futures_util::StreamExt as _;
        // wasm: the background sync loop uses `tokio::spawn` (a `Send` future) and
        // a multi-thread runtime, neither of which exists in the browser. The
        // real wasm sync (single-thread `spawn_local`) is a later step; for now
        // the stream yields a single error so callers don't hang. See docs/plan/44-*.
        #[cfg(target_arch = "wasm32")]
        {
            let _ = settings;
            futures_util::stream::once(async {
                Err(KupferdrahtError::Generic(
                    "background sync is not available on wasm yet".into(),
                ))
            })
            .boxed()
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            if settings.transport == crate::sync::SyncTransport::WebSocket {
                let weak = self.weak();
                let (tx, rx) = tokio::sync::mpsc::channel::<
                    Result<crate::sync::SyncSummary, KupferdrahtError>,
                >(4);
                tokio::spawn(async move {
                    let Some(client) = weak.upgrade() else {
                        return;
                    };
                    let frames_tx = tx.clone();
                    let result = client
                        .sync_push(settings, move |summary| {
                            // A dropped receiver is the cancellation
                            // signal — same contract as the polling arm
                            // below. A full buffer drops the tick, never
                            // the loop.
                            match frames_tx.try_send(Ok(summary)) {
                                Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                                    std::ops::ControlFlow::Break(())
                                }
                                _ => std::ops::ControlFlow::Continue(()),
                            }
                        })
                        .await;
                    if let Err(e) = result {
                        let _ = tx.send(Err(e)).await;
                    }
                });
                return tokio_stream::wrappers::ReceiverStream::new(rx).boxed();
            }
            let weak = self.weak();
            let (tx, rx) =
                tokio::sync::mpsc::channel::<Result<crate::sync::SyncSummary, KupferdrahtError>>(4);
            tokio::spawn(async move {
                loop {
                    let Some(client) = weak.upgrade() else {
                        return;
                    };
                    let summary_result = client.sync_once(settings.clone()).await;
                    if let Err(KupferdrahtError::RateLimited { retry_after_secs }) = summary_result
                    {
                        drop(client);
                        wait_out_rate_limit(&settings, retry_after_secs).await;
                        continue;
                    }
                    let is_err = summary_result.is_err();
                    let zero_events = matches!(
                        summary_result,
                        Ok(crate::sync::SyncSummary {
                            events_dispatched: 0,
                            ..
                        }),
                    );
                    // `send` returns Err when every receiver has been
                    // dropped — that's our cancellation signal.
                    if tx.send(summary_result).await.is_err() {
                        return;
                    }
                    if is_err {
                        return;
                    }
                    if zero_events {
                        crate::time::sleep(settings.idle_pause()).await;
                    }
                }
            });
            tokio_stream::wrappers::ReceiverStream::new(rx).boxed()
        }
    }

    /// Steps 62b/73 — the long-lived push loop behind
    /// [`SyncTransport::WebSocket`](crate::sync::SyncTransport): one
    /// conversations-metadata refresh per connection, then the
    /// ADR-0019 negotiation picks the socket dialect —
    /// `/v14/events` (frames acked after the cursor persist) when
    /// the server enables ConsumableNotifications, the legacy
    /// `/await` route otherwise, whose frames only trigger a REST
    /// drain (ADR-0030 — no acks, only the drain moves the cursor).
    /// Either way every event runs through the same MLS-decrypt +
    /// `EventHandler` pipeline as the polling drain. `on_frame`
    /// fires once per dispatched `/v14/events` frame (cursor
    /// persisted first), or on `/await` once per drain or transient
    /// frame;
    /// `ControlFlow::Break` closes the socket and returns `Ok(())`.
    /// Orderly closes and transport errors reconnect with a capped
    /// backoff paced by `poll_interval`; hard errors (storage,
    /// missing device) abort with `Err`.
    // On wasm the body is the "no push transport yet" stub (no await) —
    // the signature is shared with native, hence the targeted allow.
    #[cfg_attr(
        target_arch = "wasm32",
        allow(clippy::unused_async, clippy::unused_async_trait_impl)
    )]
    async fn sync_push<F>(
        &self,
        settings: crate::sync::SyncSettings,
        on_frame: F,
    ) -> Result<(), KupferdrahtError>
    where
        F: FnMut(crate::sync::SyncSummary) -> std::ops::ControlFlow<()> + Send,
    {
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (settings, on_frame);
            Err(KupferdrahtError::Generic(
                "websocket transport is not available on wasm yet".into(),
            ))
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            // Same at-most-one-sync contract as the polling loop, but
            // held for the whole push session (the polling path
            // re-acquires per sync_once iteration).
            let _guard = self
                .sync_lock
                .try_lock()
                .map_err(|_| KupferdrahtError::SyncAlreadyRunning)?;
            // Step 63 parity: the polling loops hit the throttled
            // key-package check via sync_once, which a push session
            // bypasses — run it here instead (once per session, same
            // 24-h stamp, warn-and-continue).
            self.maybe_replenish_key_packages().await;
            let client = self.weak().upgrade().ok_or_else(|| {
                KupferdrahtError::InvalidInput("sync_push: Client dropped".into())
            })?;
            let service = crate::sync::SyncService::new(self.weak(), self.event_handlers.clone());
            crate::sync::push::run_push(&client, &service, &settings, on_frame).await
        }
    }

    /// Write the freshly-synced rooms to the attached `SqliteStore`
    /// (if any). `last_synced_at_ms` is stamped at the moment of
    /// persistence; a read-back via
    /// [`Self::load_rooms_from_store`] uses this to order rooms
    /// most-recently-synced first.
    async fn persist_rooms(&self, rooms: &[Room]) -> Result<(), KupferdrahtError> {
        let Some(store) = self.store.as_ref() else {
            return Ok(());
        };
        let now = now_ms();
        let records: Vec<(RoomRecord, Vec<RoomMemberRecord>)> =
            rooms.iter().map(|r| room_to_records(r, now)).collect();
        store.replace_rooms(&records).await
    }

    /// Hydrate the in-memory rooms cache from the attached
    /// `SqliteStore` without hitting Wire. Useful at process start
    /// on a pre-synced account (the cache would otherwise be empty
    /// until `sync_metadata` completes).
    ///
    /// Rows whose `room_type` column carries an unknown variant are
    /// dropped silently (a forward-compatibility nod to later
    /// `RoomType` additions).
    ///
    /// # Errors
    /// Returns `KupferdrahtError::Storage` on `SQLite` failure or when
    /// no store is attached (`InvalidInput`).
    ///
    /// # Panics
    /// Panics if the in-memory rooms-cache `RwLock` is poisoned.
    pub async fn load_rooms_from_store(&self) -> Result<(), KupferdrahtError> {
        let store = self.store.as_ref().ok_or_else(|| {
            KupferdrahtError::InvalidInput(
                "Client::load_rooms_from_store called without ClientBuilder::base_dir".into(),
            )
        })?;
        let records = store.load_rooms().await?;
        let rooms = rooms_from_records(records, &self.weak());
        *self.rooms.write().expect("rooms lock poisoned") =
            rooms.into_iter().map(Arc::new).collect();
        // Step 60c: one-time backfill of the materialized task cache
        // after the V23 migration — a populated cache short-circuits
        // via a single COUNT query.
        crate::timeline::task::backfill_task_cache(self).await?;
        // Step 89: sweep ephemeral rows that expired while the process
        // was down and re-arm the timers of the ones still pending —
        // before any consumer can read a body that should be gone.
        self.restore_ephemeral_timers().await?;
        Ok(())
    }

    /// Resolve `display_name` / `handle` on every non-self member
    /// of `rooms` through `POST /v14/list-users`. A failed lookup
    /// (Wire's `failed` bucket) keeps the member's `display_name`
    /// as `None` rather than erroring the whole sync — the backfill
    /// is best-effort, names are UX polish, not correctness.
    async fn backfill_user_profiles(
        &self,
        rooms: Vec<Room>,
    ) -> Result<Vec<Room>, KupferdrahtError> {
        let ids = non_self_qualified_ids(&rooms);
        let lookup: HashMap<String, UserProfile> = if ids.is_empty() {
            HashMap::new()
        } else {
            let resp = self
                .http
                .list_users(self.api_version, &self.current_access_token(), &ids)
                .await?;
            resp.found
                .into_iter()
                .map(|u| (u.qualified_id.to_string(), u))
                .collect()
        };
        // Self's name / handle come from `/v14/self` (already in
        // the account snapshot). `list-users` deliberately excludes
        // self, so we apply the account profile here.
        let self_name = self.account.display_name().to_string();
        let self_handle = self.account.handle().map(str::to_string);
        Ok(rooms
            .into_iter()
            .map(|r| apply_user_profiles(r, &lookup))
            .map(|r| apply_self_profile(r, &self_name, self_handle.as_deref()))
            .collect())
    }

    /// Fetch the metadata of `ids` in requests of at most Wire's cap
    /// (Step 95c: an account in more than 1000 conversations used to
    /// send one oversized request, rejected on every cycle).
    async fn list_conversations_chunked(
        &self,
        ids: &[QualifiedId],
    ) -> Result<Vec<crate::wire::dto::OwnConversation>, KupferdrahtError> {
        let mut found = Vec::with_capacity(ids.len());
        for chunk in ids.chunks(crate::wire::HttpClient::LIST_CONVERSATIONS_MAX_IDS) {
            let resp = self
                .http
                .list_conversations(self.api_version, &self.current_access_token(), chunk)
                .await?;
            found.extend(resp.found);
        }
        Ok(found)
    }

    async fn collect_conversation_ids(&self) -> Result<Vec<QualifiedId>, KupferdrahtError> {
        let mut all_ids: Vec<QualifiedId> = Vec::new();
        let mut paging_state: Option<String> = None;
        loop {
            let page = self
                .http
                .list_conversation_ids(
                    self.api_version,
                    &self.current_access_token(),
                    paging_state.as_deref(),
                    None,
                )
                .await?;
            all_ids.extend(page.qualified_conversations);
            if !page.has_more {
                break;
            }
            paging_state = Some(page.paging_state);
        }
        Ok(all_ids)
    }

    /// Step 74: record `room_id` for the end-of-cycle removal flush.
    /// The dispatcher no longer commits inline on `ProposalQueued` —
    /// per-proposal commits storm a backlog with
    /// `mls-commit-missing-references` rejections (observations §70).
    pub(crate) fn queue_removal_flush(&self, room_id: &str) {
        debug_assert!(
            !room_id.is_empty(),
            "queue_removal_flush: room_id must be non-empty"
        );
        self.pending_removal_flushes
            .lock()
            .expect("pending_removal_flushes lock")
            .insert(room_id.to_string());
    }

    /// Snapshot of the rooms awaiting a removal flush (Step 74). An
    /// entry leaves the set only via [`Client::ack_removal_flush`]
    /// after its commit succeeded — failed flushes stay queued and
    /// retry on the next dispatch cycle.
    pub(crate) fn pending_removal_flushes(&self) -> Vec<String> {
        self.pending_removal_flushes
            .lock()
            .expect("pending_removal_flushes lock")
            .iter()
            .cloned()
            .collect()
    }

    /// Remove `room_id` from the removal-flush queue (Step 74) —
    /// called only after `commit_pending_external_removal` resolved
    /// it (committed, self-discarded, or nothing pending).
    pub(crate) fn ack_removal_flush(&self, room_id: &str) {
        self.pending_removal_flushes
            .lock()
            .expect("pending_removal_flushes lock")
            .remove(room_id);
    }

    /// Drive the commit of the queued external-sender Remove
    /// proposals for `room_id` (Step 28; batched Step 74). Called by
    /// `SyncService::flush_removal_commits` at the end of a dispatch
    /// cycle — never inline per proposal:
    /// - peer remove(s) → one commit covering every queued proposal,
    ///   upload the bundle, merge;
    /// - self remove → discard the local group + drop the room;
    /// - nothing pending → no-op.
    ///
    /// # Errors
    /// Storage / MLS / upload errors propagate (folded into
    /// `on_sync_error` by the caller).
    pub(crate) async fn commit_pending_external_removal(
        &self,
        room_id: &str,
    ) -> Result<(), KupferdrahtError> {
        let group_id = self
            .resolve_room_group_id(room_id, "commit_pending_external_removal")
            .await?;
        let mls = self.mls_client_required("commit_pending_external_removal")?;
        // Step 97d (ADR-0031): the whole stage → upload → merge under the
        // group's commit-flow lock, like every own commit.
        let _flow = self.lock_commit_flow(&group_id).await;
        let outcome = {
            let _gate = self.mls_write_gate().await;
            let guard = crate::encryption::lock_mls(mls);
            guard.commit_external_removals(&group_id)?
        };
        match outcome {
            // Peer removed: upload the staged commit, merge on success.
            // The cached room's roster reconciles on the next
            // sync_metadata — not forced here, to avoid re-entering sync
            // from within the dispatch loop.
            ExternalRemovalOutcome::Committed(bundle) => {
                tracing::info!(
                    room = room_id,
                    "Step 28: committing external-sender Remove proposal (peer removed)"
                );
                let committed = self
                    .upload_and_merge_commit_bundle(
                        mls,
                        &group_id,
                        &bundle,
                        "commit_pending_external_removal",
                    )
                    .await;
                if committed.is_ok() {
                    self.note_room_devices(room_id).await;
                }
                committed
            }
            // We are the one being removed: discard the local group and
            // drop the room from cache (a remaining member commits the
            // removal for the rest of the group).
            ExternalRemovalOutcome::SelfRemoved => {
                tracing::info!(
                    room = room_id,
                    "Step 28: external-sender Remove targets self; discarding local group"
                );
                {
                    let _gate = self.mls_write_gate().await;
                    let _ = crate::encryption::lock_mls(mls).discard_group(&group_id);
                }
                self.remove_cached_room(room_id);
                Ok(())
            }
            ExternalRemovalOutcome::Nothing => Ok(()),
        }
    }
}
