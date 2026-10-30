// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 1b — Device registration + access-token rebind.
//!
//! Failing tests per PLAN.md Step 1 "TDD starter tests, 1b":
//! * `POST /v14/clients` with `proteus_prekeys` + `lastkey` payload
//!   yields `201` + `client_id`.
//! * Missing `prekeys` / `lastkey` is rejected (regression guard —
//!   asserted indirectly by `mock.is_true(...)` on the request body).
//! * After `POST /v14/clients`, the `POST /v14/access?client_id=<id>`
//!   rebind is issued and the client holds the rebound access token.
//!
//! Tests use `httpmock` — no live Wire traffic. Expected to fail (red)
//! until the 1b implementation phase lands.

use httpmock::Method::{GET, POST, PUT};
use httpmock::MockServer;
use kupferdraht::{ClientBuilder, KupferdrahtError};

mod common;

fn mount_step_1a_endpoints(server: &MockServer) {
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
            // `Secure; Path=/access` mirrors Wire production (observations.md §18).
            // The `Secure` flag is load-bearing for the regression guard in
            // `access_rebind_request_carries_zuid_cookie_header`: reqwest's
            // automatic cookie store refuses to forward `Secure` cookies
            // over the plain-HTTP mock server, so if Kupferdraht ever stops
            // extracting `zuid` from Set-Cookie and setting the `Cookie`
            // header explicitly, the outbound `/access` request will have
            // no cookie at all — the cookie-requiring mock will not match
            // and the fallback 403 path will surface the regression.
            .header("set-cookie", "zuid=test; HttpOnly; Secure; Path=/access")
            .body(r#"{"access_token":"initial.jwt.token","token_type":"Bearer","expires_in":900,"user":"user-uuid"}"#);
    });
    server.mock(|when, then| {
        when.method(GET).path("/v14/self");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"id":"user-uuid","qualified_id":{"id":"user-uuid","domain":"example.test"},"handle":"alice","name":"Alice Example","email":"alice@example.test"}"#);
    });
    // Step 1c.6: PUT /v14/clients/{id} to register the MLS public
    // key before uploading KeyPackages (observations.md §19).
    server.mock(|when, then| {
        when.method(PUT).path("/v14/clients/new-client-id");
        then.status(200);
    });
    // Step 1c.3: login also uploads the initial KeyPackage batch.
    // Tests that don't care about upload specifics accept any shape.
    server.mock(|when, then| {
        when.method(POST)
            .path("/v14/mls/key-packages/self/new-client-id");
        then.status(201);
    });
    // Step 1c.4: login also fetches Wire's backend removal-sender keys.
    server.mock(|when, then| {
        when.method(GET).path("/v14/mls/public-keys");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"removal":{"ed25519":"ZWQ=","ecdsa_secp256r1_sha256":"cDI1Ng==","ecdsa_secp384r1_sha384":"cDM4NA==","ecdsa_secp521r1_sha512":"cDUyMQ=="}}"#);
    });
}

/// Mount a `POST /v14/clients` mock that only matches requests whose
/// JSON body contains a non-empty `prekeys` array AND a `lastkey`
/// object. Returns `201` with a fixed `client_id`.
///
/// This is the regression guard: if Kupferdraht ever serializes the request
/// without these fields, the mock will not match and the test fails.
fn mount_register_with_schema_guard<'a>(server: &'a MockServer) -> httpmock::Mock<'a> {
    server.mock(|when, then| {
        when.method(POST).path("/v14/clients").is_true(|req| {
            let body_bytes = req.body_ref();
            let Ok(body) = serde_json::from_slice::<serde_json::Value>(body_bytes) else {
                return false;
            };
            let prekeys_ok = body
                .get("prekeys")
                .and_then(serde_json::Value::as_array)
                .is_some_and(|a| !a.is_empty());
            let lastkey_ok = body.get("lastkey").is_some();
            prekeys_ok && lastkey_ok
        });
        then.status(201)
            .header("content-type", "application/json")
            .body(r#"{"id":"new-client-id","type":"permanent"}"#);
    })
}

/// Mount a `POST /v14/access?client_id=<id>` mock that **only** matches
/// requests whose `Cookie` header carries `zuid=<value>`. This is the
/// regression guard for observations.md §17, §18:
///
/// - `/access` authenticates by cookie, not Bearer — a request without
///   the `zuid` cookie is 403 at the real server.
/// - reqwest's cookie store does not reliably forward Wire's
///   `Path=/access`-scoped cookie, so Kupferdraht extracts the `zuid` and
///   sets it explicitly. If a future refactor drops the explicit
///   `Cookie` header, this mock will not match, the request will 404,
///   and the tests below will fail instead of silently regressing.
///
/// The login mock in `mount_step_1a_endpoints` sets `zuid=test`; we
/// require the exact cookie value here so the test both verifies the
/// extraction path (Set-Cookie → LoginOutcome::refresh_token) and the
/// forwarding path (LoginOutcome::refresh_token → outbound Cookie).
fn mount_rebind_for<'a>(server: &'a MockServer, client_id: &str) -> httpmock::Mock<'a> {
    server.mock(|when, then| {
        when.method(POST)
            .path("/v14/access")
            .query_param("client_id", client_id)
            .header("cookie", "zuid=test");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"access_token":"rebound.jwt.token","token_type":"Bearer","expires_in":900,"user":"user-uuid"}"#);
    })
}

#[tokio::test]
async fn register_client_request_includes_proteus_prekeys_and_lastkey() {
    let server = MockServer::start();
    mount_step_1a_endpoints(&server);
    let register = mount_register_with_schema_guard(&server);
    let _ = mount_rebind_for(&server, "new-client-id");

    let builder = ClientBuilder::new()
        .server(common::server_config(&server.base_url()))
        .permit_http();
    let _client = builder
        .login("alice@example.test".into(), "pw".into(), None)
        .await
        .expect("login should complete the full 1b flow")
        .register_device()
        .await
        .expect("register device");

    register.assert();
}

#[tokio::test]
async fn login_materializes_client_id_from_register_response() {
    let server = MockServer::start();
    mount_step_1a_endpoints(&server);
    let _ = mount_register_with_schema_guard(&server);
    let _ = mount_rebind_for(&server, "new-client-id");

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

    assert_eq!(client.device_id(), Some("new-client-id"));
}

#[tokio::test]
async fn access_token_is_rebound_after_registration() {
    let server = MockServer::start();
    mount_step_1a_endpoints(&server);
    let _ = mount_register_with_schema_guard(&server);
    let rebind = mount_rebind_for(&server, "new-client-id");

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

    rebind.assert();
    assert_eq!(
        client.access_token().as_deref(),
        Some("rebound.jwt.token"),
        "client must carry the REBOUND token, not the initial /login response"
    );
}

/// Regression guard for observations.md §17, §18: the cookie-auth contract of
/// `POST /v{N}/access` must be met by an explicit `Cookie: zuid=<value>`
/// header on the outbound request.
///
/// This test mounts a mock that **explicitly** requires the header, and
/// a fallback mock on the same path that returns 403 if the cookie is
/// missing. If Kupferdraht regresses to relying on reqwest's automatic
/// cookie store (or drops the cookie entirely), the login will fail
/// with `BackendRejected` — instead of silently succeeding on a tolerant
/// mock.
#[tokio::test]
async fn access_rebind_request_carries_zuid_cookie_header() {
    let server = MockServer::start();
    mount_step_1a_endpoints(&server);
    let _ = mount_register_with_schema_guard(&server);

    let rebind_with_cookie = server.mock(|when, then| {
        when.method(POST)
            .path("/v14/access")
            .query_param("client_id", "new-client-id")
            .header("cookie", "zuid=test");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"access_token":"rebound.jwt.token","token_type":"Bearer","expires_in":900,"user":"user-uuid"}"#);
    });
    // Fallback: any other `/access` call (no cookie, wrong cookie,
    // Bearer auth, etc.) must be rejected so a regression surfaces.
    server.mock(|when, then| {
        when.method(POST).path("/v14/access");
        then.status(403)
            .header("content-type", "application/json")
            .body(r#"{"code":403,"label":"invalid-credentials","message":"Missing cookie and token"}"#);
    });

    let builder = ClientBuilder::new()
        .server(common::server_config(&server.base_url()))
        .permit_http();
    let client = builder
        .login("alice@example.test".into(), "pw".into(), None)
        .await
        .expect("login must succeed when the `Cookie: zuid=...` header is attached")
        .register_device()
        .await
        .expect("register device");

    rebind_with_cookie.assert_calls(1);
    assert_eq!(client.access_token().as_deref(), Some("rebound.jwt.token"));
}

/// `register_device` consumes the captured session on success, so the
/// `PendingLogin` is single-use: a second call after a successful one must
/// fail rather than register a duplicate device.
#[tokio::test]
async fn register_device_is_single_use_after_success() {
    let server = MockServer::start();
    mount_step_1a_endpoints(&server);
    let _ = mount_register_with_schema_guard(&server);
    let _ = mount_rebind_for(&server, "new-client-id");

    let pending = ClientBuilder::new()
        .server(common::server_config(&server.base_url()))
        .permit_http()
        .login("alice@example.test".into(), "pw".into(), None)
        .await
        .expect("login");
    pending
        .register_device()
        .await
        .expect("first register_device succeeds");
    let err = pending
        .register_device()
        .await
        .expect_err("a second register_device after success must fail");
    assert!(
        matches!(err, KupferdrahtError::InvalidInput(_)),
        "expected InvalidInput (login already completed), got {err:?}"
    );
}

/// The device-limit recovery path: when `register_device` hits the per-user
/// cap, the captured session is **restored** so the handle stays usable —
/// `devices()` still works and `register_device` is retryable (it
/// re-attempts registration rather than reporting the login complete).
#[tokio::test]
async fn register_device_restores_session_for_retry_on_too_many_devices() {
    let server = MockServer::start();
    mount_step_1a_endpoints(&server);
    // `POST /clients` hits Wire's per-user device cap.
    server.mock(|when, then| {
        when.method(POST).path("/v14/clients");
        then.status(400)
            .header("content-type", "application/json")
            .body(r#"{"code":400,"label":"too-many-clients","message":"too many clients"}"#);
    });
    // `GET /clients` backs the recovery `devices()` listing.
    server.mock(|when, then| {
        when.method(GET).path("/v14/clients");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"[{"id":"dev-1","type":"permanent","time":"2026-04-29T10:00:00Z"}]"#);
    });

    let pending = ClientBuilder::new()
        .server(common::server_config(&server.base_url()))
        .permit_http()
        .login("alice@example.test".into(), "pw".into(), None)
        .await
        .expect("phase 1 (auth) should succeed");

    let first = pending.register_device().await;
    assert!(
        matches!(first, Err(KupferdrahtError::TooManyDevices)),
        "expected TooManyDevices, got {first:?}"
    );

    // Session survived the failure: the recovery surface still works on the
    // same handle (no re-login).
    let devices = pending
        .devices()
        .await
        .expect("devices() must work after the cap (token still valid)");
    assert_eq!(devices.len(), 1, "should list the one registered device");

    // ...and the handle is retryable: a second attempt re-runs registration
    // (TooManyDevices again), NOT `InvalidInput` — proving take/restore put
    // the session back rather than consuming it.
    let second = pending.register_device().await;
    assert!(
        matches!(second, Err(KupferdrahtError::TooManyDevices)),
        "retry must re-attempt registration, not report login complete; got {second:?}"
    );
}
