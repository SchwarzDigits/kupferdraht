// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 103 — pace the sync loops (`docs/plan/archive/103-polling-load.md`,
//! review finding H8): an unset poll interval pauses (10 s since Step 118) between empty
//! drains, and a `429` is waited out instead of aborting the loop.

use std::time::Duration;

use httpmock::Method::{GET, POST};
use httpmock::{Mock, MockServer};
use kupferdraht::{SyncSettings, SyncTransport};

mod common;

fn mount_empty_rooms(server: &MockServer) -> Mock<'_> {
    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list-ids");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"qualified_conversations":[],"has_more":false,"paging_state":"END"}"#);
    })
}

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

const TOO_MANY_REQUESTS: &str =
    r#"{"code":429,"label":"too-many-requests","message":"Please try again later."}"#;

fn polling(poll_interval: Option<Duration>) -> SyncSettings {
    SyncSettings {
        poll_interval,
        transport: SyncTransport::Polling,
    }
}

#[tokio::test]
async fn an_unset_poll_interval_pauses_between_empty_drains() {
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    mount_empty_rooms(&server);
    mount_healthy_key_packages(&server);
    let (client, _store) = common::run_login(&server).await;
    let drains = server.mock(|when, then| {
        when.method(GET).path("/v14/notifications");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"notifications":[],"has_more":false}"#);
    });

    let run = tokio::time::timeout(Duration::from_millis(1500), client.sync(polling(None))).await;

    assert!(run.is_err(), "the loop keeps running: {run:?}");
    assert_eq!(drains.calls(), 1, "one drain, then the 10 s default pause");
}

#[tokio::test]
async fn a_rate_limited_drain_is_waited_out() {
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    mount_empty_rooms(&server);
    mount_healthy_key_packages(&server);
    let (client, _store) = common::run_login(&server).await;
    let drains = server.mock(|when, then| {
        when.method(GET).path("/v14/notifications");
        then.status(429)
            .header("content-type", "application/json")
            .header("retry-after", "1")
            .body(TOO_MANY_REQUESTS);
    });

    let settings = polling(Some(Duration::from_millis(1)));
    let run = tokio::time::timeout(Duration::from_millis(2500), client.sync(settings)).await;

    assert!(run.is_err(), "a 429 does not end the loop: {run:?}");
    let calls = drains.calls();
    assert!(
        (2..=3).contains(&calls),
        "retried after `Retry-After: 1`, not at once: {calls} drains in 2.5 s"
    );
}

#[tokio::test]
async fn a_rate_limited_push_refresh_is_waited_out() {
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    let mut rooms = mount_empty_rooms(&server);
    mount_healthy_key_packages(&server);
    let (client, _store) = common::run_login(&server).await;
    rooms.delete();
    let refreshes = server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list-ids");
        then.status(429)
            .header("content-type", "application/json")
            .header("retry-after", "1")
            .body(TOO_MANY_REQUESTS);
    });

    let settings = SyncSettings {
        poll_interval: Some(Duration::from_millis(1)),
        transport: SyncTransport::WebSocket,
    };
    let run = tokio::time::timeout(Duration::from_millis(2500), client.sync(settings)).await;

    assert!(run.is_err(), "a 429 does not end the push loop: {run:?}");
    let calls = refreshes.calls();
    assert!(
        (2..=3).contains(&calls),
        "retried after `Retry-After: 1`, not at once: {calls} refreshes in 2.5 s"
    );
}
