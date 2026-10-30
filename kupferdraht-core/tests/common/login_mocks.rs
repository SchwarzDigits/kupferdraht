// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! The HTTP mocks of a full login, `httpmock` only — shared by the
//! integration tests (`tests/common`) and, through `#[path]`, by the
//! crate's own unit tests (Step 110a).
#![allow(dead_code)]

use httpmock::Method::{GET, POST, PUT};
use httpmock::{Mock, MockServer};

/// Authentication: api-version → login → self.
pub fn mount_login(server: &MockServer) {
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
}

/// Device registration: clients (POST) → clients (PUT) → access →
/// mls/key-packages → mls/public-keys. Returns the `POST /clients` mock so
/// a test can count registrations.
pub fn mount_device_registration(server: &MockServer) -> Mock<'_> {
    let register = server.mock(|when, then| {
        when.method(POST).path("/v14/clients");
        then.status(201)
            .header("content-type", "application/json")
            .body(r#"{"id":"new-client-id","type":"permanent"}"#);
    });
    server.mock(|when, then| {
        when.method(PUT).path("/v14/clients/new-client-id");
        then.status(200);
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
        when.method(POST)
            .path("/v14/mls/key-packages/self/new-client-id");
        then.status(201);
    });
    server.mock(|when, then| {
        when.method(GET).path("/v14/mls/public-keys");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"removal":{"ed25519":"ZWQ=","ecdsa_secp256r1_sha256":"cDI1Ng==","ecdsa_secp384r1_sha384":"cDM4NA==","ecdsa_secp521r1_sha512":"cDUyMQ=="}}"#);
    });
    register
}

/// The eight mocks of a full login flow: [`mount_login`] and
/// [`mount_device_registration`].
pub fn mount_full_login_flow(server: &MockServer) {
    mount_login(server);
    mount_device_registration(server);
}
