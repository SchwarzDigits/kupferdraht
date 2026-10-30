// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 1c.4 — Fetch Wire's backend external-sender public keys at
//! login time and expose the ECDSA-P256 `removal` entry on `Client`.
//!
//! Failing tests per PLAN.md Step 1 "TDD starter tests, 1c":
//!
//! * The login flow invokes `GET /v14/mls/public-keys` exactly once
//!   with the rebound access token.
//! * `Client::backend_removal_key_ecdsa_p256` returns the raw bytes
//!   (base64-decoded from the `removal.ecdsa_secp256r1_sha256` entry).
//! * A malformed base64 in the response surfaces as
//!   `KupferdrahtError::Encoding`.
//! * A non-2xx response from `/mls/public-keys` aborts the login with
//!   `BackendRejected`.
//!
//! Tests use `httpmock` — no live Wire traffic.

use httpmock::Method::{GET, POST, PUT};
use httpmock::MockServer;
use kupferdraht::{ClientBuilder, KupferdrahtError};

mod common;

/// Mount everything up through `POST /v14/mls/key-packages/self/...`.
/// Each test composes this with a test-specific `/mls/public-keys`
/// mock.
fn mount_login_through_key_package_upload(server: &MockServer) {
    server.mock(|when, then| {
        when.method(GET).path("/api-version");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"supported":[14],"development":[]}"#);
    });
    server.mock(|when, then| {
        when.method(POST).path("/v14/login");
        then.status(200)
            .header("content-type", "application/json")
            .header("set-cookie", "zuid=test; HttpOnly; Secure; Path=/access")
            .body(r#"{"access_token":"initial.jwt.token","token_type":"Bearer","expires_in":900,"user":"user-uuid"}"#);
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
        when.method(POST)
            .path("/v14/access")
            .query_param("client_id", "new-client-id")
            .header("cookie", "zuid=test");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"access_token":"rebound.jwt.token","token_type":"Bearer","expires_in":900,"user":"user-uuid"}"#);
    });
    server.mock(|when, then| {
        when.method(PUT).path("/v14/clients/new-client-id");
        then.status(200);
    });
    server.mock(|when, then| {
        when.method(POST)
            .path("/v14/mls/key-packages/self/new-client-id");
        then.status(201);
    });
}

/// Valid base64 that decodes to 0x70 0x32 0x35 0x36 (ASCII "p256") —
/// used as the canonical ECDSA-P256 removal-key value in the mocks.
const P256_B64: &str = "cDI1Ng==";
const P256_BYTES: &[u8] = b"p256";

#[tokio::test]
async fn login_fetches_backend_public_keys_with_rebound_bearer() {
    let server = MockServer::start();
    mount_login_through_key_package_upload(&server);
    let public_keys = server.mock(|when, then| {
        when.method(GET)
            .path("/v14/mls/public-keys")
            .header("authorization", "Bearer rebound.jwt.token");
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(
                r#"{{"removal":{{"ed25519":"ZWQ=","ecdsa_secp256r1_sha256":"{P256_B64}","ecdsa_secp384r1_sha384":"cDM4NA==","ecdsa_secp521r1_sha512":"cDUyMQ=="}}}}"#
            ));
    });

    let builder = ClientBuilder::new()
        .server(common::server_config(&server.base_url()))
        .permit_http();
    let _client = builder
        .login("alice@example.test".into(), "pw".into(), None)
        .await
        .expect("login must complete through the backend-keys phase")
        .register_device()
        .await
        .expect("register device");

    public_keys.assert_calls(1);
}

#[tokio::test]
async fn client_exposes_ecdsa_p256_removal_key_from_backend() {
    let server = MockServer::start();
    mount_login_through_key_package_upload(&server);
    server.mock(|when, then| {
        when.method(GET).path("/v14/mls/public-keys");
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(
                r#"{{"removal":{{"ed25519":"ZWQ=","ecdsa_secp256r1_sha256":"{P256_B64}","ecdsa_secp384r1_sha384":"cDM4NA==","ecdsa_secp521r1_sha512":"cDUyMQ=="}}}}"#
            ));
    });

    let builder = ClientBuilder::new()
        .server(common::server_config(&server.base_url()))
        .permit_http();
    let client = builder
        .login("alice@example.test".into(), "pw".into(), None)
        .await
        .expect("login")
        .register_device()
        .await
        .expect("register device");

    assert_eq!(
        client.backend_removal_key_ecdsa_p256(),
        P256_BYTES,
        "Client must expose the base64-decoded ECDSA-P256 removal key"
    );
}

#[tokio::test]
async fn malformed_base64_in_removal_key_surfaces_as_encoding_error() {
    let server = MockServer::start();
    mount_login_through_key_package_upload(&server);
    server.mock(|when, then| {
        when.method(GET).path("/v14/mls/public-keys");
        then.status(200)
            .header("content-type", "application/json")
            // `!!!` is invalid base64.
            .body(r#"{"removal":{"ed25519":"ZWQ=","ecdsa_secp256r1_sha256":"!!!","ecdsa_secp384r1_sha384":"cDM4NA==","ecdsa_secp521r1_sha512":"cDUyMQ=="}}"#);
    });

    let builder = ClientBuilder::new()
        .server(common::server_config(&server.base_url()))
        .permit_http();
    let pending = builder
        .login("alice@example.test".into(), "pw".into(), None)
        .await
        .expect("phase 1 (auth) should succeed");
    let err = pending
        .register_device()
        .await
        .expect_err("malformed base64 must abort device registration");
    assert!(
        matches!(err, KupferdrahtError::Encoding(_)),
        "expected Encoding, got {err:?}"
    );
}

#[tokio::test]
async fn login_fails_when_public_keys_endpoint_returns_server_error() {
    let server = MockServer::start();
    mount_login_through_key_package_upload(&server);
    server.mock(|when, then| {
        when.method(GET).path("/v14/mls/public-keys");
        then.status(500)
            .header("content-type", "application/json")
            .body(r#"{"code":500,"label":"server-error","message":"backend unhappy"}"#);
    });

    let builder = ClientBuilder::new()
        .server(common::server_config(&server.base_url()))
        .permit_http();
    let pending = builder
        .login("alice@example.test".into(), "pw".into(), None)
        .await
        .expect("phase 1 (auth) should succeed");
    let err = pending
        .register_device()
        .await
        .expect_err("5xx from /mls/public-keys must abort device registration");
    assert!(
        matches!(err, KupferdrahtError::BackendRejected(_)),
        "expected BackendRejected, got {err:?}"
    );
}
