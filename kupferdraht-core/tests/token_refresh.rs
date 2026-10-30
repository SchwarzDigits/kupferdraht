// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! TDD (red): in-session access-token refresh — reactive 401-retry.
//!
//! Written BEFORE the implementation; these must FAIL until the refresh
//! seam exists. They prove: a 401 on an authenticated call triggers
//! exactly one `/access` refresh + one retry with the new token; the
//! rotated `zuid` is persisted; concurrent 401s collapse to a single
//! refresh; a failed refresh surfaces an error without looping.

use httpmock::Method::{GET, POST, PUT};
use httpmock::MockServer;
use kupferdraht::KupferdrahtError;

mod common;

/// Like `common::mount_full_login_flow`, but the login `/access` rebind
/// **rotates** the `zuid` (`Set-Cookie: zuid=after-login`). The post-login
/// refresh cookie is therefore `after-login` — distinct from the original
/// `test` — so a later refresh `/access` mock can match on its own cookie
/// without colliding with the login one.
fn mount_login_rotating(server: &MockServer) {
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
            .header("set-cookie", "zuid=after-login; HttpOnly; Secure; Path=/access")
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
}

/// Mount the happy refresh trio for `GET /v14/clients`:
/// 1st hit with the post-login token (`rebound.jwt.token`) → 401;
/// `/access` (cookie `after-login`) → 200 with a NEW token
/// (`refreshed.jwt.token`) + rotated cookie (`after-refresh`);
/// 2nd hit with the new token → 200. Returns the `/access` mock handle
/// so the caller can assert it was hit exactly once.
fn mount_refresh_trio(server: &MockServer) -> httpmock::Mock<'_> {
    server.mock(|when, then| {
        when.method(GET)
            .path("/v14/clients")
            .header("authorization", "Bearer rebound.jwt.token");
        then.status(401)
            .header("content-type", "application/json")
            .body(r#"{"code":401,"label":"invalid-token","message":"x"}"#);
    });
    let refresh = server.mock(|when, then| {
        when.method(POST)
            .path("/v14/access")
            .query_param("client_id", "new-client-id")
            .header("cookie", "zuid=after-login");
        then.status(200)
            .header("content-type", "application/json")
            .header("set-cookie", "zuid=after-refresh; HttpOnly; Secure; Path=/access")
            .body(r#"{"access_token":"refreshed.jwt.token","token_type":"Bearer","expires_in":900,"user":"user-uuid"}"#);
    });
    server.mock(|when, then| {
        when.method(GET)
            .path("/v14/clients")
            .header("authorization", "Bearer refreshed.jwt.token");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"[{"id":"abcd1234","type":"permanent","time":"2026-04-29T10:00:00Z"}]"#);
    });
    refresh
}

/// t4 — a 401 triggers exactly one refresh + one retry, and the call
/// then succeeds with the new token.
#[tokio::test]
async fn authed_call_refreshes_and_retries_on_401() {
    let server = MockServer::start();
    mount_login_rotating(&server);
    let (client, _store) = common::run_login(&server).await;
    let refresh = mount_refresh_trio(&server);

    let devices = client
        .devices()
        .await
        .expect("devices() must succeed after refresh + retry");
    assert_eq!(devices.len(), 1, "retry must return the 200 body");
    refresh.assert_calls(1);
}

/// t5 — the rotated `zuid` + new access token + a fresh issued-at are
/// persisted to the session store after a refresh.
#[tokio::test]
async fn refresh_persists_rotated_zuid_and_new_access_token() {
    let server = MockServer::start();
    mount_login_rotating(&server);
    let (client, store) = common::run_login(&server).await;
    let _refresh = mount_refresh_trio(&server);

    client.devices().await.expect("devices()");

    let session = store
        .load_session("user-uuid@example.test")
        .await
        .expect("load_session")
        .expect("a session row must exist after refresh");
    assert_eq!(
        session.refresh_token, "after-refresh",
        "rotated zuid must be persisted"
    );
    assert_eq!(
        session.access_token, "refreshed.jwt.token",
        "new access token must be persisted"
    );
    assert!(
        session.access_token_issued_at_ms > 0,
        "issued_at must be stamped at acquisition"
    );
}

/// t6 — N concurrent calls that all see a 401 collapse to a single
/// network `/access` refresh (thundering-herd guard).
#[tokio::test]
async fn concurrent_401s_collapse_to_single_refresh() {
    let server = MockServer::start();
    mount_login_rotating(&server);
    let (client, _store) = common::run_login(&server).await;
    let refresh = mount_refresh_trio(&server);

    let mut handles = Vec::new();
    for _ in 0..8 {
        let c = client.clone();
        handles.push(tokio::spawn(async move { c.devices().await }));
    }
    for h in handles {
        h.await
            .expect("join")
            .expect("every concurrent devices() must succeed");
    }
    refresh.assert_calls(1);
}

/// t7 — when the refresh itself fails, the error surfaces and the call
/// is not retried in a loop.
#[tokio::test]
async fn failed_refresh_surfaces_error_without_looping() {
    let server = MockServer::start();
    mount_login_rotating(&server);
    let (client, _store) = common::run_login(&server).await;
    let authed = server.mock(|when, then| {
        when.method(GET)
            .path("/v14/clients")
            .header("authorization", "Bearer rebound.jwt.token");
        then.status(401)
            .header("content-type", "application/json")
            .body(r#"{"code":401,"label":"invalid-token","message":"x"}"#);
    });
    let refresh = server.mock(|when, then| {
        when.method(POST)
            .path("/v14/access")
            .query_param("client_id", "new-client-id")
            .header("cookie", "zuid=after-login");
        then.status(403)
            .header("content-type", "application/json")
            .body(r#"{"code":403,"label":"invalid-credentials","message":"x"}"#);
    });

    let err = client
        .devices()
        .await
        .expect_err("a failed refresh must surface an error");
    // Step 39: a 403 invalid-credentials on the /access refresh is the terminal
    // session-expiry signal (a dead zuid can't be refreshed) — typed distinctly
    // so the sync loop stops + signals re-login. It still surfaces without
    // looping, which is what this test guards.
    assert!(
        matches!(err, KupferdrahtError::SessionExpired),
        "a dead-zuid /access refresh surfaces SessionExpired, got {err:?}"
    );
    refresh.assert_calls(1);
    authed.assert_calls(1);
}
