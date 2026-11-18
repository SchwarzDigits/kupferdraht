// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 162 — SSO, typing and the email change in matrix-rust-sdk's shape.
//!
//! * SSO: `sso_login_url(redirect_url, provider)` builds the URL the
//!   application opens; `login_with_sso_callback(callback_url)` reads the
//!   session out of the URL the browser comes back to and returns a
//!   `PendingLogin`. The redirect pair the backend wants is derived from the
//!   one redirect URL (`docs/wire/kalium-readings.md` #30).
//! * Typing: `Room::typing_notice(bool)`.
//! * The email change: `Client::request_email_change(email)`.
//!
//! The names these replace are pinned gone in `step_162_surface_pins.rs`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use httpmock::Method::{GET, HEAD, POST, PUT};
use httpmock::MockServer;
use kupferdraht::keystore::{InMemoryKeyStore, SecureKeyStore};
use kupferdraht::{ClientBuilder, Kupferdraht, KupferdrahtError, SyncSettings};
use tempfile::TempDir;

mod common;

use common::drain_fixture::{DOMAIN, ROOM_UUID, mount_room, room_id};

const PROVIDER: &str = "9c8a1f00-1111-4222-8333-444455556666";
const REDIRECT: &str = "wire-kupferdraht://sso";
/// A callback as the backend sends it back: the session cookie's whole
/// `Set-Cookie` line, percent-encoded, in our success parameter; the value
/// holds `=`, `.`, `-` and `_` (live-confirmed, `docs/observations.md` §107).
const SIGNED_IN: &str = "wire-kupferdraht://sso?credential=zuid%3Da1-b2_c3.d%3D4.e%3D5%3B%20Path%3D%2Faccess%3B%20Expires%3DWed%2C%2018-Nov-2026%2010%3A00%3A00%20GMT%3B%20HttpOnly";
const SESSION_COOKIE: &str = "zuid=a1-b2_c3.d=4.e=5";
const TEAM: &str = "5e7a0c00-2222-4333-8444-555566667777";

fn home() -> PathBuf {
    let home = TempDir::new().expect("tempdir");
    let path = home.path().to_path_buf();
    let _ = Box::leak(Box::new(home));
    path
}

fn builder(server: &MockServer, home: &Path) -> ClientBuilder {
    let keystore: Arc<dyn SecureKeyStore> = Arc::new(InMemoryKeyStore::new());
    ClientBuilder::new()
        .server(common::server_config(&server.base_url()))
        .permit_http()
        .base_dir(home.to_path_buf())
        .keystore(keystore)
}

fn manager(home: &Path) -> Kupferdraht {
    let keystore: Arc<dyn SecureKeyStore> = Arc::new(InMemoryKeyStore::new());
    Kupferdraht::open(home.to_path_buf(), keystore)
        .expect("open kupferdraht")
        .permit_http()
}

fn query(url: &str) -> HashMap<String, String> {
    url::Url::parse(url)
        .expect("a URL")
        .query_pairs()
        .into_owned()
        .collect()
}

fn assert_invalid<T>(result: Result<T, KupferdrahtError>, what: &str) {
    match result {
        Err(KupferdrahtError::InvalidInput(_)) => {}
        Err(other) => panic!("{what}: expected InvalidInput, got {other:?}"),
        Ok(_) => panic!("{what}: expected InvalidInput, got Ok"),
    }
}

fn mount_known_provider(server: &MockServer) -> httpmock::Mock<'_> {
    server.mock(|when, then| {
        when.method(HEAD)
            .path(format!("/sso/initiate-login/{PROVIDER}"));
        then.status(200);
    })
}

fn mount_default_provider<'a>(server: &'a MockServer, body: &str) -> httpmock::Mock<'a> {
    let body = body.to_string();
    server.mock(move |when, then| {
        when.method(GET).path("/sso/settings");
        then.status(200)
            .header("content-type", "application/json")
            .body(body.clone());
    })
}

/// The session cookie's exchange for the first access token: `/access`
/// with the callback's cookie and without a `client_id`. The user is an SSO
/// user as the backend shows one after the first login: in a team, without
/// a handle, its address not yet confirmed and so not its `email`
/// (live-confirmed, `docs/observations.md` §107).
fn mount_sso_session(server: &MockServer) -> httpmock::Mock<'_> {
    server.mock(|when, then| {
        when.method(GET).path("/api-version");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"supported":[14],"development":[]}"#);
    });
    // Mounted before the device registration's `/access`, which asks for a
    // `client_id`: the first mock that matches answers.
    let access = server.mock(|when, then| {
        when.method(POST)
            .path("/v14/access")
            .query_param_missing("client_id")
            .header("cookie", SESSION_COOKIE);
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"access_token":"initial.jwt.token","token_type":"Bearer","expires_in":900,"user":"user-uuid"}"#);
    });
    // The device registration's rebind carries the same cookie, its `=`
    // intact.
    server.mock(|when, then| {
        when.method(POST)
            .path("/v14/access")
            .query_param("client_id", "new-client-id")
            .header("cookie", SESSION_COOKIE);
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"access_token":"rebound.jwt.token","token_type":"Bearer","expires_in":900,"user":"user-uuid"}"#);
    });
    server.mock(|when, then| {
        when.method(GET).path("/v14/self");
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(
                r#"{{"id":"user-uuid","qualified_id":{{"id":"user-uuid","domain":"example.test"}},"name":"alice@example.test","email_unvalidated":"alice@example.test","team":"{TEAM}","managed_by":"wire"}}"#
            ));
    });
    server.mock(|when, then| {
        when.method(GET).path(format!("/v14/teams/{TEAM}"));
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(
                r#"{{"id":"{TEAM}","name":"Example Team","icon":"default"}}"#
            ));
    });
    common::login_mocks::mount_device_registration(server);
    access
}

// ---- the SSO login URL ------------------------------------------------

#[tokio::test]
async fn a_provider_yields_the_start_url_with_the_redirect_pair() {
    let server = MockServer::start();
    let probe = mount_known_provider(&server);
    let home = home();

    let url = builder(&server, &home)
        .sso_login_url(Some(REDIRECT), Some(&format!("wire-{PROVIDER}")))
        .await
        .expect("a known provider");

    let parsed = url::Url::parse(&url).expect("the start URL parses");
    assert_eq!(parsed.path(), format!("/sso/initiate-login/{PROVIDER}"));
    let pairs = query(&url);
    assert_eq!(
        pairs.get("success_redirect").map(String::as_str),
        Some("wire-kupferdraht://sso?credential=$cookie")
    );
    assert_eq!(
        pairs.get("error_redirect").map(String::as_str),
        Some("wire-kupferdraht://sso?reason=$label")
    );
    probe.assert_calls(1);
}

/// The SSO session gets the same per-home cookie label a password login
/// asks for (ADR-0025), so the two kinds of login treat a home alike.
#[tokio::test]
async fn the_start_url_names_the_homes_cookie_label() {
    let server = MockServer::start();
    let seen: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let sink = seen.clone();
    server.mock(move |when, then| {
        when.method(POST).path("/v14/login").is_true(move |req| {
            let body: serde_json::Value =
                serde_json::from_slice(req.body_ref()).expect("a JSON login body");
            *sink.lock().expect("label lock") = body["label"].as_str().map(str::to_string);
            true
        });
        then.status(200)
            .header("content-type", "application/json")
            .header("set-cookie", "zuid=test; HttpOnly; Secure; Path=/access")
            .body(r#"{"access_token":"initial.jwt.token","token_type":"Bearer","expires_in":900,"user":"user-uuid"}"#);
    });
    common::mount_full_login_flow(&server);
    mount_known_provider(&server);
    let home = home();
    builder(&server, &home)
        .login("alice@example.test".into(), "pw".into(), None)
        .await
        .expect("password login");
    let password_label = seen
        .lock()
        .expect("label lock")
        .clone()
        .expect("the password login names a label");

    let url = builder(&server, &home)
        .sso_login_url(Some(REDIRECT), Some(PROVIDER))
        .await
        .expect("a known provider");

    assert_eq!(query(&url).get("label"), Some(&password_label));
}

#[tokio::test]
async fn without_a_provider_the_backends_default_is_taken() {
    let server = MockServer::start();
    let settings = mount_default_provider(
        &server,
        &format!(r#"{{"default_sso_code":"wire-{PROVIDER}"}}"#),
    );

    let url = builder(&server, &home())
        .sso_login_url(Some(REDIRECT), None)
        .await
        .expect("the backend names a default provider");

    let parsed = url::Url::parse(&url).expect("the start URL parses");
    assert_eq!(parsed.path(), format!("/sso/initiate-login/{PROVIDER}"));
    settings.assert_calls(1);
}

#[tokio::test]
async fn without_a_provider_and_without_a_default_it_is_invalid_input() {
    let server = MockServer::start();
    mount_default_provider(&server, r#"{"default_sso_code":null}"#);

    assert_invalid(
        builder(&server, &home())
            .sso_login_url(Some(REDIRECT), None)
            .await,
        "no default provider",
    );
}

#[tokio::test]
async fn an_unknown_or_empty_provider_is_invalid_input() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(HEAD).path("/sso/initiate-login/unknown");
        then.status(404);
    });
    for provider in ["unknown", "", "wire-"] {
        assert_invalid(
            builder(&server, &home())
                .sso_login_url(Some(REDIRECT), Some(provider))
                .await,
            &format!("provider {provider:?}"),
        );
    }
}

#[tokio::test]
async fn a_redirect_the_backend_refuses_is_refused_before_any_request() {
    let server = MockServer::start();
    let probe = mount_known_provider(&server);
    let too_long = format!("{REDIRECT}/{}", "x".repeat(140));
    for redirect in [
        "kupferdraht://sso",
        "https://app.example.test/sso",
        too_long.as_str(),
    ] {
        assert_invalid(
            builder(&server, &home())
                .sso_login_url(Some(redirect), Some(PROVIDER))
                .await,
            &format!("redirect {redirect:?}"),
        );
    }
    probe.assert_calls(0);
}

/// In the browser there is no redirect: the backend's page reports the
/// outcome to the window that opened it.
#[tokio::test]
async fn without_a_redirect_the_start_url_carries_no_redirect_pair() {
    let server = MockServer::start();
    mount_known_provider(&server);

    let url = builder(&server, &home())
        .sso_login_url(None, Some(PROVIDER))
        .await
        .expect("a known provider");

    let pairs = query(&url);
    assert!(!pairs.contains_key("success_redirect"), "{url}");
    assert!(!pairs.contains_key("error_redirect"), "{url}");
    assert!(pairs.contains_key("label"), "{url}");
}

#[tokio::test]
async fn the_manager_builds_the_same_start_url() {
    let server = MockServer::start();
    mount_known_provider(&server);
    let home = home();

    let url = manager(&home)
        .sso_login_url(
            common::server_config(&server.base_url()),
            Some(REDIRECT),
            Some(PROVIDER),
        )
        .await
        .expect("a known provider");

    assert_eq!(
        url::Url::parse(&url).expect("parses").path(),
        format!("/sso/initiate-login/{PROVIDER}")
    );
}

// ---- the login from the callback --------------------------------------

#[tokio::test]
async fn a_signed_in_callback_logs_in() {
    let server = MockServer::start();
    let access = mount_sso_session(&server);

    let client = manager(&home())
        .login_with_sso_callback(common::server_config(&server.base_url()), SIGNED_IN, None)
        .await
        .expect("login from the callback")
        .register_device()
        .await
        .expect("register device");

    let account = client.account();
    assert_eq!(account.display_name(), "alice@example.test");
    assert_eq!(account.handle(), None);
    assert_eq!(
        account.email(),
        None,
        "an address the backend has not confirmed is not the account's email"
    );
    assert_eq!(account.team_id(), Some(TEAM));
    access.assert_calls(1);
}

#[tokio::test]
async fn the_builder_logs_in_from_the_callback_too() {
    let server = MockServer::start();
    let access = mount_sso_session(&server);

    builder(&server, &home())
        .login_with_sso_callback(SIGNED_IN)
        .await
        .expect("login from the callback");

    access.assert_calls(1);
}

#[tokio::test]
async fn a_refused_callback_is_backend_rejected_with_its_reason() {
    let server = MockServer::start();
    let access = mount_sso_session(&server);

    let result = builder(&server, &home())
        .login_with_sso_callback("wire-kupferdraht://sso?reason=forbidden")
        .await;

    match result {
        Err(KupferdrahtError::BackendRejected(message)) => {
            assert!(message.contains("forbidden"), "{message}");
        }
        Err(other) => panic!("expected BackendRejected, got {other:?}"),
        Ok(_) => panic!("expected BackendRejected, got Ok"),
    }
    access.assert_calls(0);
}

#[tokio::test]
async fn a_callback_without_an_outcome_is_invalid_input() {
    let server = MockServer::start();
    let access = mount_sso_session(&server);
    for callback in [
        "",
        "wire-kupferdraht://sso",
        "wire-kupferdraht://sso?credential=%24cookie",
    ] {
        assert_invalid(
            builder(&server, &home())
                .login_with_sso_callback(callback)
                .await,
            &format!("callback {callback:?}"),
        );
    }
    access.assert_calls(0);
}

// ---- typing -----------------------------------------------------------

async fn room_in_cache(server: &MockServer) -> Arc<kupferdraht::Room> {
    common::mount_full_login_flow(server);
    common::mount_empty_notifications(server);
    mount_room(server);
    let (client, _store) = common::run_login(server).await;
    client
        .sync_once(SyncSettings::default())
        .await
        .expect("initial sync");
    let room = client.room(&room_id()).await.expect("the room after sync");
    // The room holds its client weakly; keep it alive for the test.
    let _ = Box::leak(Box::new(client));
    room
}

fn mount_typing<'a>(server: &'a MockServer, status: &str) -> httpmock::Mock<'a> {
    let body = format!(r#""status":"{status}""#);
    server.mock(move |when, then| {
        when.method(POST)
            .path(format!("/v14/conversations/{DOMAIN}/{ROOM_UUID}/typing"))
            .body_includes(body.clone());
        then.status(200);
    })
}

#[tokio::test]
async fn typing_notice_true_starts_and_false_stops() {
    let server = MockServer::start();
    let room = room_in_cache(&server).await;
    let started = mount_typing(&server, "started");
    let stopped = mount_typing(&server, "stopped");

    room.typing_notice(true).await.expect("typing");
    room.typing_notice(false).await.expect("not typing");

    started.assert_calls(1);
    stopped.assert_calls(1);
}

#[tokio::test]
async fn a_repeated_true_sends_one_start() {
    let server = MockServer::start();
    let room = room_in_cache(&server).await;
    let started = mount_typing(&server, "started");

    room.typing_notice(true).await.expect("typing");
    room.typing_notice(true).await.expect("still typing");

    started.assert_calls(1);
}

// ---- the email change -------------------------------------------------

#[tokio::test]
async fn request_email_change_asks_the_backend_to_confirm_the_address() {
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    let (client, _store) = common::run_login(&server).await;
    // The backend wants the session cookie beside the token here: without it
    // `403 invalid-credentials` "Missing cookie" (live-confirmed,
    // `docs/observations.md` §107).
    let change = server.mock(|when, then| {
        when.method(PUT)
            .path("/v14/access/self/email")
            .header("cookie", "zuid=test")
            .body_includes(r#""email":"new@example.test""#);
        then.status(202);
    });

    client
        .request_email_change("new@example.test")
        .await
        .expect("the backend acknowledges");

    change.assert_calls(1);
    assert_eq!(
        client.account().email(),
        Some("alice@example.test"),
        "the address changes only once the user confirms"
    );
    assert_invalid(client.request_email_change("").await, "an empty address");
}
