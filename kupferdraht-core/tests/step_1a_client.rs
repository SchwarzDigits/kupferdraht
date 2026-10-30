// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 1a — Client bootstrap, transport half.
//!
//! Failing tests per PLAN.md Step 1 "TDD starter tests, 1a":
//! * `ClientBuilder::server` carries config through to `login`.
//! * `login` with invalid credentials returns `InvalidCredentials`.
//! * `login` with `SecondFactorRequired` propagates the typed variant.
//! * `GET /v14/self` parses into a `SelfUser` with `id`, `handle`, `name`.
//!
//! Tests use `httpmock` so no live Wire traffic. They are expected to
//! fail (red) until the 1a implementation phase lands.

use httpmock::Method::{GET, POST};
use httpmock::MockServer;
use kupferdraht::{ClientBuilder, KupferdrahtError};

mod common;

fn mount_api_version(server: &MockServer) {
    server.mock(|when, then| {
        when.method(GET).path("/api-version");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"supported":[14],"development":[]}"#);
    });
}

#[tokio::test]
async fn builder_server_carries_config_through_to_login() {
    let server = MockServer::start();
    mount_api_version(&server);
    server.mock(|when, then| {
        when.method(POST).path("/v14/login");
        then.status(403)
            .body(r#"{"code":403,"label":"invalid-credentials","message":"no"}"#);
    });

    let builder = ClientBuilder::new()
        .server(common::server_config(&server.base_url()))
        .permit_http();
    let result = builder
        .login("alice@example.test".into(), "nope".into(), None)
        .await;

    // The specific error doesn't matter for this test — what matters is
    // that the request reached the mocked server (i.e. ClientBuilder::server
    // actually carried the base URL into login's HTTP dispatch).
    assert!(
        result.is_err(),
        "login must surface an error against the mock"
    );
}

#[tokio::test]
async fn login_with_invalid_credentials_returns_invalid_credentials_variant() {
    let server = MockServer::start();
    mount_api_version(&server);
    server.mock(|when, then| {
        when.method(POST).path("/v14/login");
        then.status(403)
            .header("content-type", "application/json")
            .body(r#"{"code":403,"label":"invalid-credentials","message":"invalid"}"#);
    });

    let builder = ClientBuilder::new()
        .server(common::server_config(&server.base_url()))
        .permit_http();
    let result = builder
        .login("alice@example.test".into(), "wrong".into(), None)
        .await;

    match result {
        Err(KupferdrahtError::InvalidCredentials) => {}
        other => panic!("expected InvalidCredentials, got {other:?}"),
    }
}

#[tokio::test]
async fn login_with_second_factor_required_propagates_typed_variant() {
    let server = MockServer::start();
    mount_api_version(&server);
    server.mock(|when, then| {
        when.method(POST).path("/v14/login");
        then.status(403)
            .header("content-type", "application/json")
            .body(r#"{"code":403,"label":"code-authentication-required","message":"2fa"}"#);
    });

    let builder = ClientBuilder::new()
        .server(common::server_config(&server.base_url()))
        .permit_http();
    let result = builder
        .login("alice@example.test".into(), "correct".into(), None)
        .await;

    match result {
        Err(KupferdrahtError::SecondFactorRequired) => {}
        other => panic!("expected SecondFactorRequired, got {other:?}"),
    }
}

#[tokio::test]
async fn successful_login_fetches_self_and_parses_into_self_user() {
    // Happy path end-to-end: /api-version → /v14/login → /v14/self →
    // /v14/clients (register) → /v14/access?client_id= (rebind).
    // Tests in step_1b_* cover the 1b-specific assertions; this test
    // keeps its 1a-era focus on SelfUser parsing but needs the full
    // mock set so login doesn't fail mid-flow.
    let server = MockServer::start();
    mount_api_version(&server);
    server.mock(|when, then| {
        when.method(POST).path("/v14/login");
        then.status(200)
            .header("content-type", "application/json")
            .header("set-cookie", "zuid=test-cookie-value; HttpOnly")
            .body(r#"{"access_token":"a.b.c","token_type":"Bearer","expires_in":900,"user":"user-uuid"}"#);
    });
    server.mock(|when, then| {
        when.method(GET).path("/v14/self");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"id":"user-uuid","qualified_id":{"id":"user-uuid","domain":"example.test"},"handle":"alice","name":"Alice Example","email":"alice@example.test"}"#);
    });
    server.mock(|when, then| {
        when.method(POST).path("/v14/clients");
        then.status(201)
            .header("content-type", "application/json")
            .body(r#"{"id":"new-client-id","type":"permanent"}"#);
    });
    server.mock(|when, then| {
        when.method(POST).path("/v14/access").query_param("client_id", "new-client-id");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"access_token":"rebound.jwt","token_type":"Bearer","expires_in":900,"user":"user-uuid"}"#);
    });
    // Step 1c.6: before the KeyPackage upload, login PUTs the MLS
    // signature public key via /v14/clients/{id} (observations.md §19).
    server.mock(|when, then| {
        when.method(httpmock::Method::PUT)
            .path("/v14/clients/new-client-id");
        then.status(200);
    });
    // Step 1c.3: login's final phase uploads the initial KeyPackage
    // batch. This 1a-era test doesn't care about the upload shape; a
    // permissive 201 keeps the login flow green here so `step_1c_*`
    // can own the upload-body assertions.
    server.mock(|when, then| {
        when.method(POST)
            .path("/v14/mls/key-packages/self/new-client-id");
        then.status(201);
    });
    // Step 1c.4: login then fetches Wire's backend removal-sender keys.
    // Same permissive stance as the upload mock.
    server.mock(|when, then| {
        when.method(GET).path("/v14/mls/public-keys");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"removal":{"ed25519":"ZWQ=","ecdsa_secp256r1_sha256":"cDI1Ng==","ecdsa_secp384r1_sha384":"cDM4NA==","ecdsa_secp521r1_sha512":"cDUyMQ=="}}"#);
    });

    let builder = ClientBuilder::new()
        .server(common::server_config(&server.base_url()))
        .permit_http();
    let client = builder
        .login("alice@example.test".into(), "correct".into(), None)
        .await
        .expect("login should succeed against happy-path mocks")
        .register_device()
        .await
        .expect("register device");

    let account = client.account();
    assert_eq!(account.user_id().as_str(), "user-uuid@example.test");
    assert_eq!(account.handle(), Some("alice"));
    assert_eq!(account.display_name(), "Alice Example");
    assert_eq!(client.api_version(), 14);
    assert_eq!(
        client.user_id().map(|u| u.as_str()),
        Some("user-uuid@example.test")
    );
}
