// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 10d — inbound event dispatch for the three notification
//! types ADR-0012 reacts to: `feature-config.update`,
//! `user.properties-set`, `user.properties-delete`.
//!
//! Drives `SyncService::drain_pending` against an httpmock-backed
//! Wire stand-in. Asserts that each event triggers the documented
//! re-fetch / upsert / local-delete behavior on the V18 tables.
//! Errors fold into `on_sync_error` per the existing 6-II pattern;
//! the drain continues regardless.

use httpmock::Method::{GET, POST};
use httpmock::MockServer;

mod common;

#[tokio::test]
async fn feature_config_update_event_triggers_refetch_and_replace() {
    // Wire's `feature-config.update` event ships without the new
    // value — the dispatcher must re-fetch `/v14/feature-configs`
    // (the whole-team response, since v14 has no per-feature read
    // endpoint) and replace the persisted snapshot.
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    // sync_once includes a conversations-metadata refresh. These
    // tests don't care about rooms; mount an empty list-ids so
    // sync_once's first phase returns 0 rooms.
    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list-ids");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"qualified_conversations":[],"has_more":false,"paging_state":"END"}"#);
    });
    let fc_mock = server.mock(|when, then| {
        when.method(GET).path("/v14/feature-configs");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"fileSharing":{"status":"enabled","config":{}},"mls":{"status":"enabled","config":{}}}"#);
    });
    server.mock(|when, then| {
        when.method(GET).path("/v14/properties");
        then.status(200)
            .header("content-type", "application/json")
            .body("[]");
    });
    let notifications_mock = server.mock(|when, then| {
        when.method(GET).path("/v14/notifications");
        then.status(200)
            .header("content-type", "application/json")
            .body(
                r#"{"notifications":[{"id":"notif-1","payload":[{"type":"feature-config.update"}]}],"has_more":false}"#,
            );
    });

    let (client, store) = common::run_login(&server).await;

    let initial = store.list_feature_configs().await.expect("initial list");
    assert_eq!(initial.len(), 2, "login populated 2 entries");
    fc_mock.assert_calls(1);

    client
        .sync_once(kupferdraht::SyncSettings::default())
        .await
        .expect("sync_once");

    fc_mock.assert_calls(2);
    notifications_mock.assert_calls(1);
    let updated = store
        .list_feature_configs()
        .await
        .expect("post-update list");
    assert_eq!(
        updated.len(),
        2,
        "table replaced via dispatch refetch (same body, but proves the path executed)"
    );
}

#[tokio::test]
async fn user_properties_set_event_refetches_value_and_upserts_row() {
    // Wire's `user.properties-set` event carries the affected `key`
    // but not the new value — the dispatcher must re-fetch
    // `/v14/properties/{key}` and upsert the row.
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    // sync_once includes a conversations-metadata refresh. These
    // tests don't care about rooms; mount an empty list-ids so
    // sync_once's first phase returns 0 rooms.
    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list-ids");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"qualified_conversations":[],"has_more":false,"paging_state":"END"}"#);
    });
    server.mock(|when, then| {
        when.method(GET).path("/v14/feature-configs");
        then.status(200)
            .header("content-type", "application/json")
            .body("{}");
    });
    server.mock(|when, then| {
        when.method(GET).path("/v14/properties");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"["WIRE_RECEIPT_MODE"]"#);
    });
    let pv_mock = server.mock(|when, then| {
        when.method(GET).path("/v14/properties/WIRE_RECEIPT_MODE");
        then.status(200)
            .header("content-type", "application/json")
            .body("1");
    });
    server.mock(|when, then| {
        when.method(GET).path("/v14/notifications");
        then.status(200)
            .header("content-type", "application/json")
            .body(
                r#"{"notifications":[{"id":"notif-2","payload":[{"type":"user.properties-set","key":"WIRE_RECEIPT_MODE"}]}],"has_more":false}"#,
            );
    });

    let (client, store) = common::run_login(&server).await;
    let initial = store
        .get_user_property("WIRE_RECEIPT_MODE")
        .await
        .expect("initial");
    assert_eq!(initial.expect("present").value, "1");

    client
        .sync_once(kupferdraht::SyncSettings::default())
        .await
        .expect("sync_once");

    // 1 hit from login fetch_property_value, 1 hit from event refetch.
    pv_mock.assert_calls(2);
    let after = store
        .get_user_property("WIRE_RECEIPT_MODE")
        .await
        .expect("after")
        .expect("still present");
    assert_eq!(after.value, "1");
}

#[tokio::test]
async fn user_properties_delete_event_removes_local_row_without_wire_delete() {
    // ADR-0012 §Out of scope: the event handler MUST NOT call Wire's
    // `DELETE /v14/properties/{key}` — Wire is the source of truth
    // for the deletion in the first place; calling DELETE again
    // would re-trigger the same event. Mock only the GET endpoints;
    // an accidental DELETE would 404 (httpmock default), which would
    // surface as `BackendRejected` from the dispatcher and fail the
    // drain.
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    // sync_once includes a conversations-metadata refresh. These
    // tests don't care about rooms; mount an empty list-ids so
    // sync_once's first phase returns 0 rooms.
    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list-ids");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"qualified_conversations":[],"has_more":false,"paging_state":"END"}"#);
    });
    server.mock(|when, then| {
        when.method(GET).path("/v14/feature-configs");
        then.status(200)
            .header("content-type", "application/json")
            .body("{}");
    });
    server.mock(|when, then| {
        when.method(GET).path("/v14/properties");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"["WIRE_RECEIPT_MODE"]"#);
    });
    server.mock(|when, then| {
        when.method(GET).path("/v14/properties/WIRE_RECEIPT_MODE");
        then.status(200)
            .header("content-type", "application/json")
            .body("1");
    });
    server.mock(|when, then| {
        when.method(GET).path("/v14/notifications");
        then.status(200)
            .header("content-type", "application/json")
            .body(
                r#"{"notifications":[{"id":"notif-3","payload":[{"type":"user.properties-delete","key":"WIRE_RECEIPT_MODE"}]}],"has_more":false}"#,
            );
    });

    let (client, store) = common::run_login(&server).await;
    let initial = store.list_user_properties().await.expect("initial");
    assert_eq!(initial.len(), 1);

    client
        .sync_once(kupferdraht::SyncSettings::default())
        .await
        .expect("sync_once");

    let after = store.list_user_properties().await.expect("after");
    assert!(
        after.is_empty(),
        "user.properties-delete must wipe the local row; got {after:?}"
    );
}
