// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 109 — commit a drained page, not each notification (ADR-0037).
//! TDD red.
//!
//! On remote storage every commit is a round trip. A page is now one MLS
//! window and one `kupferdraht.db` page transaction with a savepoint per
//! notification; then `kupferdraht.db` commits, then `mls.db`, then the cursor
//! moves — ADR-0032's order, per page. Other writers of `kupferdraht.db` wait
//! for the page, and what the embedder hears about the page is emitted
//! after the commit.
//!
//! The fixture (tests/common/drain_fixture) is the Step 97e client joined
//! to bob's group; its `FaultyStore` records every store call, fails or
//! parks chosen ones.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use httpmock::MockServer;
use kupferdraht::SyncSettings;
use kupferdraht::storage::Store;

mod common;

use common::drain_fixture::{
    Fixture, Notif, cursor, drain, joined_client, mls_event, mount_message_upload, mount_page,
    room_id, text_content, text_message,
};

/// Wait for a parked store call; a red build never makes it, so fail
/// instead of hanging.
async fn parked(arrived: Arc<tokio::sync::Notify>, what: &str) {
    tokio::time::timeout(Duration::from_secs(10), arrived.notified())
        .await
        .unwrap_or_else(|_| panic!("the drain never reached {what}"));
}

/// A page of three text notifications n2..n4 after the Welcome at n1;
/// returns their events.
async fn three_message_page(server: &MockServer, fx: &mut Fixture) -> Vec<String> {
    let events: Vec<String> = ["m-109-1", "m-109-2", "m-109-3"]
        .iter()
        .map(|id| mls_event(&fx.bob.application_message(&text_message(id, id))))
        .collect();
    mount_page(
        server,
        Some("n1"),
        &[
            Notif {
                id: "n2",
                events: vec![events[0].clone()],
            },
            Notif {
                id: "n3",
                events: vec![events[1].clone()],
            },
            Notif {
                id: "n4",
                events: vec![events[2].clone()],
            },
        ],
    );
    mount_page(server, Some("n4"), &[]);
    events
}

fn is_insert(method: &str) -> bool {
    method.starts_with("insert_timeline_event")
}

/// The page is one transaction: one `begin_page`, every insert inside it,
/// one `commit_page` — and the cursor is written after the commit, not in
/// it.
#[tokio::test]
async fn a_page_is_one_transaction_and_the_cursor_follows_it() {
    let server = MockServer::start();
    let mut fx = joined_client(&server).await;
    three_message_page(&server, &mut fx).await;
    fx.store.clear_calls();

    drain(&fx.client).await;

    let calls = fx.store.calls();
    let begin = calls.iter().position(|c| c == "begin_page");
    let commit = calls.iter().position(|c| c == "commit_page");
    let (Some(begin), Some(commit)) = (begin, commit) else {
        panic!("the page opens and commits a page transaction: {calls:?}");
    };
    assert_eq!(
        calls.iter().filter(|c| *c == "begin_page").count(),
        1,
        "one page transaction for the page: {calls:?}"
    );
    let inserts: Vec<usize> = calls
        .iter()
        .enumerate()
        .filter(|(_, c)| is_insert(c))
        .map(|(i, _)| i)
        .collect();
    assert_eq!(inserts.len(), 3, "three messages stored: {calls:?}");
    assert!(
        inserts.iter().all(|&i| begin < i && i < commit),
        "every insert lies inside the page: {calls:?}"
    );
    let cursor_write = calls
        .iter()
        .rposition(|c| c == "save_sync_state")
        .expect("the cursor is written");
    assert!(
        cursor_write > commit,
        "the cursor is written after the page commits: {calls:?}"
    );
    assert_eq!(cursor(&fx.store).await.as_deref(), Some("n4"));
    for id in ["m-109-1", "m-109-2", "m-109-3"] {
        assert_eq!(fx.recorder.count(id), 1, "{id} arrives once");
    }
}

/// A notification in the middle of the page fails to persist: it rolls
/// back to its savepoint — its MLS changes too — everything before it
/// commits, the cursor stops before it. A later drain brings the rest.
#[tokio::test]
async fn a_failed_notification_keeps_the_ones_before_it() {
    let server = MockServer::start();
    let mut fx = joined_client(&server).await;
    let events = three_message_page(&server, &mut fx).await;
    // What the backend answers once the cursor stands at n2.
    mount_page(
        &server,
        Some("n2"),
        &[
            Notif {
                id: "n3",
                events: vec![events[1].clone()],
            },
            Notif {
                id: "n4",
                events: vec![events[2].clone()],
            },
        ],
    );
    let inserts = Arc::new(AtomicUsize::new(0));
    let seen = inserts.clone();
    fx.store.fail_when(Box::new(move |method| {
        is_insert(method) && seen.fetch_add(1, Ordering::SeqCst) == 1
    }));

    let _ = fx.client.sync_once(SyncSettings::default()).await;
    assert_eq!(
        cursor(&fx.store).await.as_deref(),
        Some("n2"),
        "the cursor stops before the failed notification"
    );
    assert_eq!(fx.recorder.count("m-109-1"), 1, "the one before is kept");
    assert_eq!(fx.recorder.count("m-109-2"), 0);
    assert_eq!(fx.recorder.count("m-109-3"), 0);

    fx.store.heal();
    drain(&fx.client).await;
    assert_eq!(cursor(&fx.store).await.as_deref(), Some("n4"));
    for id in ["m-109-1", "m-109-2", "m-109-3"] {
        assert_eq!(
            fx.recorder.count(id),
            1,
            "{id} arrives once — the failed one decrypts again"
        );
    }
}

/// The page commit fails: the whole page rolls back, nothing was announced
/// to the embedder, the cursor stays. A later drain processes the page
/// again and every message arrives once.
#[tokio::test]
async fn a_failed_page_commit_rolls_back_the_page() {
    let server = MockServer::start();
    let mut fx = joined_client(&server).await;
    three_message_page(&server, &mut fx).await;
    fx.store
        .fail_when(Box::new(|method| method == "commit_page"));

    let _ = fx.client.sync_once(SyncSettings::default()).await;
    assert_eq!(cursor(&fx.store).await.as_deref(), Some("n1"));
    for id in ["m-109-1", "m-109-2", "m-109-3"] {
        assert_eq!(
            fx.recorder.count(id),
            0,
            "{id} was not announced: its page was rolled back"
        );
    }

    fx.store.heal();
    drain(&fx.client).await;
    assert_eq!(cursor(&fx.store).await.as_deref(), Some("n4"));
    for id in ["m-109-1", "m-109-2", "m-109-3"] {
        assert_eq!(fx.recorder.count(id), 1, "{id} arrives once");
    }
}

/// What the embedder hears about a page comes after the page commits.
#[tokio::test]
async fn the_embedder_hears_of_a_page_after_it_commits() {
    let server = MockServer::start();
    let mut fx = joined_client(&server).await;
    three_message_page(&server, &mut fx).await;

    let arrived = fx.store.pause_next("commit_page");
    let drainer = fx.client.clone();
    let drain_task = tokio::spawn(async move { drainer.sync_once(SyncSettings::default()).await });
    parked(arrived, "commit_page").await;
    assert_eq!(
        fx.recorder.count("m-109-1"),
        0,
        "nothing is announced before the page commits"
    );

    fx.store.release();
    drain_task.await.expect("drain task").expect("drain");
    for id in ["m-109-1", "m-109-2", "m-109-3"] {
        assert_eq!(fx.recorder.count(id), 1, "{id} is announced once");
    }
}

/// A send waits for the whole page, not just one notification: its
/// ratchet advance must never land in the page's MLS window.
#[tokio::test]
async fn a_send_waits_for_the_whole_page() {
    let server = MockServer::start();
    let mut fx = joined_client(&server).await;
    let uploads = mount_message_upload(&server);
    three_message_page(&server, &mut fx).await;

    let arrived = fx.store.pause_next("commit_page");
    let drainer = fx.client.clone();
    let drain_task = tokio::spawn(async move { drainer.sync_once(SyncSettings::default()).await });
    parked(arrived, "commit_page").await;
    let timeline = fx
        .client
        .room(&room_id())
        .await
        .expect("room")
        .timeline()
        .expect("timeline");
    let send_task =
        tokio::spawn(async move { timeline.send_text("sent during the page".into()).await });
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !send_task.is_finished(),
        "the send waits while the page is open, though every notification is done"
    );

    fx.store.release();
    drain_task.await.expect("drain task").expect("drain");
    send_task.await.expect("send task").expect("send");
    let sent = uploads.lock().expect("uploads").last().cloned();
    let plaintext = fx
        .bob
        .decrypt_application_message(&sent.expect("the send was uploaded"));
    assert_eq!(text_content(&plaintext), "sent during the page");
}

/// A write of the embedder while a page is open waits for it — and is not
/// lost when the page then fails and rolls back.
#[tokio::test]
async fn a_write_during_a_failing_page_is_kept() {
    let server = MockServer::start();
    let mut fx = joined_client(&server).await;
    let _uploads = mount_message_upload(&server);
    three_message_page(&server, &mut fx).await;

    fx.store
        .fail_when(Box::new(|method| method == "commit_page"));
    let arrived = fx.store.pause_next("commit_page");
    let drainer = fx.client.clone();
    let drain_task = tokio::spawn(async move { drainer.sync_once(SyncSettings::default()).await });
    parked(arrived, "commit_page").await;
    let timeline = fx
        .client
        .room(&room_id())
        .await
        .expect("room")
        .timeline()
        .expect("timeline");
    let send_task =
        tokio::spawn(async move { timeline.send_text("written during the page".into()).await });
    tokio::time::sleep(Duration::from_millis(300)).await;

    fx.store.release();
    let _ = drain_task.await.expect("drain task");
    send_task.await.expect("send task").expect("send");
    let kept = fx
        .store
        .load_timeline_events(&room_id())
        .await
        .expect("timeline")
        .iter()
        .any(|row| row.body.contains("written during the page"));
    assert!(kept, "the send's row survives the rolled-back page");
    assert_eq!(cursor(&fx.store).await.as_deref(), Some("n1"));

    fx.store.heal();
    drain(&fx.client).await;
    for id in ["m-109-1", "m-109-2", "m-109-3"] {
        assert_eq!(fx.recorder.count(id), 1, "{id} arrives once");
    }
}

/// A crash after the MLS commit, before the cursor: the page comes again,
/// its MLS work is done — the replay is skipped quietly, nothing twice.
#[tokio::test]
async fn a_page_replayed_after_its_mls_commit_is_skipped_quietly() {
    let server = MockServer::start();
    let mut fx = joined_client(&server).await;
    three_message_page(&server, &mut fx).await;
    let committed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let seen = committed.clone();
    fx.store.fail_when(Box::new(move |method| {
        if method == "commit_page" {
            seen.store(true, Ordering::SeqCst);
            return false;
        }
        method == "save_sync_state" && seen.load(Ordering::SeqCst)
    }));

    let _ = fx.client.sync_once(SyncSettings::default()).await;
    assert!(
        committed.load(Ordering::SeqCst),
        "the page committed before its cursor write failed"
    );
    fx.store.heal();
    let errors_before = fx.recorder.errors().len();
    drain(&fx.client).await;

    assert_eq!(
        fx.recorder.errors()[errors_before..].to_vec(),
        Vec::<String>::new(),
        "the replay raises no sync error"
    );
    for id in ["m-109-1", "m-109-2", "m-109-3"] {
        assert_eq!(fx.recorder.count(id), 1, "{id} is announced once");
    }
    assert_eq!(cursor(&fx.store).await.as_deref(), Some("n4"));
}
