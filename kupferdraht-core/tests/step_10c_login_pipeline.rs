// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 10c — login pipeline integration for feature-configs +
//! user-properties.
//!
//! Drives `ClientBuilder::login` against an httpmock-backed Wire
//! stand-in and asserts that the new `LoginPipeline::fetch_server_configs`
//! phase persists `/v14/feature-configs` + `/v14/properties[/{key}]`
//! into the V18 tables. The error-posture is **warn-and-continue**
//! per ADR-0012 §Login pipeline integration: a 5xx on either surface
//! must not block login. The four tests below cover the matrix from
//! `docs/plan/10-server-feature-configs.md` §10c TDD checklist plus
//! the per-key partial-failure case the inner loop is built for.

use std::sync::Arc;

use httpmock::Method::GET;
use httpmock::MockServer;
use kupferdraht::storage::Store;

mod common;

#[tokio::test]
async fn login_persists_feature_configs_and_user_properties_on_success() {
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    server.mock(|when, then| {
        when.method(GET)
            .path("/v14/feature-configs")
            .header("authorization", "Bearer rebound.jwt.token");
        then.status(200)
            .header("content-type", "application/json")
            .body(representative_feature_configs_body());
    });
    server.mock(|when, then| {
        when.method(GET)
            .path("/v14/properties")
            .header("authorization", "Bearer rebound.jwt.token");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"["WIRE_RECEIPT_MODE","labels"]"#);
    });
    server.mock(|when, then| {
        when.method(GET)
            .path("/v14/properties/WIRE_RECEIPT_MODE")
            .header("authorization", "Bearer rebound.jwt.token");
        then.status(200)
            .header("content-type", "application/json")
            .body("1");
    });
    server.mock(|when, then| {
        when.method(GET)
            .path("/v14/properties/labels")
            .header("authorization", "Bearer rebound.jwt.token");
        then.status(200)
            .header("content-type", "application/json")
            .body("[]");
    });

    let store = run_login_against(&server).await;

    let configs = store
        .list_feature_configs()
        .await
        .expect("list_feature_configs");
    assert_eq!(
        configs.len(),
        3,
        "all three representative-fixture features must persist"
    );
    let by_name: std::collections::HashMap<_, _> =
        configs.into_iter().map(|e| (e.name.clone(), e)).collect();
    assert_eq!(
        by_name["fileSharing"].status.as_deref(),
        Some("enabled"),
        "fileSharing.status round-trips"
    );
    assert!(
        by_name["selfDeletingMessages"]
            .config_json
            .contains("enforcedTimeoutSeconds"),
        "a feature's config payload round-trips"
    );
    assert_eq!(
        by_name["mlsE2EId"].status.as_deref(),
        Some("disabled"),
        "mlsE2EId.status round-trips"
    );

    let properties = store
        .list_user_properties()
        .await
        .expect("list_user_properties");
    assert_eq!(properties.len(), 2, "both property keys must persist");
    let by_key: std::collections::HashMap<_, _> =
        properties.into_iter().map(|e| (e.key.clone(), e)).collect();
    assert_eq!(
        by_key["WIRE_RECEIPT_MODE"].value, "1",
        "WIRE_RECEIPT_MODE value persisted verbatim"
    );
    assert_eq!(
        by_key["labels"].value, "[]",
        "labels value persisted verbatim"
    );
}

#[tokio::test]
async fn login_succeeds_when_feature_configs_endpoint_returns_500() {
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    server.mock(|when, then| {
        when.method(GET).path("/v14/feature-configs");
        then.status(500)
            .header("content-type", "application/json")
            .body(r#"{"code":500,"label":"server-error","message":"transient"}"#);
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

    let store = run_login_against(&server).await;

    assert!(
        store
            .list_feature_configs()
            .await
            .expect("list_feature_configs")
            .is_empty(),
        "feature-configs 500 must leave the table untouched (warn-and-continue)"
    );
    let properties = store
        .list_user_properties()
        .await
        .expect("list_user_properties");
    assert_eq!(
        properties.len(),
        1,
        "properties path must still persist when /feature-configs failed"
    );
    assert_eq!(properties[0].key, "WIRE_RECEIPT_MODE");
}

#[tokio::test]
async fn login_succeeds_when_property_keys_endpoint_returns_500() {
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    server.mock(|when, then| {
        when.method(GET).path("/v14/feature-configs");
        then.status(200)
            .header("content-type", "application/json")
            .body(representative_feature_configs_body());
    });
    server.mock(|when, then| {
        when.method(GET).path("/v14/properties");
        then.status(500)
            .header("content-type", "application/json")
            .body(r#"{"code":500,"label":"server-error","message":"transient"}"#);
    });

    let store = run_login_against(&server).await;

    let configs = store
        .list_feature_configs()
        .await
        .expect("list_feature_configs");
    assert_eq!(
        configs.len(),
        3,
        "feature-configs path must still persist when /properties failed"
    );
    assert!(
        store
            .list_user_properties()
            .await
            .expect("list_user_properties")
            .is_empty(),
        "/properties 500 must leave the user-properties table untouched"
    );
}

#[tokio::test]
async fn client_accessors_surface_persisted_configs_and_properties() {
    // Step 10c public-API surface: Client::feature_configs /
    // feature_config / user_properties / user_property delegate to
    // the persisted V18 tables. Same login fixture as test #1; this
    // test covers the read path through the ready Client.
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    server.mock(|when, then| {
        when.method(GET).path("/v14/feature-configs");
        then.status(200)
            .header("content-type", "application/json")
            .body(representative_feature_configs_body());
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

    let (client, _) = common::run_login(&server).await;

    let configs = client.feature_configs().await.expect("feature_configs");
    assert_eq!(configs.len(), 3);
    let one = client
        .feature_config("fileSharing")
        .await
        .expect("feature_config")
        .expect("fileSharing must be present");
    assert_eq!(one.name, "fileSharing");
    assert_eq!(one.status.as_deref(), Some("enabled"));

    let missing = client
        .feature_config("doesNotExist")
        .await
        .expect("feature_config none");
    assert!(missing.is_none());

    let properties = client.user_properties().await.expect("user_properties");
    assert_eq!(properties.len(), 1);
    let one = client
        .user_property("WIRE_RECEIPT_MODE")
        .await
        .expect("user_property")
        .expect("WIRE_RECEIPT_MODE must be present");
    assert_eq!(one.value, "1");
}

#[tokio::test]
async fn login_persists_partial_properties_when_one_value_fetch_fails() {
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    server.mock(|when, then| {
        when.method(GET).path("/v14/feature-configs");
        then.status(200)
            .header("content-type", "application/json")
            .body(representative_feature_configs_body());
    });
    server.mock(|when, then| {
        when.method(GET).path("/v14/properties");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"["WIRE_RECEIPT_MODE","labels"]"#);
    });
    server.mock(|when, then| {
        when.method(GET).path("/v14/properties/WIRE_RECEIPT_MODE");
        then.status(200)
            .header("content-type", "application/json")
            .body("1");
    });
    // `labels` value-fetch fails — the inner loop must skip this key
    // and still persist whatever else succeeded.
    server.mock(|when, then| {
        when.method(GET).path("/v14/properties/labels");
        then.status(500)
            .header("content-type", "application/json")
            .body(r#"{"code":500,"label":"server-error"}"#);
    });

    let store = run_login_against(&server).await;

    let properties = store
        .list_user_properties()
        .await
        .expect("list_user_properties");
    assert_eq!(
        properties.len(),
        1,
        "only the successfully-fetched key must be persisted"
    );
    assert_eq!(properties[0].key, "WIRE_RECEIPT_MODE");
}

// ---- fixture --------------------------------------------------------

/// Synthesised representative `/v14/feature-configs` body — three
/// entries chosen to cover the corners (full status+config, status-only,
/// status+config). Mirrors `wire::http::feature_configs::tests::
/// representative_feature_configs` but trimmed to three entries to keep
/// the integration assertions short.
fn representative_feature_configs_body() -> String {
    r#"{
        "fileSharing": {
            "status": "enabled",
            "config": {}
        },
        "mlsE2EId": { "status": "disabled" },
        "selfDeletingMessages": {
            "status": "enabled",
            "config": { "enforcedTimeoutSeconds": 0 }
        }
    }"#
    .to_string()
}

async fn run_login_against(server: &MockServer) -> Arc<dyn Store> {
    let (_client, store) = common::run_login(server).await;
    store
}
