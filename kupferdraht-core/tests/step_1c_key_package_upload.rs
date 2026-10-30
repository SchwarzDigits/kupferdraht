// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 1c.3 — Upload the initial `KeyPackage` batch to
//! `POST /v14/mls/key-packages/self/{client_id}` as the final phase of
//! `ClientBuilder::login`.
//!
//! Failing tests per PLAN.md Step 1 "TDD starter tests, 1c":
//!
//! * The login flow invokes `POST /v14/mls/key-packages/self/<client_id>`
//!   exactly once with a non-empty `key_packages` JSON array carrying
//!   base64-encoded TLS-serialized `KeyPackage`s.
//! * The upload body carries `INITIAL_KEY_PACKAGE_COUNT = 100` entries.
//! * A non-2xx response from the upload endpoint (e.g. `400
//!   mls-protocol-error`) aborts the login with `BackendRejected`.
//!
//! Tests use `httpmock` — no live Wire traffic.

use httpmock::Method::{GET, POST, PUT};
use httpmock::MockServer;
use kupferdraht::{ClientBuilder, KupferdrahtError};

mod common;

/// Mount every endpoint the login flow hits *except* the `/mls/key-packages`
/// upload: api-version, login, self, clients register, access rebind.
/// Each test below composes this with a test-specific upload mock.
fn mount_login_and_registration(server: &MockServer) {
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
    // Step 1c.6: PUT /v14/clients/{id} to register the MLS public
    // key before uploading KeyPackages (observations.md §19).
    server.mock(|when, then| {
        when.method(PUT).path("/v14/clients/new-client-id");
        then.status(200);
    });
    // Step 1c.4: login's last phase fetches the backend removal-sender
    // public keys. Default permissive mock for the upload-focused tests
    // in this file; `step_1c_backend_public_keys.rs` owns the shape
    // assertions.
    server.mock(|when, then| {
        when.method(GET).path("/v14/mls/public-keys");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"removal":{"ed25519":"ZWQ=","ecdsa_secp256r1_sha256":"cDI1Ng==","ecdsa_secp384r1_sha384":"cDM4NA==","ecdsa_secp521r1_sha512":"cDUyMQ=="}}"#);
    });
}

/// Extract the `key_packages` JSON array from a raw POST body. Returns
/// `None` if the body is empty, not UTF-8, not JSON, or missing the
/// field — each of which fails the matcher.
fn key_packages_array(body: &[u8]) -> Option<Vec<serde_json::Value>> {
    let value: serde_json::Value = serde_json::from_slice(body).ok()?;
    value.get("key_packages")?.as_array().cloned()
}

#[tokio::test]
async fn login_uploads_key_package_batch_to_wire() {
    let server = MockServer::start();
    mount_login_and_registration(&server);
    let upload = server.mock(|when, then| {
        when.method(POST)
            .path("/v14/mls/key-packages/self/new-client-id")
            .header_exists("authorization")
            .is_true(|req| {
                let Some(packages) = key_packages_array(req.body_ref()) else {
                    return false;
                };
                !packages.is_empty()
            });
        then.status(201);
    });

    let builder = ClientBuilder::new()
        .server(common::server_config(&server.base_url()))
        .permit_http();
    let _client = builder
        .login("alice@example.test".into(), "pw".into(), None)
        .await
        .expect("login must complete through the KeyPackage upload phase")
        .register_device()
        .await
        .expect("register device");

    upload.assert_calls(1);
}

#[tokio::test]
async fn login_uploads_initial_key_package_count_of_one_hundred() {
    let server = MockServer::start();
    mount_login_and_registration(&server);
    let upload = server.mock(|when, then| {
        when.method(POST)
            .path("/v14/mls/key-packages/self/new-client-id")
            .is_true(|req| key_packages_array(req.body_ref()).is_some_and(|arr| arr.len() == 100));
        then.status(201);
    });

    let builder = ClientBuilder::new()
        .server(common::server_config(&server.base_url()))
        .permit_http();
    let _client = builder
        .login("alice@example.test".into(), "pw".into(), None)
        .await
        .expect("login")
        .register_device()
        .await
        .expect("register device");

    upload.assert_calls(1);
}

#[tokio::test]
async fn login_uploaded_key_packages_are_base64_encoded_strings() {
    let server = MockServer::start();
    mount_login_and_registration(&server);
    let upload = server.mock(|when, then| {
        when.method(POST)
            .path("/v14/mls/key-packages/self/new-client-id")
            .is_true(|req| {
                let Some(packages) = key_packages_array(req.body_ref()) else {
                    return false;
                };
                packages.iter().all(|v| {
                    v.as_str().is_some_and(|s| {
                        use base64::Engine as _;
                        let decoded = base64::engine::general_purpose::STANDARD.decode(s);
                        decoded.is_ok_and(|b| !b.is_empty())
                    })
                })
            });
        then.status(201);
    });

    let builder = ClientBuilder::new()
        .server(common::server_config(&server.base_url()))
        .permit_http();
    let _client = builder
        .login("alice@example.test".into(), "pw".into(), None)
        .await
        .expect("login")
        .register_device()
        .await
        .expect("register device");

    upload.assert_calls(1);
}

#[tokio::test]
async fn login_fails_when_upload_returns_mls_protocol_error() {
    let server = MockServer::start();
    mount_login_and_registration(&server);
    server.mock(|when, then| {
        when.method(POST)
            .path("/v14/mls/key-packages/self/new-client-id");
        then.status(400)
            .header("content-type", "application/json")
            .body(r#"{"code":400,"label":"mls-protocol-error","message":"MLS protocol error"}"#);
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
        .expect_err("upload 400 must abort device registration");
    assert!(
        matches!(err, KupferdrahtError::BackendRejected(_)),
        "expected BackendRejected from upload 400, got {err:?}"
    );
}
