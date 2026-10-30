// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 63 — key-package pool replenishment (TDD red phase).
//!
//! Pins `docs/plan/63-key-package-replenishment.md` §TDD list: the
//! pool self-heals via `GET …/key-packages/self/{client}/count` +
//! the existing upload path. Thresholds: below `MIN` (50 = half of
//! `INITIAL_KEY_PACKAGE_COUNT`) the pool is topped back up to 100;
//! at or above `MIN` nothing happens. The automatic check rides the
//! sync-surface entry points, throttled to once per 24 h, and must
//! never break sync (warn-and-continue).

use httpmock::Method::{GET, POST};
use httpmock::MockServer;
use serde_json::Value;

mod common;

/// Mock the count endpoint (`GET /v14/mls/key-packages/self/{id}/count`).
/// Pins the `ciphersuite` query param (decimal 2 = P-256) — current
/// wire-server 400s without it (live-observed 2026-08-13).
fn mount_key_package_count(server: &MockServer, count: u32) -> httpmock::Mock<'_> {
    server.mock(move |when, then| {
        when.method(GET)
            .path_includes("/mls/key-packages/self/")
            .path_includes("/count")
            .query_param("ciphersuite", "2");
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(r#"{{"count":{count}}}"#));
    })
}

/// Mock the upload endpoint, matching only a body of exactly 60
/// packages (the 100→40 top-up difference; httpmock 0.7 matchers are
/// plain `fn` pointers, so the count is fixed rather than captured).
/// Mount BEFORE `mount_full_login_flow` — httpmock consults mocks in
/// registration order, and the login flow's generic key-packages mock
/// would otherwise swallow the replenishment upload; the login's own
/// 100-package body does not match this 60-matcher and falls through.
fn mount_replenishment_upload_of_60(server: &MockServer) -> httpmock::Mock<'_> {
    server.mock(|when, then| {
        when.method(POST)
            .path_includes("/mls/key-packages/self/")
            .is_true(|req| {
                serde_json::from_slice::<Value>(req.body_ref())
                    .ok()
                    .and_then(|v| v.get("key_packages").cloned())
                    .and_then(|kps| kps.as_array().map(|a| a.len()))
                    == Some(60)
            });
        then.status(201);
    })
}

/// Empty-rooms + empty-queue mocks so `sync_once` runs with minimal
/// ceremony (`list-ids` empty → the conversations `list` call is
/// skipped entirely).
fn mount_empty_sync(server: &MockServer) {
    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list-ids");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"qualified_conversations":[],"has_more":false,"paging_state":"END"}"#);
    });
    server.mock(|when, then| {
        when.method(GET).path("/v14/notifications");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"notifications":[],"has_more":false}"#);
    });
}

#[tokio::test]
async fn replenish_uploads_the_difference_when_pool_low() {
    let server = MockServer::start();
    let upload = mount_replenishment_upload_of_60(&server);
    common::mount_full_login_flow(&server);
    let (client, _store) = common::run_login(&server).await;
    let count = mount_key_package_count(&server, 40);

    let uploaded = client
        .replenish_key_packages()
        .await
        .expect("replenish succeeds");
    assert_eq!(uploaded, 60, "40 on the server → top up to 100");
    assert_eq!(count.calls(), 1);
    assert_eq!(upload.calls(), 1, "exactly one upload of exactly 60");
}

#[tokio::test]
async fn replenish_skips_upload_when_pool_at_min() {
    // Boundary pin: MIN itself (50) is healthy — only *below* MIN
    // triggers the top-up.
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    let (client, _store) = common::run_login(&server).await;
    let count = mount_key_package_count(&server, 50);

    let uploaded = client
        .replenish_key_packages()
        .await
        .expect("replenish succeeds");
    assert_eq!(uploaded, 0, "pool at MIN needs no upload");
    assert_eq!(count.calls(), 1);
}

#[tokio::test]
async fn sync_once_triggers_the_check_at_most_once_per_day() {
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    mount_empty_sync(&server);
    let (client, _store) = common::run_login(&server).await;
    let count = mount_key_package_count(&server, 100);

    client
        .sync_once(kupferdraht::SyncSettings::default())
        .await
        .expect("first sync_once");
    client
        .sync_once(kupferdraht::SyncSettings::default())
        .await
        .expect("second sync_once");

    assert_eq!(
        count.calls(),
        1,
        "the auto-check runs on the first sync entry and is then \
         throttled (24 h stamp) — not once per iteration"
    );
}

#[tokio::test]
async fn count_endpoint_failure_does_not_break_sync() {
    // Warn-and-continue guard-rail (green already in the red phase —
    // it pins that wiring the hook must not couple sync success to
    // the key-package check).
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    mount_empty_sync(&server);
    let (client, _store) = common::run_login(&server).await;
    server.mock(|when, then| {
        when.method(GET)
            .path_includes("/mls/key-packages/self/")
            .path_includes("/count");
        then.status(500);
    });

    client
        .sync_once(kupferdraht::SyncSettings::default())
        .await
        .expect("a failing key-package check must never break sync");
}
