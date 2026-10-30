// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 64 — sync robustness (TDD red phase).
//!
//! Pins `docs/plan/64-sync-robustness.md` §Goal 1–3 (review findings
//! S8, S13, S1 — `open.md` §66):
//!
//! * a redelivered notification whose id equals the persisted cursor
//!   is skipped, mirroring the push loop's dedup;
//! * a stale-cursor 404 resets the cursor and recovers within the
//!   same drain (backend-spec §11.4), instead of wedging sync;
//! * a `conversation.mls-message-add` whose dispatch fails with a
//!   generic (non-`MlsOutOfOrder`) error still triggers one room
//!   recovery — today only `MlsOutOfOrder` heals, so a commit lost
//!   to a transient error waits for the *next* message to break
//!   before the room resyncs.

use httpmock::Method::{GET, POST};
use httpmock::MockServer;
use kupferdraht::storage::SyncStateRecord;

mod common;

/// Empty rooms metadata so `sync_once` runs with minimal ceremony.
fn mount_empty_rooms(server: &MockServer) {
    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list-ids");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"qualified_conversations":[],"has_more":false,"paging_state":"END"}"#);
    });
}

/// One MLS group room (same shape as the step_14b fixture) so the
/// dispatcher can resolve `room-a`'s group and `resync_room` finds a
/// cached room with a `group_id`.
fn mount_one_group_room(server: &MockServer) {
    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list-ids");
        then.status(200)
            .header("content-type", "application/json")
            .body(
                r#"{"qualified_conversations":[{"id":"room-a","domain":"example.test"}],
                     "has_more":false,"paging_state":"END"}"#,
            );
    });
    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list");
        then.status(200)
            .header("content-type", "application/json")
            .body(
                r#"{"found":[{
                    "qualified_id":{"id":"room-a","domain":"example.test"},
                    "type":0,
                    "protocol":"mls",
                    "epoch":1,
                    "group_id":"Z3JwLW9uZQ==",
                    "name":"Step 64",
                    "members":{
                      "self":{"qualified_id":{"id":"user-uuid","domain":"example.test"}},
                      "others":[]
                    }
                }], "not_found":[], "failed":[]}"#,
            );
    });
}

/// Healthy key-package pool so the Step-63 auto-check stays silent.
fn mount_healthy_key_packages(server: &MockServer) {
    server.mock(|when, then| {
        when.method(GET)
            .path_includes("/mls/key-packages/self/")
            .path_includes("/count");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"count":100}"#);
    });
}

async fn seed_cursor(store: &std::sync::Arc<dyn kupferdraht::storage::Store>, cursor: &str) {
    store
        .save_sync_state(&SyncStateRecord {
            last_event_id: Some(cursor.to_string()),
            ..SyncStateRecord::default()
        })
        .await
        .expect("seed cursor");
}

async fn persisted_cursor(
    store: &std::sync::Arc<dyn kupferdraht::storage::Store>,
) -> Option<String> {
    store
        .load_sync_state()
        .await
        .expect("sync_state readable")
        .and_then(|s| s.last_event_id)
}

#[tokio::test]
async fn drain_skips_a_notification_matching_the_persisted_cursor() {
    // §11.3-symmetric dedup: the push loop acks a redelivered frame
    // without re-dispatching; the drain must tolerate a server that
    // redelivers the cursor notification the same way.
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    mount_empty_rooms(&server);
    mount_healthy_key_packages(&server);
    let (client, store) = common::run_login(&server).await;
    seed_cursor(&store, "notif-1").await;

    server.mock(|when, then| {
        when.method(GET).path("/v14/notifications");
        then.status(200)
            .header("content-type", "application/json")
            .body(
                r#"{"notifications":[
                    {"id":"notif-1","payload":[{"type":"unknown.for-test"}]},
                    {"id":"notif-2","payload":[{"type":"unknown.for-test"}]}
                   ],"has_more":false}"#,
            );
    });

    let summary = client
        .sync_once(kupferdraht::SyncSettings::default())
        .await
        .expect("drain succeeds");
    assert_eq!(
        summary.events_dispatched, 1,
        "the redelivered notif-1 must be skipped, only notif-2 dispatched"
    );
    assert_eq!(
        persisted_cursor(&store).await.as_deref(),
        Some("notif-2"),
        "cursor advances past the redelivery"
    );
}

#[tokio::test]
async fn drain_recovers_from_stale_cursor_404() {
    // backend-spec §11.4: a 404 on `since` means "cursor lost" — the
    // client resets and refetches marker-less instead of wedging.
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    mount_empty_rooms(&server);
    mount_healthy_key_packages(&server);
    let (client, store) = common::run_login(&server).await;
    seed_cursor(&store, "gone-cursor").await;

    server.mock(|when, then| {
        when.method(GET)
            .path("/v14/notifications")
            .query_param("since", "gone-cursor");
        then.status(404)
            .header("content-type", "application/json")
            .body(r#"{"code":404,"label":"not-found","message":"no such notification"}"#);
    });
    let fresh = server.mock(|when, then| {
        when.method(GET)
            .path("/v14/notifications")
            .is_true(|req| req.query_params().iter().all(|(k, _)| k != "since"));
        then.status(200)
            .header("content-type", "application/json")
            .body(
                r#"{"notifications":[{"id":"notif-9","payload":[{"type":"unknown.for-test"}]}],
                     "has_more":false}"#,
            );
    });

    let summary = client
        .sync_once(kupferdraht::SyncSettings::default())
        .await
        .expect("a stale cursor must not wedge the drain");
    assert_eq!(
        summary.events_dispatched, 1,
        "the marker-less refetch dispatches"
    );
    assert_eq!(fresh.calls(), 1, "exactly one marker-less recovery fetch");
    assert_eq!(
        persisted_cursor(&store).await.as_deref(),
        Some("notif-9"),
        "cursor resumes from the recovered queue head"
    );
}

#[tokio::test]
async fn failed_mls_message_dispatch_triggers_room_recovery() {
    // S1 residual: today only MlsOutOfOrder heals; a commit lost to a
    // generic error (here: no local MLS group → decrypt fails) waits
    // for the NEXT message to break before the room recovers. The
    // drain must attempt recovery for the failing message itself.
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    mount_one_group_room(&server);
    mount_healthy_key_packages(&server);
    let groupinfo = server.mock(|when, then| {
        when.method(GET).path_includes("/groupinfo");
        // 404 keeps the recovery attempt observable while the rejoin
        // itself fails — recover_room folds that into on_sync_error.
        then.status(404)
            .header("content-type", "application/json")
            .body(r#"{"code":404,"label":"mls-missing-group-info","message":"none"}"#);
    });
    let (client, store) = common::run_login(&server).await;

    server.mock(|when, then| {
        when.method(GET).path("/v14/notifications");
        then.status(200)
            .header("content-type", "application/json")
            .body(
                r#"{"notifications":[{"id":"notif-5","payload":[{
                    "type":"conversation.mls-message-add",
                    "qualified_conversation":{"id":"room-a","domain":"example.test"},
                    "qualified_from":{"id":"bob-uuid","domain":"example.test"},
                    "data":"AAECAw=="
                }]}],"has_more":false}"#,
            );
    });

    client
        .sync_once(kupferdraht::SyncSettings::default())
        .await
        .expect("per-event failures never abort the drain");
    assert_eq!(
        groupinfo.calls(),
        1,
        "a generically-failing mls-message-add must trigger exactly one \
         room-recovery attempt (resync_room → groupinfo fetch)"
    );
    assert_eq!(
        persisted_cursor(&store).await.as_deref(),
        Some("notif-5"),
        "the cursor contract is unchanged — it advances regardless"
    );
}
