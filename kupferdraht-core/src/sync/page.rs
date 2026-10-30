// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Dispatch a drained page (Step 109, ADR-0037): one window for the page,
//! a savepoint per notification, then the data commit, the MLS commit, the
//! notifications to the embedder, and the cursor — ADR-0032's order, per
//! page.

use std::sync::Arc;

use crate::client::Client;
use crate::error::KupferdrahtError;
use crate::storage::page::page_scope;
use crate::sync::outbox::{self, Held};
use crate::sync::service::{NotificationOutcome, SyncService};
use crate::sync::window::PageWindow;
use crate::wire::notification_dto::QueuedNotification;

/// What dispatching a page came to.
#[derive(Default)]
pub(super) struct PageRun {
    /// The events dispatched in the committed notifications.
    pub(super) dispatched: u32,
    /// The last committed notification — where the cursor now stands.
    pub(super) last_id: Option<String>,
    /// Why the page stopped before its end, if it did.
    pub(super) stop: Option<PageStop>,
}

/// Why a page stopped before its end: the notifications before it are
/// committed, this one is rolled back and the cursor stays before it.
pub(super) enum PageStop {
    /// An MLS message of `room_id` awaits an own pending commit.
    Deferred { room_id: String },
    /// Persisting the notification failed.
    Failed(KupferdrahtError),
}

impl SyncService {
    /// Dispatch `notifs` as one page and advance the cursor past the last
    /// committed one.
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` when the page's window cannot open or commit,
    /// or the cursor cannot be written; the page is then processed again
    /// by a later drain. A notification that fails is reported through
    /// [`PageRun::stop`], after the ones before it are committed.
    pub(super) async fn dispatch_page_of(
        &self,
        client: &Arc<Client>,
        notifs: &[&QueuedNotification],
    ) -> Result<PageRun, KupferdrahtError> {
        if notifs.is_empty() {
            return Ok(PageRun::default());
        }
        // Boxed: the page's future is large, and every sync loop awaits it.
        let (run, held) =
            Box::pin(page_scope(outbox::scope(self.run_page(client, notifs)))).await?;
        // Announce what is committed before the cursor moves: a failed
        // cursor write replays the page, and the replay dedupes — it would
        // never announce it again.
        self.emit_held(client, held);
        // The cursor after the MLS commit, outside the page (ADR-0032).
        if let Some(id) = &run.last_id {
            save_cursor(client, id).await?;
        }
        Ok(run)
    }

    /// Dispatch one notification as a page of one — the push paths, which
    /// run natively only.
    ///
    /// # Errors
    /// As [`Self::dispatch_page_of`], and the notification's own failure.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) async fn dispatch_notification(
        &self,
        client: &Arc<Client>,
        notif: &QueuedNotification,
    ) -> Result<NotificationOutcome, KupferdrahtError> {
        let run = self.dispatch_page_of(client, &[notif]).await?;
        match run.stop {
            None => Ok(NotificationOutcome::Dispatched(run.dispatched)),
            Some(PageStop::Deferred { room_id }) => Ok(NotificationOutcome::Deferred { room_id }),
            Some(PageStop::Failed(e)) => Err(e),
        }
    }

    /// Run the page inside its window; returns what was held for the
    /// embedder once the page committed.
    async fn run_page(
        &self,
        client: &Arc<Client>,
        notifs: &[&QueuedNotification],
    ) -> Result<(PageRun, Vec<Held>), KupferdrahtError> {
        let mut window = PageWindow::open(client).await?;
        let mut run = PageRun::default();
        for (index, notif) in notifs.iter().enumerate() {
            let index = u32::try_from(index)
                .map_err(|_| KupferdrahtError::Generic("a page longer than u32::MAX".into()))?;
            window.savepoint(index).await?;
            match self.dispatch_notification_events(client, notif).await {
                Ok(NotificationOutcome::Dispatched(n)) => {
                    run.dispatched = run.dispatched.saturating_add(n);
                    run.last_id = Some(notif.id.clone());
                }
                Ok(NotificationOutcome::Deferred { room_id }) => {
                    window.rollback_to(index).await?;
                    run.stop = Some(PageStop::Deferred { room_id });
                    break;
                }
                Err(e) => {
                    window.rollback_to(index).await?;
                    run.stop = Some(PageStop::Failed(e));
                    break;
                }
            }
        }
        if run.last_id.is_none() {
            window.rollback();
            return Ok((run, Vec::new()));
        }
        window.commit().await?;
        Ok((run, outbox::take()))
    }
}

/// Persist the cursor past notification `id`.
async fn save_cursor(client: &Client, id: &str) -> Result<(), KupferdrahtError> {
    let Some(store) = client.store_handle() else {
        return Ok(());
    };
    let mut state = store.load_sync_state().await?.unwrap_or_default();
    state.last_event_id = Some(id.to_string());
    store.save_sync_state(&state).await
}
