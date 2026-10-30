// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Shared test fixtures for kupferdraht-core integration tests.

pub mod drain_fixture;
pub mod faulty_store;
pub mod keystore;
pub mod login_mocks;
pub mod mls;

use std::sync::Arc;

use httpmock::Method::GET;
use httpmock::MockServer;
use kupferdraht::keystore::{InMemoryKeyStore, SecureKeyStore};
use kupferdraht::storage::Store;
use kupferdraht::{Client, ClientBuilder, ServerConfig};
use tempfile::TempDir;

/// Build a `ServerConfig` whose `api_base_url` and `websocket_url`
/// both point at the given mock base URL. The 17 integration tests
/// against `httpmock` all used to define this identically locally;
/// lift it here.
#[allow(dead_code)]
pub fn server_config(base_url: &str) -> ServerConfig {
    ServerConfig {
        api_base_url: base_url.to_string(),
        websocket_url: base_url.to_string(),
        title: "mock".to_string(),
    }
}

/// Drive a full `ClientBuilder::login(...)` against a pre-mocked
/// `MockServer` (see [`mount_full_login_flow`]) and return the ready
/// `Client` together with its SDK-opened [`Store`].
///
/// The temporary home directory backing the store is leaked so the store
/// stays valid past this function — the OS reaps it on process exit.
///
/// Callers that only need the `Client` can destructure `(client, _)`.
#[allow(dead_code)]
pub async fn run_login(server: &MockServer) -> (Arc<Client>, Arc<dyn Store>) {
    let home = TempDir::new().expect("TempDir");
    let home_path = home.path().to_path_buf();
    let _ = Box::leak(Box::new(home));
    let keystore: Arc<dyn SecureKeyStore> = Arc::new(InMemoryKeyStore::new());
    let client = ClientBuilder::new()
        .server(server_config(&server.base_url()))
        .permit_http()
        .base_dir(home_path)
        .keystore(keystore)
        .login("alice@example.test".into(), "pw".into(), None)
        .await
        .expect("login")
        .register_device()
        .await
        .expect("register device");
    let store = client.store().expect("client has an SDK-opened store");
    (client, store)
}

/// Mount the eight HTTP mocks that cover a full login flow:
/// api-version → login → self → clients (POST) → clients (PUT) →
/// access → mls/key-packages → mls/public-keys ([`login_mocks`]).
///
/// Call once per `MockServer` before invoking `ClientBuilder::login`.
#[allow(dead_code)]
pub fn mount_full_login_flow(server: &MockServer) {
    login_mocks::mount_full_login_flow(server);
}

/// Mount `/v14/notifications` returning an empty queue so the
/// notifications-drain phase of `Client::sync_once` succeeds in tests
/// that only care about the metadata-refresh half. Use alongside the
/// existing conversations / list-users mocks.
#[allow(dead_code)]
pub fn mount_empty_notifications(server: &MockServer) {
    server.mock(|when, then| {
        when.method(GET).path("/v14/notifications");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"notifications":[],"has_more":false}"#);
    });
}
