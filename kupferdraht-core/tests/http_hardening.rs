// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Chore: `harden-http-client` — HTTPS enforcement + timeouts on
//! `HttpClient::new`. Consumed via the `ClientBuilder` facade.
//!
//! Two contracts locked in here:
//!
//! * A `ServerConfig` whose `api_base_url` is not `https://` must be
//!   rejected with `KupferdrahtError::InvalidInput` unless the caller has
//!   explicitly opted in via `ClientBuilder::permit_http()`.
//! * A connect-timeout of at most a few seconds must be in effect —
//!   hitting a port that silently drops SYN packets surfaces as a
//!   `KupferdrahtError::Network` within that window, not a hang.

use kupferdraht::{ClientBuilder, KupferdrahtError};

mod common;

#[tokio::test]
async fn login_rejects_plain_http_url_without_permit_http() {
    // Deliberately `http://` (not `https://`). This ServerConfig
    // would normally point at a mock server, but the builder has no
    // `permit_http()` call, so the URL must be rejected before any
    // network traffic is attempted.
    let err = ClientBuilder::new()
        .server(common::server_config("http://127.0.0.1:1"))
        .login("alice@example.test".into(), "pw".into(), None)
        .await
        .expect_err("plain HTTP must be rejected");
    assert!(
        matches!(err, KupferdrahtError::InvalidInput(_)),
        "expected InvalidInput for plain HTTP, got {err:?}"
    );
}

#[tokio::test]
async fn login_with_permit_http_accepts_plain_http_base_url_as_far_as_dispatch() {
    // With `.permit_http()` the URL must at least pass the scheme
    // check. Port 1 is a reserved/unbound port so the request will
    // fail with `KupferdrahtError::Network` — that's fine for this test,
    // we only need to see the error shape *not* be `InvalidInput`.
    let err = ClientBuilder::new()
        .server(common::server_config("http://127.0.0.1:1"))
        .permit_http()
        .login("alice@example.test".into(), "pw".into(), None)
        .await
        .expect_err("unreachable port must still fail");
    assert!(
        matches!(err, KupferdrahtError::Network(_)),
        "expected Network (not InvalidInput) once permit_http is set, got {err:?}"
    );
}

#[tokio::test]
async fn login_against_unreachable_host_fails_within_connect_timeout() {
    // 127.0.0.2:1 is address-reserved and typically produces a
    // connect failure (ECONNREFUSED) rather than a hang. This test
    // guards against an accidental regression that removes the
    // connect-timeout on reqwest — with no timeout, some kernels
    // stall SYN retries for minutes. We require the failure to
    // surface within 15 seconds (the hard upper bound for
    // `HttpClient::new`'s connect_timeout + the immediate retry
    // round).
    let started = std::time::Instant::now();
    let err = ClientBuilder::new()
        .server(common::server_config("http://127.0.0.2:1"))
        .permit_http()
        .login("alice@example.test".into(), "pw".into(), None)
        .await
        .expect_err("unreachable address must fail");
    let elapsed = started.elapsed();
    assert!(
        matches!(err, KupferdrahtError::Network(_)),
        "expected Network failure, got {err:?}"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(15),
        "login must fail within the connect-timeout budget, took {elapsed:?}"
    );
}
