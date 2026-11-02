// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 132 — review residuals: transport, platform and threading
//! hardening (TDD red).
//!
//! The pins with a consumer-visible surface live here; the ones on
//! crate-private functions sit beside them (`wire/websocket.rs`,
//! `sync/push/legacy.rs`, `storage/home.rs`, `wire/dto.rs`,
//! `rooms/room.rs`, `encryption/asset_crypto.rs`), the removal-flush
//! restart in `step_74_removal_batching.rs` and the cold room cache in
//! `step_14g_auto_hydration.rs`.
//!
//! * A redirect of the API is not followed (M2): no 3xx is honoured, so a
//!   307 never re-sends the login body to another host.
//! * `ws://` is refused without `permit_http`, like `http://` (M3).
//! * The URL scheme is checked before the home or the keystore is touched
//!   (former gap-B.26).
//! * A backend whose `defaultCipherSuite` is not ours is a typed error at
//!   login and at resume (NG-25, measured live: observations §94).
//! * `docs/bindings.md` states the threading contract and the
//!   `assetToken` line; the `Store` trait names its atomicity expectation.

use std::path::Path;
use std::sync::Arc;

use httpmock::Method::GET;
use httpmock::MockServer;
use kupferdraht::keystore::{InMemoryKeyStore, SecureKeyStore};
use kupferdraht::{ClientBuilder, KupferdrahtError};

mod common;

/// Read a file at the workspace root (one level above the
/// `kupferdraht-core` manifest).
fn workspace_file(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

const API_VERSION_BODY: &str =
    r#"{"supported":[14],"development":[],"domain":"example.test","federation":false}"#;

// ── 1. redirects ──────────────────────────────────────────────────────

/// Wire's API does not redirect; a 3xx is an error. Measured against the
/// redirect target: it must never be requested — for a 307 that would
/// re-send a login body to the host of the server's choosing.
#[tokio::test]
async fn login_does_not_follow_a_redirect_of_the_api() {
    for status in [302u16, 307] {
        let server = MockServer::start_async().await;
        let target = server.mock(|when, then| {
            when.method(GET).path("/elsewhere/api-version");
            then.status(200)
                .header("content-type", "application/json")
                .body(API_VERSION_BODY);
        });
        server.mock(|when, then| {
            when.method(GET).path("/api-version");
            then.status(status).header(
                "location",
                format!("{}/elsewhere/api-version", server.base_url()),
            );
        });
        let err = ClientBuilder::new()
            .server(common::server_config(&server.base_url()))
            .permit_http()
            .login("alice@example.test".into(), "pw".into(), None)
            .await
            .expect_err("a redirected API must not log in");
        assert_eq!(
            target.calls(),
            0,
            "{status}: the redirect target must not be requested (login ended with {err:?})"
        );
    }
}

// ── 2. ws:// without permit_http ─────────────────────────────────────

/// The socket URL gets the same scheme rule as the API URL: `ws://` only
/// with `permit_http`, refused before any network traffic.
#[tokio::test]
async fn login_refuses_a_plain_ws_socket_url_without_permit_http() {
    let mut config = common::server_config("https://127.0.0.1:1");
    config.websocket_url = "ws://127.0.0.1:1".into();
    let err = ClientBuilder::new()
        .server(config)
        .login("alice@example.test".into(), "pw".into(), None)
        .await
        .expect_err("ws:// without permit_http must be refused");
    assert!(
        matches!(err, KupferdrahtError::InvalidInput(ref m) if m.contains("ws://")),
        "expected InvalidInput naming ws://, got {err:?}"
    );
}

// ── 8. the scheme check comes before the home ────────────────────────

/// A base directory that cannot be a home (a file) would fail with
/// `Storage`; with the scheme checked first the plain-`http://` URL is
/// refused as `InvalidInput` before the home or the keystore is touched
/// (observations §76: the Keychain probe used to block the rejection).
#[tokio::test]
async fn login_checks_the_url_scheme_before_opening_the_home() {
    let file = tempfile::NamedTempFile::new().expect("tempfile");
    let keystore: Arc<dyn SecureKeyStore> = Arc::new(InMemoryKeyStore::new());
    let err = ClientBuilder::new()
        .server(common::server_config("http://127.0.0.1:1"))
        .base_dir(file.path().to_path_buf())
        .keystore(keystore)
        .login("alice@example.test".into(), "pw".into(), None)
        .await
        .expect_err("plain http must be refused");
    assert!(
        matches!(err, KupferdrahtError::InvalidInput(_)),
        "the URL check must come before the home is opened, got {err:?}"
    );
}

// ── 12. ciphersuite mismatch ─────────────────────────────────────────

fn feature_configs_with_default_suite(suite: u8) -> String {
    format!(
        r#"{{"mls":{{"status":"enabled","config":{{"allowedCipherSuites":[1,2],"defaultCipherSuite":{suite},"defaultProtocol":"mls","supportedProtocols":["mls","proteus"]}}}}}}"#
    )
}

fn mount_feature_configs(server: &MockServer, suite: u8) -> httpmock::Mock<'_> {
    server.mock(|when, then| {
        when.method(GET).path("/v14/feature-configs");
        then.status(200)
            .header("content-type", "application/json")
            .body(feature_configs_with_default_suite(suite));
    })
}

/// A backend whose team default is another suite (the self-hosted stack of
/// observations §94 runs `1`) fails at login with both suites named,
/// instead of on the first group add with "no KeyPackages".
#[tokio::test]
async fn login_refuses_a_backend_whose_default_ciphersuite_differs() {
    let server = MockServer::start_async().await;
    common::mount_full_login_flow(&server);
    let _configs = mount_feature_configs(&server, 1);
    let dir = tempfile::tempdir().expect("tempdir");
    let keystore: Arc<dyn SecureKeyStore> = Arc::new(InMemoryKeyStore::new());
    let outcome = ClientBuilder::new()
        .server(common::server_config(&server.base_url()))
        .permit_http()
        .base_dir(dir.path().to_path_buf())
        .keystore(keystore)
        .login("alice@example.test".into(), "pw".into(), None)
        .await;
    let err = match outcome {
        Err(e) => e,
        Ok(pending) => pending
            .register_device()
            .await
            .expect_err("a differing default ciphersuite must refuse the device"),
    };
    assert!(
        matches!(
            err,
            KupferdrahtError::CiphersuiteMismatch {
                backend: 1,
                ours: 2
            }
        ),
        "expected CiphersuiteMismatch {{ backend: 1, ours: 2 }}, got {err:?}"
    );
}

/// The same check on the login that reuses an existing device (the
/// "resume" of the login pipeline; `ClientBuilder::resume` itself is
/// offline and fetches nothing): a backend reconfigured to another default
/// since the first login is refused before the device is reused.
#[tokio::test]
async fn relogin_on_a_reused_device_refuses_a_backend_whose_default_ciphersuite_differs() {
    let server = MockServer::start_async().await;
    common::mount_full_login_flow(&server);
    let mut ours = mount_feature_configs(&server, 2);
    let dir = tempfile::tempdir().expect("tempdir");
    let keystore: Arc<dyn SecureKeyStore> = Arc::new(InMemoryKeyStore::new());
    let app = kupferdraht::Kupferdraht::open(dir.path().to_path_buf(), keystore)
        .expect("open kupferdraht")
        .permit_http();
    let first = app
        .login(
            common::server_config(&server.base_url()),
            "alice@example.test".into(),
            "pw".into(),
            None,
            None,
        )
        .await
        .expect("login on our suite")
        .register_device()
        .await
        .expect("register on our suite");
    let device_id = first.device_id().expect("device id").to_string();
    drop(first);
    ours.delete();
    let _theirs = mount_feature_configs(&server, 1);
    server.mock(|when, then| {
        when.method(GET).path(format!("/v14/clients/{device_id}"));
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(r#"{{"id":"{device_id}","type":"permanent"}}"#));
    });

    let outcome = app
        .login(
            common::server_config(&server.base_url()),
            "alice@example.test".into(),
            "pw".into(),
            None,
            None,
        )
        .await;
    let err = match outcome {
        Err(e) => e,
        Ok(pending) => pending
            .register_device()
            .await
            .expect_err("a backend on another default suite must refuse the reused device"),
    };
    assert!(
        matches!(
            err,
            KupferdrahtError::CiphersuiteMismatch {
                backend: 1,
                ours: 2
            }
        ),
        "expected CiphersuiteMismatch {{ backend: 1, ours: 2 }}, got {err:?}"
    );
}

// ── 5 / 6 / 7. documentation and trait pins ─────────────────────────

/// `docs/bindings.md` states the threading contract: every call may block
/// for storage and crypto, so a consumer calls from a background
/// dispatcher, never the main thread (H5).
#[test]
fn bindings_doc_states_the_threading_contract() {
    let doc = workspace_file("docs/bindings.md");
    assert!(
        doc.contains("## Threading"),
        "docs/bindings.md needs a Threading section (Step 132 item 5)"
    );
    let lower = doc.to_lowercase();
    for needle in ["may block", "main thread", "background"] {
        assert!(
            lower.contains(needle),
            "the threading contract must say '{needle}'"
        );
    }
}

/// `docs/bindings.md` tells the consumer not to log `AssetMeta` as a
/// whole: the generated `toString` / `description` prints `assetToken`
/// although the Rust `Debug` redacts it (H6 residual).
#[test]
fn bindings_doc_warns_about_the_asset_token_in_generated_to_string() {
    let doc = workspace_file("docs/bindings.md");
    assert!(
        doc.contains("assetToken"),
        "docs/bindings.md must name assetToken (Step 132 item 6)"
    );
    let lower = doc.to_lowercase();
    assert!(
        lower.contains("do not log") || lower.contains("never log"),
        "the assetToken line must tell the consumer not to log AssetMeta"
    );
}

/// The `Store` trait's documentation names the atomicity expectation a
/// custom implementation has to meet (B7 / B9 residual).
#[test]
fn store_trait_doc_names_the_atomicity_expectation() {
    let source = workspace_file("kupferdraht-core/src/storage/records.rs");
    let at = source
        .find("pub trait Store")
        .expect("records.rs declares the Store trait");
    let doc_comment = &source[at.saturating_sub(2500)..at];
    assert!(
        doc_comment.to_lowercase().contains("atomic"),
        "the Store trait's doc comment must state the atomicity expectation (Step 132 item 7)"
    );
}
