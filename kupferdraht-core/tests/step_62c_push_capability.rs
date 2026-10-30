// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 62c — push-capability getter (TDD red phase).
//!
//! `Client::server_supports_push_events()` projects the cached
//! `consumableNotifications` feature config into the transport-choice
//! bool: whether the server's RabbitMQ-backed `/v14/events` delivery
//! is operational (observations §66 — with the flag off, the socket
//! connects but never receives an event). Absent entry → `false`:
//! deliberately the opposite default direction to `file_sharing()`'s
//! permissive one — a wrong `true` silently breaks receive, a wrong
//! `false` merely polls.

use httpmock::Method::GET;
use httpmock::MockServer;

mod common;

fn mount_login_with_feature_configs(server: &MockServer, body: &str) {
    common::mount_full_login_flow(server);
    let body = body.to_string();
    server.mock(move |when, then| {
        when.method(GET).path("/v14/feature-configs");
        then.status(200)
            .header("content-type", "application/json")
            .body(body);
    });
}

#[tokio::test]
async fn push_support_enabled_when_feature_enabled() {
    let server = MockServer::start();
    mount_login_with_feature_configs(
        &server,
        r#"{"consumableNotifications":{"status":"enabled","lockStatus":"unlocked","ttl":"unlimited"}}"#,
    );
    let (client, _) = common::run_login(&server).await;
    assert!(
        client
            .server_supports_push_events()
            .await
            .expect("getter reads the cached config"),
        "status enabled must report push support"
    );
}

#[tokio::test]
async fn push_support_disabled_when_feature_disabled() {
    // Verbatim live fixture, self-hosted instance 2026-08-12 — the
    // wire-server default (root Helm flag `consumableNotifications:
    // false` renders as a locked, disabled team feature).
    let server = MockServer::start();
    mount_login_with_feature_configs(
        &server,
        r#"{"consumableNotifications":{"lockStatus":"locked","status":"disabled","ttl":"unlimited"}}"#,
    );
    let (client, _) = common::run_login(&server).await;
    assert!(
        !client
            .server_supports_push_events()
            .await
            .expect("getter reads the cached config"),
        "status disabled must report no push support"
    );
}

#[tokio::test]
async fn push_support_defaults_to_false_when_entry_absent() {
    // A backend that predates consumable notifications never reports
    // the key at all — the conservative default is "poll".
    let server = MockServer::start();
    mount_login_with_feature_configs(&server, r#"{"fileSharing":{"status":"enabled"}}"#);
    let (client, _) = common::run_login(&server).await;
    assert!(
        !client
            .server_supports_push_events()
            .await
            .expect("getter tolerates the absent entry"),
        "absent entry must default to no push support (opposite of file_sharing's permissive default)"
    );
}
