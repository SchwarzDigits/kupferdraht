// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 118 — the room list is refreshed when it may be out of date, not
//! every sync pass.
//!
//! The list (`list-ids`, `list`, `list-users`) only changes through events
//! the drain sees. So it is stale at start, and the drain marks it stale
//! when it hands over an event that may change it (one Kupferdraht does not
//! process itself, or a Welcome) or when this client cannot have seen every
//! event: the queue was drained elsewhere (another client or process on the
//! account, ADR-0031) or the backend lost the cursor. A refresh clears the
//! mark; a failed one leaves it set.
//!
//! The safety net: a list older than [`MAX_AGE_MS`] is refreshed too, so a
//! change no trigger caught is stale for at most that long.

use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Mutex, PoisonError};

use crate::client::Client;
use crate::error::KupferdrahtError;

/// How old the room list may get before a pass refreshes it without a
/// trigger (5 minutes — the browser's socket renewal).
const MAX_AGE_MS: i64 = 5 * 60 * 1000;

/// Where this client's last drain left the cursor.
enum DrainedTo {
    /// No drain yet: nothing to compare the next one against.
    Never,
    /// The cursor the last drain ended at (`None`: the queue's start).
    At(Option<String>),
}

/// Whether the cached room list may be out of date, when it was last
/// refreshed, and where this client's last drain left the cursor.
pub(crate) struct RoomFreshness {
    stale: AtomicBool,
    refreshed_at_ms: AtomicI64,
    drained_to: Mutex<DrainedTo>,
}

impl Default for RoomFreshness {
    /// Stale: a new client has not fetched the list yet.
    fn default() -> Self {
        Self {
            stale: AtomicBool::new(true),
            refreshed_at_ms: AtomicI64::new(0),
            drained_to: Mutex::new(DrainedTo::Never),
        }
    }
}

impl RoomFreshness {
    /// The room list may be out of date.
    pub(crate) fn mark_stale(&self) {
        self.stale.store(true, Ordering::SeqCst);
    }

    /// A drain starts at `cursor`: anything else than where this client's
    /// last drain ended means the queue was drained elsewhere.
    pub(crate) fn note_drain_start(&self, cursor: Option<&str>) {
        let drained_to = self
            .drained_to
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let DrainedTo::At(ended) = &*drained_to
            && ended.as_deref() != cursor
        {
            self.mark_stale();
        }
    }

    /// A drain of this client ended at `cursor`.
    pub(crate) fn note_drain_end(&self, cursor: Option<String>) {
        *self
            .drained_to
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = DrainedTo::At(cursor);
    }

    /// Whether a refresh is due at `now_ms` — marked stale, or older than
    /// [`MAX_AGE_MS`] — clearing the mark.
    fn take_due(&self, now_ms: i64) -> bool {
        let marked = self.stale.swap(false, Ordering::SeqCst);
        marked || now_ms.saturating_sub(self.refreshed_at_ms.load(Ordering::SeqCst)) >= MAX_AGE_MS
    }

    /// The list was refreshed at `now_ms`.
    fn note_refreshed(&self, now_ms: i64) {
        self.refreshed_at_ms.store(now_ms, Ordering::SeqCst);
    }
}

impl Client {
    /// The room list may be out of date (see the module docs).
    pub(crate) fn mark_rooms_stale(&self) {
        self.room_freshness.mark_stale();
    }

    /// The drain's view of the room list's freshness.
    pub(crate) fn room_freshness(&self) -> &RoomFreshness {
        &self.room_freshness
    }

    /// Refresh the room list when it is stale or older than [`MAX_AGE_MS`].
    /// The mark is cleared before the fetch, so an event marking it during
    /// the fetch is not lost; a failed fetch sets it again.
    ///
    /// # Errors
    /// Whatever [`Client::sync_metadata`] returns.
    pub(crate) async fn refresh_rooms_if_stale(&self) -> Result<(), KupferdrahtError> {
        let now_ms = crate::time::now_ms();
        if !self.room_freshness.take_due(now_ms) {
            return Ok(());
        }
        if let Err(e) = self.sync_metadata().await {
            self.mark_rooms_stale();
            return Err(e);
        }
        self.room_freshness.note_refreshed(now_ms);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW_MS: i64 = 1_800_000_000_000;

    /// A list refreshed at `NOW_MS`, the mark cleared.
    fn fresh() -> RoomFreshness {
        let freshness = RoomFreshness::default();
        freshness.take_due(NOW_MS);
        freshness.note_refreshed(NOW_MS);
        freshness
    }

    #[test]
    fn a_new_client_is_due() {
        assert!(RoomFreshness::default().take_due(NOW_MS));
    }

    #[test]
    fn a_fresh_list_is_not_due_until_marked() {
        let freshness = fresh();
        assert!(!freshness.take_due(NOW_MS + 1_000));
        freshness.mark_stale();
        assert!(freshness.take_due(NOW_MS + 1_000));
        assert!(
            !freshness.take_due(NOW_MS + 1_000),
            "taking clears the mark"
        );
    }

    /// The safety net: a change no trigger caught is stale for at most
    /// five minutes.
    #[test]
    fn a_list_older_than_five_minutes_is_due() {
        let freshness = fresh();
        assert!(!freshness.take_due(NOW_MS + MAX_AGE_MS - 1));
        assert!(freshness.take_due(NOW_MS + MAX_AGE_MS));
        assert_eq!(MAX_AGE_MS, 300_000);
    }

    /// Before its first drain this client has no cursor to compare: the
    /// first drain refreshes nothing the start's refresh did not.
    #[test]
    fn the_first_drain_marks_nothing() {
        let freshness = fresh();

        freshness.note_drain_start(Some("n3"));

        assert!(!freshness.take_due(NOW_MS));
    }

    #[test]
    fn a_drain_from_where_the_last_ended_keeps_the_list_fresh() {
        let freshness = fresh();
        freshness.note_drain_end(Some("n7".into()));

        freshness.note_drain_start(Some("n7"));

        assert!(!freshness.take_due(NOW_MS));
    }

    #[test]
    fn a_drain_from_elsewhere_marks_the_list_stale() {
        let freshness = fresh();
        freshness.note_drain_end(Some("n7".into()));

        freshness.note_drain_start(Some("n9"));

        assert!(freshness.take_due(NOW_MS));
    }
}
