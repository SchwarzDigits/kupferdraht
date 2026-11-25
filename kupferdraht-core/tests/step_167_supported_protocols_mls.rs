// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 167 — the account says it speaks MLS (TDD red).
//!
//! brig keeps `supported_protocols` per user and starts a new account with
//! `["proteus"]`; Wire's clients pick a 1:1's protocol from the intersection
//! of both users' lists, so a Kupferdraht user looks Proteus-only to the webapp
//! (observations §94). A login now declares `["mls"]` with
//! `PUT /self/supported-protocols` unless `GET /self` says so already; the
//! declaration is an enrichment like the feature configs (ADR-0012): a
//! failure is logged and the login stands. `Account::supported_protocols`
//! is the list as of the last login and comes back at `resume`.

use std::sync::{Arc, Mutex};

use httpmock::Method::PUT;
use httpmock::{Mock, MockServer};
use kupferdraht::keystore::{InMemoryKeyStore, SecureKeyStore};
use kupferdraht::{Client, Kupferdraht};
use tempfile::TempDir;

mod common;

const QUALIFIED: &str = "user-uuid@example.test";

/// `GET /self` with `supported_protocols` as the JSON list given, or
/// without the field.
fn self_body(protocols: Option<&str>) -> String {
    let protocols = protocols.map_or(String::new(), |p| format!(r#","supported_protocols":{p}"#));
    format!(
        r#"{{"id":"user-uuid","qualified_id":{{"id":"user-uuid","domain":"example.test"}},"handle":"alice","name":"Alice Example","email":"alice@example.test"{protocols}}}"#
    )
}

/// `PUT /v14/self/supported-protocols` answering `status`; every body it
/// receives is kept.
fn mount_declaration(server: &MockServer, status: u16) -> (Mock<'_>, Arc<Mutex<Vec<String>>>) {
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let sink = bodies.clone();
    let mock = server.mock(move |when, then| {
        when.method(PUT)
            .path("/v14/self/supported-protocols")
            .is_true(move |req| {
                sink.lock()
                    .expect("bodies")
                    .push(String::from_utf8_lossy(req.body_ref()).to_string());
                true
            });
        then.status(status)
            .header("content-type", "application/json")
            .body(if status >= 400 {
                r#"{"code":500,"label":"server-error","message":"refused"}"#
            } else {
                ""
            });
    });
    (mock, bodies)
}

struct Session {
    _home: TempDir,
    app: Kupferdraht,
    client: Arc<Client>,
}

async fn login(server: &MockServer, protocols: Option<&str>) -> Session {
    common::login_mocks::mount_login_with_self(server, &self_body(protocols));
    common::login_mocks::mount_device_registration(server);
    let home = TempDir::new().expect("tempdir");
    let keystore: Arc<dyn SecureKeyStore> = Arc::new(InMemoryKeyStore::new());
    let app = Kupferdraht::open(home.path().to_path_buf(), keystore)
        .expect("open kupferdraht")
        .permit_http();
    let client = app
        .login(
            common::server_config(&server.base_url()),
            "alice@example.test".into(),
            "pw".into(),
            None,
            None,
        )
        .await
        .expect("login")
        .register_device()
        .await
        .expect("register device");
    Session {
        _home: home,
        app,
        client,
    }
}

fn declared(bodies: &Mutex<Vec<String>>) -> Vec<serde_json::Value> {
    bodies
        .lock()
        .expect("bodies")
        .iter()
        .map(|b| serde_json::from_str(b).expect("a JSON body"))
        .collect()
}

/// A new account says `["proteus"]`: the login declares `["mls"]` exactly
/// once, with nothing else in the list, and the account reads `["mls"]`.
#[tokio::test]
async fn an_account_that_says_proteus_is_declared_mls_once() {
    let server = MockServer::start();
    let (put, bodies) = mount_declaration(&server, 200);

    let session = login(&server, Some(r#"["proteus"]"#)).await;

    put.assert_calls(1);
    assert_eq!(
        declared(&bodies),
        vec![serde_json::json!({ "supported_protocols": ["mls"] })]
    );
    assert_eq!(session.client.account().supported_protocols(), ["mls"]);
}

/// An account that says `["mls"]` already is left alone: no request.
#[tokio::test]
async fn an_account_that_says_mls_is_not_declared_again() {
    let server = MockServer::start();
    let (put, _bodies) = mount_declaration(&server, 200);

    let session = login(&server, Some(r#"["mls"]"#)).await;

    put.assert_calls(0);
    assert_eq!(session.client.account().supported_protocols(), ["mls"]);
}

/// `["proteus", "mls"]` is not `["mls"]`: Kupferdraht speaks MLS alone and
/// says so.
#[tokio::test]
async fn an_account_that_says_both_is_narrowed_to_mls() {
    let server = MockServer::start();
    let (put, bodies) = mount_declaration(&server, 200);

    let session = login(&server, Some(r#"["proteus","mls"]"#)).await;

    put.assert_calls(1);
    assert_eq!(
        declared(&bodies),
        vec![serde_json::json!({ "supported_protocols": ["mls"] })]
    );
    assert_eq!(session.client.account().supported_protocols(), ["mls"]);
}

/// A `/self` without the field is treated as not yet declared.
#[tokio::test]
async fn an_account_without_the_field_is_declared() {
    let server = MockServer::start();
    let (put, _bodies) = mount_declaration(&server, 200);

    let session = login(&server, None).await;

    put.assert_calls(1);
    assert_eq!(session.client.account().supported_protocols(), ["mls"]);
}

/// A refused declaration is logged; the login stands, and the account
/// reads what the backend said.
#[tokio::test]
async fn a_refused_declaration_leaves_the_login_standing() {
    let server = MockServer::start();
    let (put, _bodies) = mount_declaration(&server, 500);

    let session = login(&server, Some(r#"["proteus"]"#)).await;

    put.assert_calls(1);
    assert_eq!(session.client.account().supported_protocols(), ["proteus"]);
}

/// The list is a snapshot of the last login and comes back at `resume`.
#[tokio::test]
async fn a_resume_has_the_list_of_the_last_login() {
    let server = MockServer::start();
    let (_put, _bodies) = mount_declaration(&server, 200);
    let session = login(&server, Some(r#"["proteus"]"#)).await;
    drop(session.client);

    let resumed = session.app.resume(QUALIFIED).await.expect("resume");

    assert_eq!(resumed.account().supported_protocols(), ["mls"]);
}
