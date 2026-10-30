// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 14b — sync-surface refactor + `sync_members` rename.
//!
//! TDD red-phase. Eight integration tests pin the public-API
//! contract from `docs/plan/14-matrix-sdk-parity.md` §"TDD starter
//! tests — 14b" and CHANGELOG.md §40, §41:
//!
//! * `sync_once(SyncSettings)` composes a conversations-metadata
//!   refresh + one `/v14/notifications` drain + cursor advance,
//!   returns a `SyncSummary { rooms_updated, events_dispatched }`.
//! * `sync_with_callback(settings, F)` loops `sync_once` and invokes
//!   the callback per iteration; `ControlFlow::Break(())` exits.
//! * `sync_stream(settings)` surfaces the same loop as a
//!   `BoxStream<SyncSummary>`; dropping the stream cancels the
//!   underlying polling task.
//! * Concurrent `sync*` calls on the same `Client` surface
//!   `KupferdrahtError::SyncAlreadyRunning`.
//! * `Client::sync_members(&room_id)` + `Room::sync_members()`
//!   replace `refresh_members` (verb-taxonomy finding §41).
//!
//! `Client::sync(SyncSettings)` is **not tested here** — adding it
//! to the type collides with the existing `Client::sync() ->
//! Arc<SyncService>` factory. The atomic swap lands in the
//! green-phase commit. `sync_with_callback`'s loop semantics cover
//! the same shape.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use futures_util::StreamExt;
use httpmock::Method::{GET, POST};
use httpmock::MockServer;
use kupferdraht::{KupferdrahtError, SyncSettings, SyncSummary, SyncTransport};
use tokio::time::timeout;

mod common;

async fn logged_in_client(server: &MockServer) -> Arc<kupferdraht::Client> {
    common::mount_full_login_flow(server);
    let (client, _store) = common::run_login(server).await;
    client
}

/// Minimal conversation JSON for the rooms-metadata mocks. `type=0`
/// = Group; `members.others` empty so no `/list-users` backfill is
/// required.
fn conv_json(id: &str) -> String {
    format!(
        r#"{{
            "qualified_id":{{"id":"{id}","domain":"example.test"}},
            "type":0,
            "protocol":"mls",
            "epoch":1,
            "group_id":"Z3JwLW9uZQ==",
            "name":"Step 14b smoke",
            "members":{{
              "self":{{"qualified_id":{{"id":"user-uuid","domain":"example.test"}}}},
              "others":[]
            }}
        }}"#
    )
}

/// Mount the conversations side of `sync_once` so that one Group
/// room is visible to the caller.
fn mount_rooms_with_one_group(server: &MockServer, conv_id: &str) {
    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list-ids");
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(
                r#"{{"qualified_conversations":[{{"id":"{conv_id}","domain":"example.test"}}],
                     "has_more":false,"paging_state":"END"}}"#
            ));
    });
    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list");
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(
                r#"{{"found":[{}], "not_found":[], "failed":[]}}"#,
                conv_json(conv_id),
            ));
    });
}

/// Mount the notifications side of `sync_once` so the queue carries
/// exactly one frame with `count` events (the events themselves can
/// be unknown types — the dispatcher skips them gracefully).
fn mount_notifications_with(server: &MockServer, frame_id: &str, count: usize) {
    let events = (0..count)
        .map(|_| r#"{"type":"unknown.for-test"}"#.to_string())
        .collect::<Vec<_>>()
        .join(",");
    server.mock(|when, then| {
        when.method(GET).path("/v14/notifications");
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(
                r#"{{"notifications":[{{"id":"{frame_id}","payload":[{events}]}}],
                     "has_more":false}}"#
            ));
    });
}

/// Mount notifications to return an empty queue (no frames).
fn mount_empty_notifications(server: &MockServer) {
    server.mock(|when, then| {
        when.method(GET).path("/v14/notifications");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"notifications":[],"has_more":false}"#);
    });
}

#[tokio::test]
async fn sync_once_returns_summary_with_room_and_event_counts() {
    let server = MockServer::start();
    let client = logged_in_client(&server).await;
    mount_rooms_with_one_group(&server, "room-a");
    mount_notifications_with(&server, "notif-1", 2);

    let summary = client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync_once must succeed");
    assert_eq!(summary.rooms_updated, 1, "one room in the mocked list");
    assert_eq!(summary.events_dispatched, 2, "two events in the frame");
}

#[tokio::test]
async fn sync_once_advances_cursor_so_second_call_returns_zero_events() {
    // First call drains a frame, persists the cursor. Second call
    // hits an empty queue and reports zero events.
    let server = MockServer::start();
    let client = logged_in_client(&server).await;
    mount_rooms_with_one_group(&server, "room-a");

    // Notifications mock: first hit returns one frame, later hits
    // return empty. httpmock matches in registration order, so we
    // mount the empty mock first and the populated one with a
    // matcher that limits it to the first call — simpler: two
    // sequential mocks where the second overrides.
    // First call (no `since` yet): one notification queued.
    server.mock(|when, then| {
        when.method(GET)
            .path("/v14/notifications")
            .is_true(|req| req.query_params().iter().all(|(k, _)| k != "since"));
        then.status(200)
            .header("content-type", "application/json")
            .body(
                r#"{"notifications":[{"id":"notif-1","payload":[{"type":"unknown.for-test"}]}],
                     "has_more":false}"#,
            );
    });
    // Second call (cursor advanced): empty queue.
    server.mock(|when, then| {
        when.method(GET)
            .path("/v14/notifications")
            .query_param("since", "notif-1");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"notifications":[],"has_more":false}"#);
    });

    let first_summary = client
        .sync_once(SyncSettings::default())
        .await
        .expect("first sync_once");
    assert_eq!(first_summary.events_dispatched, 1);

    let second_summary = client
        .sync_once(SyncSettings::default())
        .await
        .expect("second sync_once");
    assert_eq!(
        second_summary.events_dispatched, 0,
        "cursor must advance — second call sees an empty queue"
    );
}

#[tokio::test]
async fn sync_with_callback_invokes_per_iteration_and_break_exits() {
    let server = MockServer::start();
    let client = logged_in_client(&server).await;
    mount_rooms_with_one_group(&server, "room-a");
    mount_empty_notifications(&server);

    let calls = Arc::new(AtomicU32::new(0));
    let calls_clone = Arc::clone(&calls);
    let settings = SyncSettings {
        poll_interval: Some(Duration::from_millis(1)),
        // Step 118: push is the native default; these pin the polling loop.
        transport: SyncTransport::Polling,
    };
    timeout(
        Duration::from_secs(5),
        client.sync_with_callback(settings, move |_summary: SyncSummary| {
            let n = calls_clone.fetch_add(1, Ordering::SeqCst);
            // Loop three times, then break.
            if n >= 2 {
                std::ops::ControlFlow::Break(())
            } else {
                std::ops::ControlFlow::Continue(())
            }
        }),
    )
    .await
    .expect("sync_with_callback must finish within the timeout")
    .expect("sync_with_callback must succeed");
    assert_eq!(
        calls.load(Ordering::SeqCst),
        3,
        "callback should fire 3 times before Break exits the loop"
    );
}

#[tokio::test]
async fn sync_stream_yields_summary_per_iteration() {
    let server = MockServer::start();
    let client = logged_in_client(&server).await;
    mount_rooms_with_one_group(&server, "room-a");
    mount_empty_notifications(&server);

    let settings = SyncSettings {
        poll_interval: Some(Duration::from_millis(1)),
        // Step 118: push is the native default; these pin the polling loop.
        transport: SyncTransport::Polling,
    };
    let mut stream = client.sync_stream(settings);
    let first = timeout(Duration::from_secs(5), stream.next())
        .await
        .expect("stream must yield first item")
        .expect("stream must not terminate before first item")
        .expect("first item must be Ok");
    let second = timeout(Duration::from_secs(5), stream.next())
        .await
        .expect("stream must yield second item")
        .expect("stream must not terminate")
        .expect("second item must be Ok");
    // Both items are zero-events summaries (empty notification
    // queue), but the rooms-updated count from the metadata refresh
    // is consistent across iterations.
    assert_eq!(first.rooms_updated, 1);
    assert_eq!(second.rooms_updated, 1);
}

#[tokio::test]
async fn sync_stream_drop_cancels_polling_task() {
    let server = MockServer::start();
    let client = logged_in_client(&server).await;
    mount_rooms_with_one_group(&server, "room-a");
    mount_empty_notifications(&server);

    let settings = SyncSettings {
        poll_interval: Some(Duration::from_millis(1)),
        // Step 118: push is the native default; these pin the polling loop.
        transport: SyncTransport::Polling,
    };
    let conv_mock = server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list-ids");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"qualified_conversations":[],"has_more":false,"paging_state":"END"}"#);
    });

    let mut stream = client.sync_stream(settings);
    // Consume one item.
    let _ = timeout(Duration::from_secs(5), stream.next())
        .await
        .expect("first yield");
    let hits_before_drop = conv_mock.calls();
    drop(stream);
    // Give the spawned task a moment to observe the drop.
    tokio::time::sleep(Duration::from_millis(100)).await;
    let hits_after_drop = conv_mock.calls();
    // After dropping the stream, no further polling iterations
    // should fire. Allow a small grace (one in-flight iteration
    // might still complete) — anything more than that is a leaked
    // task.
    assert!(
        hits_after_drop - hits_before_drop <= 1,
        "drop must cancel polling: {hits_before_drop} → {hits_after_drop} hits"
    );
}

#[tokio::test]
async fn concurrent_sync_returns_sync_already_running() {
    let server = MockServer::start();
    let client = logged_in_client(&server).await;
    mount_rooms_with_one_group(&server, "room-a");
    // Notification call takes ≥ 100 ms so the second sync_once
    // starts while the first is still mid-flight.
    server.mock(|when, then| {
        when.method(GET).path("/v14/notifications");
        then.status(200)
            .header("content-type", "application/json")
            .delay(Duration::from_millis(100))
            .body(r#"{"notifications":[],"has_more":false}"#);
    });

    let c1 = client.clone();
    let c2 = client.clone();
    let t1 = tokio::spawn(async move { c1.sync_once(SyncSettings::default()).await });
    // Give task 1 a head-start so it owns the cursor lock.
    tokio::time::sleep(Duration::from_millis(20)).await;
    let r2 = c2.sync_once(SyncSettings::default()).await;
    let r1 = t1.await.expect("task 1 join");

    assert!(r1.is_ok(), "first sync_once must succeed: {r1:?}");
    assert!(
        matches!(r2, Err(KupferdrahtError::SyncAlreadyRunning)),
        "second concurrent sync_once must return SyncAlreadyRunning, got {r2:?}"
    );
}

// `sync_members` (renamed from `refresh_members`) is covered by the
// existing `step_3_room_join.rs` tests after they were migrated to
// call the new name. The rename is a pure symbol change — no
// behavioural test is duplicated here, since the heavyweight MLS
// bootstrap those tests already perform would only be duplicated.
