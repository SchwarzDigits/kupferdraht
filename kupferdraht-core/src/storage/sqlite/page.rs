// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! The page transaction of the `SQLite` store (Step 109, ADR-0037): one
//! `BEGIN IMMEDIATE` for a drained page, a savepoint per notification,
//! and a gate that makes every write from outside the drain wait until
//! the page ends. The per-event transactions of Step 105 become
//! savepoints while a page is open (`write_tx`).

use tokio::sync::OwnedRwLockWriteGuard;

use crate::error::KupferdrahtError;

use super::SqliteStore;

/// Open the page: `hold` is the page gate, taken exclusively by the caller.
pub(super) fn begin_page(
    store: &SqliteStore,
    hold: OwnedRwLockWriteGuard<()>,
) -> Result<(), KupferdrahtError> {
    let mut page = store.page.lock().map_err(poisoned)?;
    assert!(
        page.is_none(),
        "one page at a time: the drain lock serialises pages"
    );
    store
        .lock()?
        .execute_batch("BEGIN IMMEDIATE")
        .map_err(|e| KupferdrahtError::Storage(format!("page begin: {e}")))?;
    *page = Some(hold);
    Ok(())
}

pub(super) fn savepoint(store: &SqliteStore, index: u32) -> Result<(), KupferdrahtError> {
    debug_assert!(page_open(store), "a savepoint inside an open page");
    store
        .lock()?
        .execute_batch(&format!("SAVEPOINT notification_{index}"))
        .map_err(|e| KupferdrahtError::Storage(format!("page savepoint {index}: {e}")))
}

pub(super) fn rollback_to_savepoint(
    store: &SqliteStore,
    index: u32,
) -> Result<(), KupferdrahtError> {
    debug_assert!(page_open(store), "a rollback inside an open page");
    store
        .lock()?
        .execute_batch(&format!("ROLLBACK TO notification_{index}"))
        .map_err(|e| KupferdrahtError::Storage(format!("page rollback to {index}: {e}")))
}

/// Commit the page and release the gate; a failed commit rolls back.
pub(super) fn commit_page(store: &SqliteStore) -> Result<(), KupferdrahtError> {
    let committed = store
        .lock()?
        .execute_batch("COMMIT")
        .map_err(|e| KupferdrahtError::Storage(format!("page commit: {e}")));
    if committed.is_err() {
        rollback_page(store);
    } else {
        release(store);
    }
    committed
}

/// Roll the page back (a no-op without one) and release the gate.
pub(super) fn rollback_page(store: &SqliteStore) {
    if let Ok(conn) = store.lock()
        && !conn.is_autocommit()
        && let Err(e) = conn.execute_batch("ROLLBACK")
    {
        tracing::warn!(error = %e, "page rollback failed");
    }
    release(store);
}

/// Drop the page's hold on the gate: writes that waited go on.
fn release(store: &SqliteStore) {
    if let Ok(mut page) = store.page.lock() {
        page.take();
    }
}

fn page_open(store: &SqliteStore) -> bool {
    store.page.lock().is_ok_and(|page| page.is_some())
}

fn poisoned<T>(_: std::sync::PoisonError<T>) -> KupferdrahtError {
    KupferdrahtError::Storage("page state lock poisoned".into())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use crate::storage::page::page_scope;
    use crate::storage::sqlite::{SqliteStore, fresh_store};
    use crate::storage::{Store, SyncStateRecord, TimelineEventRecord};

    const ROOM: &str = "room@wire.test";

    fn row(message_id: &str) -> TimelineEventRecord {
        TimelineEventRecord {
            message_id: message_id.into(),
            room_id: ROOM.into(),
            sender_user_id: "u@wire.test".into(),
            sender_device_id: "dev".into(),
            kind: "text".into(),
            body: "hi".into(),
            sent_at_ms: 1_000,
            delivery_status: "received".into(),
            reply_to_id: None,
            edited_at_ms: None,
            redacted_at_ms: None,
            expires_at_ms: None,
            ephemeral_ttl_ms: None,
        }
    }

    /// Count the commits of the store's connection from now on.
    fn count_commits(store: &SqliteStore) -> Arc<AtomicUsize> {
        let commits = Arc::new(AtomicUsize::new(0));
        let hook = commits.clone();
        store.lock().expect("lock").commit_hook(Some(move || {
            hook.fetch_add(1, Ordering::SeqCst);
            false
        }));
        commits
    }

    async fn has_row(store: &SqliteStore, message_id: &str) -> bool {
        store
            .load_timeline_events(ROOM)
            .await
            .expect("load")
            .iter()
            .any(|r| r.message_id == message_id)
    }

    async fn cursor(store: &SqliteStore) -> Option<String> {
        store
            .load_sync_state()
            .await
            .expect("sync state")
            .and_then(|s| s.last_event_id)
    }

    /// The writes of a page — plain ones and the combined per-event
    /// writes of Step 105 — commit once, together.
    #[tokio::test]
    async fn a_page_commits_once() {
        let (_dir, store) = fresh_store();
        let commits = count_commits(&store);
        page_scope(async {
            store.begin_page().await.expect("begin");
            store.insert_timeline_event(&row("m1")).await.expect("m1");
            let inserted = store
                .insert_timeline_event_with_mentions(&row("m2"), &[])
                .await
                .expect("m2 joins the page instead of opening its own transaction");
            assert!(inserted);
            store.commit_page().await.expect("commit");
        })
        .await;
        assert_eq!(commits.load(Ordering::SeqCst), 1, "one commit for the page");
        assert!(has_row(&store, "m1").await && has_row(&store, "m2").await);
    }

    /// Rolling back to a notification's savepoint undoes that notification
    /// alone; what came before commits with the page.
    #[tokio::test]
    async fn a_savepoint_undoes_one_notification_and_keeps_the_ones_before() {
        let (_dir, store) = fresh_store();
        page_scope(async {
            store.begin_page().await.expect("begin");
            store.savepoint(0).await.expect("savepoint 0");
            store.insert_timeline_event(&row("m1")).await.expect("m1");
            store.savepoint(1).await.expect("savepoint 1");
            store.insert_timeline_event(&row("m2")).await.expect("m2");
            store.rollback_to_savepoint(1).await.expect("rollback to 1");
            store.commit_page().await.expect("commit");
        })
        .await;
        assert!(has_row(&store, "m1").await, "the notification before stays");
        assert!(
            !has_row(&store, "m2").await,
            "the rolled-back notification is gone"
        );
    }

    /// A rolled-back page leaves nothing behind.
    #[tokio::test]
    async fn a_rolled_back_page_leaves_nothing() {
        let (_dir, store) = fresh_store();
        page_scope(async {
            store.begin_page().await.expect("begin");
            store.insert_timeline_event(&row("m1")).await.expect("m1");
            store.rollback_page();
        })
        .await;
        assert!(!has_row(&store, "m1").await);
    }

    /// A write from outside the drain waits until the page is committed,
    /// and then commits on its own — it is never part of the page.
    #[tokio::test]
    async fn a_write_from_outside_waits_for_the_page() {
        let (_dir, store) = fresh_store();
        let store = Arc::new(store);
        page_scope(async {
            store.begin_page().await.expect("begin");
            store.insert_timeline_event(&row("m1")).await.expect("m1");
        })
        .await;

        let outside = store.clone();
        let writer = tokio::spawn(async move {
            outside
                .save_sync_state(&SyncStateRecord {
                    last_event_id: Some("from-outside".into()),
                    ..SyncStateRecord::default()
                })
                .await
        });
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(
            !writer.is_finished(),
            "the outside write waits for the page"
        );

        page_scope(async { store.commit_page().await.expect("commit") }).await;
        writer.await.expect("writer task").expect("outside write");
        assert_eq!(cursor(&store).await.as_deref(), Some("from-outside"));
    }

    /// A write that waited for a page that was rolled back is not lost: it
    /// runs after the rollback, on its own.
    #[tokio::test]
    async fn a_waiting_write_survives_a_rolled_back_page() {
        let (_dir, store) = fresh_store();
        let store = Arc::new(store);
        page_scope(async {
            store.begin_page().await.expect("begin");
            store.insert_timeline_event(&row("m1")).await.expect("m1");
        })
        .await;

        let outside = store.clone();
        let writer = tokio::spawn(async move { outside.insert_timeline_event(&row("mine")).await });
        tokio::time::sleep(Duration::from_millis(200)).await;
        store.rollback_page();
        writer.await.expect("writer task").expect("outside write");

        assert!(!has_row(&store, "m1").await, "the page is gone");
        assert!(has_row(&store, "mine").await, "the waiting write is kept");
    }

    /// Reads never wait for a page.
    #[tokio::test]
    async fn reads_do_not_wait_for_a_page() {
        let (_dir, store) = fresh_store();
        let store = Arc::new(store);
        page_scope(async {
            store.begin_page().await.expect("begin");
            store.insert_timeline_event(&row("m1")).await.expect("m1");
        })
        .await;

        let reader = store.clone();
        let read = tokio::time::timeout(Duration::from_secs(1), async move {
            reader.load_timeline_events(ROOM).await
        })
        .await;
        assert!(read.is_ok(), "a read returns while the page is open");
        store.rollback_page();
    }
}
