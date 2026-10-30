// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 10e — typed getters layered over Step 10c's raw accessors.
//!
//! `Client::file_sharing()` projects `fileSharing` into the
//! `FileSharingPolicy` enum (`Disabled` / `Enabled`);
//! `Client::receipt_mode()` projects
//! `WIRE_RECEIPT_MODE` into `ReceiptMode` (`Off` / `On`). Both return
//! the safest default (`Enabled`, `On`) when the underlying entry
//! is missing — see ADR-0012 §Decision.
//!
//! Asserts the live-shaped Wire fixture from the 2026-05-08 smoke
//! pass: `fileSharing: enabled, config: {}` → `Enabled`;
//! `WIRE_RECEIPT_MODE: 1` → `On`.

use httpmock::Method::GET;
use httpmock::MockServer;
use kupferdraht::{FileSharingPolicy, ReceiptMode};

mod common;

#[tokio::test]
async fn file_sharing_disabled_status_yields_disabled() {
    let server = MockServer::start();
    mount_login_with_feature_configs(&server, r#"{"fileSharing":{"status":"disabled"}}"#);
    let (client, _) = common::run_login(&server).await;
    assert!(matches!(
        client.file_sharing().await.expect("file_sharing"),
        FileSharingPolicy::Disabled
    ));
}

#[tokio::test]
async fn file_sharing_enabled_with_empty_config_yields_enabled_all() {
    // Live Wire v14 backend fixture observed 2026-05-08:
    // `fileSharing: { status: "enabled", config: {} }`. No
    // config object → the policy stays `Enabled`.
    let server = MockServer::start();
    mount_login_with_feature_configs(
        &server,
        r#"{"fileSharing":{"status":"enabled","config":{}}}"#,
    );
    let (client, _) = common::run_login(&server).await;
    assert!(matches!(
        client.file_sharing().await.expect("file_sharing"),
        FileSharingPolicy::Enabled
    ));
}

#[tokio::test]
async fn file_sharing_enabled_ignores_any_config_object() {
    // Wire's `fileSharing` feature carries no `config` the client acts
    // on; whatever a backend puts there must not change the policy.
    let server = MockServer::start();
    mount_login_with_feature_configs(
        &server,
        r#"{"fileSharing":{"status":"enabled","config":{"someFutureField":true}}}"#,
    );
    let (client, _) = common::run_login(&server).await;
    assert!(matches!(
        client.file_sharing().await.expect("file_sharing"),
        FileSharingPolicy::Enabled
    ));
}

#[tokio::test]
async fn file_sharing_missing_entry_defaults_to_enabled_all() {
    // No fileSharing entry in the team config — Kupferdraht falls back to
    // Enabled per ADR-0012 §Decision (safest default: permit; let
    // any later 403 surface in the action's error path).
    let server = MockServer::start();
    mount_login_with_feature_configs(&server, "{}");
    let (client, _) = common::run_login(&server).await;
    assert!(matches!(
        client.file_sharing().await.expect("file_sharing"),
        FileSharingPolicy::Enabled
    ));
}

#[tokio::test]
async fn receipt_mode_zero_yields_off() {
    let server = MockServer::start();
    mount_login_with_property(&server, "WIRE_RECEIPT_MODE", "0");
    let (client, _) = common::run_login(&server).await;
    assert!(matches!(
        client.receipt_mode().await.expect("receipt_mode"),
        ReceiptMode::Off
    ));
}

#[tokio::test]
async fn receipt_mode_one_yields_on() {
    let server = MockServer::start();
    mount_login_with_property(&server, "WIRE_RECEIPT_MODE", "1");
    let (client, _) = common::run_login(&server).await;
    assert!(matches!(
        client.receipt_mode().await.expect("receipt_mode"),
        ReceiptMode::On
    ));
}

#[tokio::test]
async fn receipt_mode_quoted_value_strips_quotes() {
    // Some Wire clients write the property as a JSON-encoded string
    // (`"\"0\""` rather than `"0"`). Kupferdraht tolerates both shapes.
    let server = MockServer::start();
    mount_login_with_property(&server, "WIRE_RECEIPT_MODE", r#""0""#);
    let (client, _) = common::run_login(&server).await;
    assert!(matches!(
        client.receipt_mode().await.expect("receipt_mode"),
        ReceiptMode::Off
    ));
}

#[tokio::test]
async fn receipt_mode_missing_entry_defaults_to_on() {
    let server = MockServer::start();
    mount_login_with_no_properties(&server);
    let (client, _) = common::run_login(&server).await;
    assert!(matches!(
        client.receipt_mode().await.expect("receipt_mode"),
        ReceiptMode::On
    ));
}

// ---- fixture helpers ------------------------------------------------

fn mount_login_with_feature_configs(server: &MockServer, body: &str) {
    common::mount_full_login_flow(server);
    server.mock(|when, then| {
        when.method(GET).path("/v14/feature-configs");
        then.status(200)
            .header("content-type", "application/json")
            .body(body);
    });
    server.mock(|when, then| {
        when.method(GET).path("/v14/properties");
        then.status(200).body("[]");
    });
}

fn mount_login_with_property(server: &MockServer, key: &str, value: &str) {
    common::mount_full_login_flow(server);
    server.mock(|when, then| {
        when.method(GET).path("/v14/feature-configs");
        then.status(200).body("{}");
    });
    server.mock(|when, then| {
        when.method(GET).path("/v14/properties");
        then.status(200).body(format!(r#"["{key}"]"#));
    });
    server.mock(|when, then| {
        when.method(GET).path(format!("/v14/properties/{key}"));
        then.status(200).body(value);
    });
}

fn mount_login_with_no_properties(server: &MockServer) {
    common::mount_full_login_flow(server);
    server.mock(|when, then| {
        when.method(GET).path("/v14/feature-configs");
        then.status(200).body("{}");
    });
    server.mock(|when, then| {
        when.method(GET).path("/v14/properties");
        then.status(200).body("[]");
    });
}
