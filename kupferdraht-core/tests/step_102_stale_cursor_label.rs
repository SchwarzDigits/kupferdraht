// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 102 — reset the cursor only on the backend's `not-found`
//! (`docs/plan/archive/102-stale-cursor-label.md`, review finding H3).
//!
//! A `404` that is not the backend's labelled answer to a `since` it no
//! longer holds — a proxy's error page — must surface as an error and
//! leave the persisted cursor alone. The labelled reset itself stays
//! pinned by `step_64_sync_robustness::drain_recovers_from_stale_cursor_404`.

use httpmock::Method::{GET, POST};
use httpmock::MockServer;
use kupferdraht::storage::SyncStateRecord;

mod common;

fn mount_empty_rooms(server: &MockServer) {
    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list-ids");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"qualified_conversations":[],"has_more":false,"paging_state":"END"}"#);
    });
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
async fn a_proxy_404_keeps_the_cursor() {
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    mount_empty_rooms(&server);
    mount_healthy_key_packages(&server);
    let (client, store) = common::run_login(&server).await;
    seed_cursor(&store, "kept-cursor").await;

    server.mock(|when, then| {
        when.method(GET)
            .path("/v14/notifications")
            .query_param("since", "kept-cursor");
        then.status(404)
            .header("content-type", "text/html")
            .body("<html><body><h1>404 Not Found</h1><hr>nginx</body></html>");
    });
    let from_head = server.mock(|when, then| {
        when.method(GET)
            .path("/v14/notifications")
            .is_true(|req| req.query_params().iter().all(|(k, _)| k != "since"));
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"notifications":[],"has_more":false}"#);
    });

    let result = client.sync_once(kupferdraht::SyncSettings::default()).await;

    assert!(
        matches!(
            result,
            Err(kupferdraht::KupferdrahtError::BackendRejected(_))
        ),
        "a 404 that is not the backend's `not-found` surfaces: {result:?}"
    );
    assert_eq!(from_head.calls(), 0, "no refetch from the queue head");
    assert_eq!(
        persisted_cursor(&store).await.as_deref(),
        Some("kept-cursor"),
        "the cursor survives a proxy's 404"
    );
}
