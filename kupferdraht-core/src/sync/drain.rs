// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! The REST drain (Steps 5 / 14b / 64; restructured in Step 97d,
//! ADR-0031): one cursor-advancing dispatch per account at a time.
//!
//! Per page: fetch `GET /v14/notifications?since=` **without** a lock →
//! take the account's drain lock (`Client::lock_drain`) → compare the
//! persisted cursor with the page's `since` (another `Client` or process
//! moved it → release and refetch) → dispatch and advance → release.
//! Network follow-ups (healing, receipts, config refetches, the removal
//! flush) run in `end_dispatch_cycle`, after the lock.
//!
//! A notification whose MLS message awaits an own pending commit stops
//! the page before it (the cursor stays): this process's commit flow in
//! flight → wait for it and drain again; none → stop the drain until a
//! later one; a pending commit that held the drain back for
//! [`ABANDON_PENDING_COMMIT_AFTER_MS`] → clear it and drain again (the
//! message then heals as before). The clock belongs to the commit: it
//! is keyed by the group epoch the commit was staged at, so a merged
//! commit's clock never counts against the next one in the same room.

use std::sync::Arc;

use tracing::{debug, warn};

use crate::client::Client;
use crate::error::KupferdrahtError;
use crate::sync::page::PageStop;
use crate::sync::service::SyncService;
use crate::wire::http::notifications::FetchedNotifications;
use crate::wire::notification_dto::{NotificationListResponse, QueuedNotification};

/// How long a pending commit may hold the drain back before it counts
/// as abandoned (its flow crashed, or the process holding it died).
const ABANDON_PENDING_COMMIT_AFTER_MS: i64 = 60_000;

/// What dispatching one fetched page came to.
enum PageOutcome {
    /// Every notification dispatched; the count of events.
    Done(u32),
    /// Another `Client` or process advanced the cursor after the fetch.
    CursorMoved(Option<String>),
    /// Stopped before a notification awaiting an own pending commit.
    Deferred { dispatched: u32, room_id: String },
}

/// What a deferred notification means for the drain (pure).
#[derive(Debug, PartialEq, Eq)]
enum DeferralStep {
    /// This process's commit flow is in flight: wait, then drain again.
    WaitForFlow,
    /// Stop the drain before the notification; a later drain retries.
    StopDrain,
    /// The pending commit held the drain back too long: clear it.
    Abandon,
}

/// The abandonment clock of one room's deferral: the group epoch its
/// pending commit was staged at, and when the drain first stopped
/// before a message awaiting it (ms).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct DeferralClock {
    epoch: u64,
    since_ms: i64,
}

/// The clock a deferral counts against (pure). A known clock of the
/// same epoch keeps its start; a different epoch means the commit it
/// timed was merged and this deferral waits on a newer one — the clock
/// starts over at `now_ms`, so a merged commit never counts against
/// the one that follows it in the same room.
fn deferral_clock(known: Option<DeferralClock>, epoch: u64, now_ms: i64) -> DeferralClock {
    match known {
        Some(clock) if clock.epoch == epoch => clock,
        _ => DeferralClock {
            epoch,
            since_ms: now_ms,
        },
    }
}

/// Decide a deferral from whether a flow of this process is in flight
/// and how long the room has been deferred.
fn deferral_step(flow_in_flight: bool, deferred_for_ms: i64) -> DeferralStep {
    if flow_in_flight {
        DeferralStep::WaitForFlow
    } else if deferred_for_ms >= ABANDON_PENDING_COMMIT_AFTER_MS {
        DeferralStep::Abandon
    } else {
        DeferralStep::StopDrain
    }
}

impl SyncService {
    /// One-shot REST drain — pulls every queued event from
    /// `GET /v14/notifications` (paged), runs each through the
    /// MLS-decrypt + `EventHandler` pipeline, and advances the
    /// persisted `last_event_id` cursor so a follow-up call only
    /// sees new events. Returns the number of notifications
    /// consumed (one notification = one Wire `payload` batch).
    ///
    /// Idempotent at the cursor level — a crash mid-page resumes
    /// from the last successfully-processed notification on the
    /// next call.
    ///
    /// # Errors
    /// * `KupferdrahtError::InvalidInput` — owning `Client` dropped or
    ///   no `device_id` on file.
    /// * `KupferdrahtError::Network` / `BackendRejected` — `/notifications`
    ///   transport / 4xx / 5xx. The backend's `404 not-found` on a
    ///   stale `since` cursor is recovered in-place instead (Steps 64,
    ///   102): the cursor resets and the drain refetches marker-less,
    ///   per backend-spec §11.4. Any other `404` is an error.
    /// * `KupferdrahtError::Encoding` — malformed response body.
    /// * `KupferdrahtError::Storage` — `sync_state` write failure.
    /// * `KupferdrahtError::AccountLocked` — another `Client` or process kept
    ///   the drain lock past its wait (retryable, Step 97d).
    /// * Per-event dispatch errors fold into `on_sync_error` and the
    ///   drain continues.
    pub async fn drain_pending(&self) -> Result<u32, KupferdrahtError> {
        let (client, device_id) = self.drain_client()?;
        let access_token = client.access_token().unwrap_or_default();
        self.begin_dispatch_cycle();
        let mut since = persisted_last_event_id(&client).await?;
        // Step 118: a cursor this client did not leave means the queue was
        // drained elsewhere — the room list may have missed an event.
        client.room_freshness().note_drain_start(since.as_deref());
        let mut total: u32 = 0;
        let mut cursor_reset_done = false;
        loop {
            let page = fetch_page_recovering_stale_cursor(
                &client,
                &access_token,
                &device_id,
                &mut since,
                &mut cursor_reset_done,
            )
            .await?;
            match self.dispatch_page(&client, &page, &mut since).await? {
                PageOutcome::Done(n) => {
                    total = total.saturating_add(n);
                    if !page.has_more {
                        break;
                    }
                }
                PageOutcome::CursorMoved(cursor) => {
                    // Step 118: what the other drain consumed, this client
                    // never sees — the room list may have missed an event.
                    client.mark_rooms_stale();
                    since = cursor;
                }
                PageOutcome::Deferred {
                    dispatched,
                    room_id,
                } => {
                    total = total.saturating_add(dispatched);
                    if !self.resolve_deferral(&client, &room_id).await {
                        break;
                    }
                    since = persisted_last_event_id(&client).await?;
                }
            }
        }
        client.room_freshness().note_drain_end(since);
        // Follow-ups, removal flush, ephemeral sweep — after the lock.
        self.end_dispatch_cycle(&client).await;
        Ok(total)
    }

    /// The owning `Client` and its device id.
    fn drain_client(&self) -> Result<(Arc<Client>, String), KupferdrahtError> {
        let client = self.client.upgrade().ok_or_else(|| {
            KupferdrahtError::InvalidInput("SyncService::drain_pending: Client dropped".into())
        })?;
        let device_id = client.device_id().map(str::to_string).ok_or_else(|| {
            KupferdrahtError::InvalidInput("drain_pending: device_id missing".into())
        })?;
        Ok((client, device_id))
    }

    /// Dispatch one fetched page under the drain lock. The page is only
    /// dispatched if the persisted cursor is still the one it was fetched
    /// with; `since` advances past the page's last committed notification.
    async fn dispatch_page(
        &self,
        client: &Arc<Client>,
        page: &NotificationListResponse,
        since: &mut Option<String>,
    ) -> Result<PageOutcome, KupferdrahtError> {
        let _lock = client.lock_drain().await?;
        if let Some(persisted) = cursor_moved(client, since.as_deref()).await? {
            debug!("drain: another client advanced the cursor; refetching");
            return Ok(PageOutcome::CursorMoved(persisted));
        }
        // Redelivery dedup: a frame whose id equals the cursor this page
        // was fetched with was already processed — skip it. Step 64.
        let notifs: Vec<&QueuedNotification> = page
            .notifications
            .iter()
            .filter(|notif| {
                let redelivered = since.as_deref() == Some(notif.id.as_str());
                if redelivered {
                    debug!(notif_id = %notif.id, "drain: redelivered cursor frame; skipping");
                }
                !redelivered
            })
            .collect();
        // One window for the page (Step 109, ADR-0037).
        let run = self.dispatch_page_of(client, &notifs).await?;
        if let Some(id) = run.last_id {
            *since = Some(id);
        }
        match run.stop {
            None => {
                self.deferrals.lock().expect("deferrals lock").clear();
                Ok(PageOutcome::Done(run.dispatched))
            }
            Some(PageStop::Deferred { room_id }) => Ok(PageOutcome::Deferred {
                dispatched: run.dispatched,
                room_id,
            }),
            Some(PageStop::Failed(e)) => Err(e),
        }
    }

    /// Act on a notification of `room_id` that awaits an own pending
    /// commit: `true` → drain again (after the flow ended, or after the
    /// abandoned commit was cleared); `false` → stop before it.
    pub(super) async fn resolve_deferral(&self, client: &Arc<Client>, room_id: &str) -> bool {
        let Some(group_id) = client.group_id_for_room(room_id).await else {
            return false;
        };
        let Some(epoch) = pending_commit_epoch(client, room_id, &group_id) else {
            return false;
        };
        let now = crate::time::now_ms();
        let clock = {
            let mut clocks = self.deferrals.lock().expect("deferrals lock");
            let clock = deferral_clock(clocks.get(room_id).copied(), epoch, now);
            clocks.insert(room_id.to_string(), clock);
            clock
        };
        match deferral_step(
            client.commit_flow_in_flight(&group_id),
            now - clock.since_ms,
        ) {
            DeferralStep::WaitForFlow => {
                client.await_commit_flow(&group_id).await;
                true
            }
            DeferralStep::StopDrain => {
                debug!(
                    room_id,
                    "drain: next-epoch message awaits an own pending commit; stopping"
                );
                false
            }
            DeferralStep::Abandon => {
                self.deferrals
                    .lock()
                    .expect("deferrals lock")
                    .remove(room_id);
                let _gate = client.mls_write_gate().await;
                clear_abandoned_commit(client, room_id, &group_id)
            }
        }
    }
}

/// The epoch of the deferred room's group — the one its pending commit
/// was staged at; `None` (logged) when the group cannot be read.
fn pending_commit_epoch(client: &Client, room_id: &str, group_id: &[u8]) -> Option<u64> {
    let mls = client.mls_client_required("resolve_deferral").ok()?;
    match crate::encryption::lock_mls(mls).group_epoch(group_id) {
        Ok(epoch) => Some(epoch),
        Err(e) => {
            warn!(room_id, error = %e, "drain: reading the deferred group's epoch failed");
            None
        }
    }
}

/// Clear a pending commit that held the drain back past the bound, so
/// the message behind it is processed (and heals) as before Step 97d.
/// `true` when cleared.
fn clear_abandoned_commit(client: &Client, room_id: &str, group_id: &[u8]) -> bool {
    warn!(room_id, "drain: own pending commit abandoned; clearing it");
    let Ok(mls) = client.mls_client_required("resolve_deferral") else {
        return false;
    };
    match crate::encryption::lock_mls(mls).clear_pending_commit(group_id) {
        Ok(()) => true,
        Err(e) => {
            warn!(room_id, error = %e, "drain: clearing the abandoned commit failed");
            false
        }
    }
}

/// `Some(persisted cursor)` when the persisted cursor is no longer
/// `since` — another `Client` or process dispatched in between. Without
/// a store there is no shared cursor: never moved.
async fn cursor_moved(
    client: &Client,
    since: Option<&str>,
) -> Result<Option<Option<String>>, KupferdrahtError> {
    if client.store_handle().is_none() {
        return Ok(None);
    }
    let persisted = persisted_last_event_id(client).await?;
    Ok((persisted.as_deref() != since).then_some(persisted))
}

async fn persisted_last_event_id(client: &Client) -> Result<Option<String>, KupferdrahtError> {
    let Some(store) = client.store_handle() else {
        return Ok(None);
    };
    Ok(store.load_sync_state().await?.and_then(|s| s.last_event_id))
}

/// Fetch one `/v14/notifications` page; when the backend no longer
/// holds `since` (its labelled `404 not-found` — backend-spec §11.4)
/// reset the cursor and refetch marker-less. The reset happens at most
/// once per drain; any other 404 surfaces as an error and leaves the
/// cursor alone. Steps 64, 102.
async fn fetch_page_recovering_stale_cursor(
    client: &Arc<Client>,
    access_token: &str,
    device_id: &str,
    since: &mut Option<String>,
    cursor_reset_done: &mut bool,
) -> Result<crate::wire::notification_dto::NotificationListResponse, KupferdrahtError> {
    loop {
        let fetched = client
            .http_fetch_notifications(
                client.api_version(),
                access_token,
                device_id,
                since.as_deref(),
                100,
            )
            .await?;
        match fetched {
            FetchedNotifications::Page(page) => return Ok(page),
            FetchedNotifications::SinceNotFound if !*cursor_reset_done => {
                debug_assert!(since.is_some(), "not-found answers a request with `since`");
                warn!("drain: the backend no longer holds the cursor; refetching marker-less");
                // Step 118: the refetch skips the gap — the room list may
                // have missed an event in it.
                client.mark_rooms_stale();
                clear_persisted_cursor(client).await?;
                *since = None;
                *cursor_reset_done = true;
            }
            FetchedNotifications::SinceNotFound => {
                return Err(KupferdrahtError::BackendRejected(
                    "fetch_notifications: the backend lost the cursor twice in one drain".into(),
                ));
            }
        }
    }
}

/// Drop the persisted `last_event_id` so a crash mid-recovery does not
/// retry the dead cursor forever; the dispatch loop re-persists fresh
/// ids as it processes the marker-less refetch.
async fn clear_persisted_cursor(client: &Client) -> Result<(), KupferdrahtError> {
    let Some(store) = client.store_handle() else {
        return Ok(());
    };
    let mut state = store.load_sync_state().await?.unwrap_or_default();
    state.last_event_id = None;
    store.save_sync_state(&state).await
}

#[cfg(test)]
mod tests {
    use super::{
        ABANDON_PENDING_COMMIT_AFTER_MS, DeferralClock, DeferralStep, deferral_clock, deferral_step,
    };

    #[test]
    fn a_first_deferral_starts_the_clock_now() {
        assert_eq!(
            deferral_clock(None, 7, 1_000),
            DeferralClock {
                epoch: 7,
                since_ms: 1_000
            }
        );
    }

    #[test]
    fn a_deferral_at_the_same_epoch_keeps_the_clock() {
        let started = DeferralClock {
            epoch: 7,
            since_ms: 1_000,
        };
        assert_eq!(deferral_clock(Some(started), 7, 50_000), started);
    }

    #[test]
    fn a_deferral_at_another_epoch_starts_over() {
        // The commit timed so far was merged (7 → 8); the message now
        // waits on a newer pending commit — never abandoned on the old
        // commit's clock.
        let stale = DeferralClock {
            epoch: 7,
            since_ms: 1_000,
        };
        assert_eq!(
            deferral_clock(Some(stale), 8, 90_000),
            DeferralClock {
                epoch: 8,
                since_ms: 90_000
            }
        );
    }

    #[test]
    fn a_flow_in_flight_is_waited_for_however_long_the_deferral() {
        assert_eq!(deferral_step(true, 0), DeferralStep::WaitForFlow);
        assert_eq!(
            deferral_step(true, ABANDON_PENDING_COMMIT_AFTER_MS * 2),
            DeferralStep::WaitForFlow
        );
    }

    #[test]
    fn without_a_flow_the_drain_stops_until_the_bound() {
        assert_eq!(deferral_step(false, 0), DeferralStep::StopDrain);
        assert_eq!(
            deferral_step(false, ABANDON_PENDING_COMMIT_AFTER_MS - 1),
            DeferralStep::StopDrain
        );
    }

    #[test]
    fn a_pending_commit_past_the_bound_is_abandoned() {
        assert_eq!(
            deferral_step(false, ABANDON_PENDING_COMMIT_AFTER_MS),
            DeferralStep::Abandon
        );
    }
}
