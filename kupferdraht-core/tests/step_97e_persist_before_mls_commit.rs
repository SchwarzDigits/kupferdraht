// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 97e — commit an inbound notification's MLS state only after its
//! data is persisted (ADR-0032, review finding B9). TDD red.
//!
//! Receiving changes two databases: decrypting advances the MLS state in
//! `mls.db` (the message key is deleted), the content lands in
//! `kupferdraht.db` afterwards, the cursor after that. A failure between the
//! two lost the message: the notification comes again, but its key is
//! gone. Now each notification is processed at least once — its MLS
//! changes commit after its data is stored — and every step tolerates a
//! replay.
//!
//! A `FaultyStore` (tests/common) makes one `kupferdraht.db` write fail or
//! park at a chosen point; a fixture peer (bob, `common::mls::PeerGroup`)
//! owns the MLS group and reads what the client sends.

use std::sync::Arc;
use std::time::Duration;

use httpmock::MockServer;
use kupferdraht::SyncSettings;
use kupferdraht::storage::Store;

mod common;

use common::drain_fixture::{
    Notif, cursor, drain, joined_client, mls_event, mount_message_upload, mount_page,
    reaction_message, room_id, text_content, text_message,
};

// ---- tests ----

/// B9 itself: the message is decrypted, then storing it fails. Its MLS
/// changes must roll back and the cursor stay before it, so a later
/// drain decrypts it again. Before: the key was gone, the cursor moved
/// on — the message was lost.
#[tokio::test]
async fn a_failed_persist_rolls_back_the_mls_state_and_the_message_arrives_later() {
    let server = MockServer::start();
    let mut fx = joined_client(&server).await;
    let message = fx
        .bob
        .application_message(&text_message("m-97e-1", "later"));
    mount_page(
        &server,
        Some("n1"),
        &[Notif {
            id: "n2",
            events: vec![mls_event(&message)],
        }],
    );
    mount_page(&server, Some("n2"), &[]);

    fx.store.fail_when(Box::new(|method| {
        method == "insert_timeline_event_with_mentions"
    }));
    let _ = fx.client.sync_once(SyncSettings::default()).await;
    assert_eq!(
        cursor(&fx.store).await.as_deref(),
        Some("n1"),
        "the cursor stays before the notification whose data was not stored"
    );

    fx.store.heal();
    drain(&fx.client).await;
    assert_eq!(
        fx.recorder.count("m-97e-1"),
        1,
        "a later drain decrypts and stores the message"
    );
}

/// A crash after the MLS commit, before the cursor: the notification
/// comes again, but its MLS work is done. The replay is skipped quietly —
/// no sync error, no second callback.
#[tokio::test]
async fn a_notification_replayed_after_its_mls_commit_is_skipped_quietly() {
    let server = MockServer::start();
    let mut fx = joined_client(&server).await;
    let message = fx.bob.application_message(&text_message("m-97e-2", "once"));
    mount_page(
        &server,
        Some("n1"),
        &[Notif {
            id: "n2",
            events: vec![mls_event(&message)],
        }],
    );
    mount_page(&server, Some("n2"), &[]);

    // The cursor write after this notification's data fails.
    let stored = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let seen = stored.clone();
    fx.store.fail_when(Box::new(move |method| {
        if method == "insert_timeline_event_with_mentions" {
            seen.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        method == "save_sync_state" && seen.load(std::sync::atomic::Ordering::SeqCst)
    }));
    let _ = fx.client.sync_once(SyncSettings::default()).await;
    fx.store.heal();
    let errors_before = fx.recorder.errors().len();

    drain(&fx.client).await;
    assert_eq!(
        fx.recorder.errors()[errors_before..].to_vec(),
        Vec::<String>::new(),
        "the replay raises no sync error"
    );
    assert_eq!(fx.recorder.count("m-97e-2"), 1, "no second callback");
    assert_eq!(cursor(&fx.store).await.as_deref(), Some("n2"));
}

/// A send must not write into an open notification window — its ratchet
/// advance must never be rolled back with the notification. It waits.
#[tokio::test]
async fn a_send_waits_until_the_notification_window_closes() {
    let server = MockServer::start();
    let mut fx = joined_client(&server).await;
    let uploads = mount_message_upload(&server);
    let message = fx
        .bob
        .application_message(&text_message("m-97e-3", "incoming"));
    mount_page(
        &server,
        Some("n1"),
        &[Notif {
            id: "n2",
            events: vec![mls_event(&message)],
        }],
    );
    mount_page(&server, Some("n2"), &[]);

    let arrived = fx.store.pause_next("insert_timeline_event_with_mentions");
    let drainer = fx.client.clone();
    let drain_task = tokio::spawn(async move { drainer.sync_once(SyncSettings::default()).await });
    arrived.notified().await;
    let timeline = fx
        .client
        .room(&room_id())
        .await
        .expect("room")
        .timeline()
        .expect("timeline");
    let send_task =
        tokio::spawn(async move { timeline.send_text("sent during the window".into()).await });
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !send_task.is_finished(),
        "the send waits while the notification window is open"
    );

    fx.store.release();
    drain_task.await.expect("drain task").expect("drain");
    send_task.await.expect("send task").expect("send");
    let sent = uploads.lock().expect("uploads").last().cloned();
    let plaintext = fx
        .bob
        .decrypt_application_message(&sent.expect("the send was uploaded"));
    assert_eq!(text_content(&plaintext), "sent during the window");
    assert_eq!(fx.recorder.count("m-97e-3"), 1);
}

/// A notification with two events: a reaction, then a text whose storing
/// fails. The rollback covers both; the replay applies the reaction a
/// second time — which must not duplicate it — and stores the text.
#[tokio::test]
async fn a_rolled_back_notification_replays_without_duplicating_effects() {
    let server = MockServer::start();
    let mut fx = joined_client(&server).await;
    let target = fx
        .bob
        .application_message(&text_message("m-97e-t0", "target"));
    mount_page(
        &server,
        Some("n1"),
        &[Notif {
            id: "n2",
            events: vec![mls_event(&target)],
        }],
    );
    drain(&fx.client).await;
    let reaction = fx
        .bob
        .application_message(&reaction_message("m-97e-r1", "m-97e-t0", "👍"));
    let text = fx
        .bob
        .application_message(&text_message("m-97e-4", "after the reaction"));
    mount_page(
        &server,
        Some("n2"),
        &[Notif {
            id: "n3",
            events: vec![mls_event(&reaction), mls_event(&text)],
        }],
    );
    mount_page(&server, Some("n3"), &[]);

    fx.store.fail_when(Box::new(|method| {
        method == "insert_timeline_event_with_mentions"
    }));
    let _ = fx.client.sync_once(SyncSettings::default()).await;
    fx.store.heal();
    drain(&fx.client).await;

    assert_eq!(fx.recorder.count("m-97e-4"), 1, "the text arrives once");
    let reactions: Vec<_> = fx
        .store
        .load_reactions_for_room(&room_id())
        .await
        .expect("reactions")
        .into_iter()
        .filter(|r| r.message_id == "m-97e-t0")
        .collect();
    assert_eq!(
        reactions.len(),
        1,
        "the reaction exists once: {reactions:?}"
    );
}
