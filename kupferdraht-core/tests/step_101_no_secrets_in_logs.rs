// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 101 — no secrets in logs (review finding H6). TDD red.
//!
//! The login logged the full `Set-Cookie` headers — the `zuid` refresh
//! cookie, which is a session on its own — at debug, in a warning, and
//! in the text of the error a consumer receives when the cookie is
//! missing; a further debug line carried the cookie's first characters.
//! Every event is captured here (a global subscriber, the Step-95a
//! pattern) and searched for a sentinel planted in the cookie.

use std::sync::{Arc, Mutex};

use httpmock::Method::POST;
use httpmock::MockServer;
use kupferdraht::keystore::{InMemoryKeyStore, SecureKeyStore};
use kupferdraht::{ClientBuilder, KupferdrahtError};

mod common;

/// Minimal capturing `tracing::Subscriber` — records the rendered fields
/// of every event Kupferdraht emits; installed once as the global default.
#[derive(Default)]
struct CaptureSubscriber {
    events: Arc<Mutex<Vec<String>>>,
}

impl tracing::Subscriber for CaptureSubscriber {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        // Kupferdraht's own events only — the mock server logs the requests it
        // receives (cookies included); that is the test fixture talking.
        if !event.metadata().target().starts_with("kupferdraht") {
            return;
        }
        let mut rendered = String::new();
        event.record(&mut FieldRenderer(&mut rendered));
        self.events.lock().expect("capture lock").push(rendered);
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

struct FieldRenderer<'a>(&'a mut String);

impl tracing::field::Visit for FieldRenderer<'_> {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        use std::fmt::Write as _;
        let _ = write!(self.0, "{}={value:?} ", field.name());
    }
}

fn captured_events() -> Arc<Mutex<Vec<String>>> {
    static EVENTS: std::sync::OnceLock<Arc<Mutex<Vec<String>>>> = std::sync::OnceLock::new();
    EVENTS
        .get_or_init(|| {
            let subscriber = CaptureSubscriber::default();
            let events = Arc::clone(&subscriber.events);
            tracing::subscriber::set_global_default(subscriber)
                .expect("the only global subscriber of this test binary");
            events
        })
        .clone()
}

fn events_mentioning(events: &Arc<Mutex<Vec<String>>>, needle: &str) -> Vec<String> {
    events
        .lock()
        .expect("capture lock")
        .iter()
        .filter(|e| e.contains(needle))
        .cloned()
        .collect()
}

/// `POST /v14/login` answering with `set_cookie`, mounted before the
/// shared login fixture (the first matching mock wins).
fn mount_login_with_cookie(server: &MockServer, set_cookie: &str) {
    let set_cookie = set_cookie.to_string();
    server.mock(move |when, then| {
        when.method(POST).path("/v14/login");
        then.status(200)
            .header("content-type", "application/json")
            .header("set-cookie", set_cookie.as_str())
            .body(r#"{"access_token":"initial.jwt.token","token_type":"Bearer","expires_in":900,"user":"user-uuid"}"#);
    });
}

fn keystore() -> Arc<dyn SecureKeyStore> {
    Arc::new(InMemoryKeyStore::new())
}

/// A successful login: the `zuid` cookie value appears in no log event.
#[tokio::test]
async fn the_login_cookie_never_reaches_the_log() {
    let events = captured_events();
    let server = MockServer::start();
    let secret = "zuid-SECRET-101-A";
    mount_login_with_cookie(
        &server,
        &format!("zuid={secret}; HttpOnly; Secure; Path=/access"),
    );
    server.mock(move |when, then| {
        when.method(POST).path("/v14/access");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"access_token":"rebound.jwt.token","token_type":"Bearer","expires_in":900,"user":"user-uuid"}"#);
    });
    common::mount_full_login_flow(&server);
    let home = tempfile::tempdir().expect("tempdir");
    ClientBuilder::new()
        .server(common::server_config(&server.base_url()))
        .permit_http()
        .base_dir(home.path().to_path_buf())
        .keystore(keystore())
        .login("alice@example.test".into(), "pw".into(), None)
        .await
        .expect("login")
        .register_device()
        .await
        .expect("register device");

    assert_eq!(
        events_mentioning(&events, "SECRET-101-A"),
        Vec::<String>::new(),
        "no log event carries the zuid cookie"
    );
    assert_eq!(
        events_mentioning(&events, "zuid-SEC"),
        Vec::<String>::new(),
        "not even its first characters"
    );
}

/// A login response without a `zuid`: neither the warning nor the error
/// the consumer receives echoes the other cookies' values.
#[tokio::test]
async fn a_login_without_a_zuid_echoes_no_cookie_value() {
    let events = captured_events();
    let server = MockServer::start();
    mount_login_with_cookie(&server, "session=SECRET-101-B; HttpOnly; Secure");
    common::mount_full_login_flow(&server);
    let home = tempfile::tempdir().expect("tempdir");
    let result = ClientBuilder::new()
        .server(common::server_config(&server.base_url()))
        .permit_http()
        .base_dir(home.path().to_path_buf())
        .keystore(keystore())
        .login("alice@example.test".into(), "pw".into(), None)
        .await;

    let Err(err) = result else {
        panic!("a login without zuid fails");
    };
    assert!(
        matches!(err, KupferdrahtError::BackendRejected(_)),
        "rejected: {err:?}"
    );
    assert!(
        !err.to_string().contains("SECRET-101-B"),
        "the error names no cookie value: {err}"
    );
    assert_eq!(
        events_mentioning(&events, "SECRET-101-B"),
        Vec::<String>::new(),
        "no log event carries a cookie value"
    );
}
