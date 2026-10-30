// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 118 (D4, D5) — the native push loop behaves like the browser's,
//! and push is the native default (ADR-0033). TDD red.
//!
//! Before: a socket that could not be opened stopped receive entirely —
//! the loop only retried the connect, with a backoff doubling from 5 s to
//! 30 s, and never drained. Now a failed connect drains anyway, and the
//! loop connects at most once per push pace (`poll_interval`, 10 s when
//! unset) — like the browser, which polls and retries every 10 s without
//! a socket.
//!
//! Harness: the Step-97b one — a loopback TCP listener plays the push
//! service (or refuses to), httpmock plays the REST queue.

use std::ops::ControlFlow;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use httpmock::Method::{GET, POST};
use httpmock::MockServer;
use kupferdraht::keystore::{InMemoryKeyStore, SecureKeyStore};
use kupferdraht::sync::{EventHandler, Message, TypingEvent};
use kupferdraht::{Client, Kupferdraht, ServerConfig, SyncSettings, SyncTransport};
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::time::timeout;

mod common;

async fn logged_in_client(server: &MockServer, ws_base: &str) -> (Arc<Client>, TempDir) {
    let home = TempDir::new().expect("TempDir");
    let keystore: Arc<dyn SecureKeyStore> = Arc::new(InMemoryKeyStore::new());
    let app = Kupferdraht::open(home.path().to_path_buf(), keystore)
        .expect("open kupferdraht")
        .permit_http();
    let server_config = ServerConfig {
        api_base_url: server.base_url(),
        websocket_url: ws_base.to_string(),
        title: "mock".to_string(),
    };
    let client = app
        .login(
            server_config,
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
    (client, home)
}

fn mount_rooms(server: &MockServer) {
    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list-ids");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"qualified_conversations":[],"has_more":false,"paging_state":"END"}"#);
    });
}

/// A REST page with one typing event from `from_uuid` — observable through
/// `on_typing` without MLS state.
fn mount_typing_once(server: &MockServer, from_uuid: &str) {
    let body = format!(
        r#"{{"notifications":[{{"id":"n1","payload":[{{"type":"conversation.typing","qualified_conversation":{{"id":"room-a","domain":"example.test"}},"qualified_from":{{"id":"{from_uuid}","domain":"example.test"}},"data":{{"status":"started"}}}}],"transient":false}}],"has_more":false}}"#
    );
    server.mock(move |when, then| {
        when.method(GET)
            .path("/v14/notifications")
            .is_true(|req| !req.query_params_map().contains_key("since"));
        then.status(200)
            .header("content-type", "application/json")
            .body(body);
    });
    common::mount_empty_notifications(server);
}

#[derive(Default)]
struct TypingSeen(Mutex<Vec<String>>);

impl EventHandler for TypingSeen {
    fn on_message(&self, _message: Message) {}
    fn on_room_invited(&self, _room_id: String) {}
    fn on_sync_error(&self, _description: String) {}
    fn on_typing(&self, event: TypingEvent) {
        self.0.lock().expect("typing").extend(event.typing_users);
    }
}

fn push_settings(pace: Duration) -> SyncSettings {
    SyncSettings {
        poll_interval: Some(pace),
        transport: SyncTransport::WebSocket,
    }
}

/// Run the push loop for `window`, whatever it does.
async fn push_for(client: &Arc<Client>, settings: SyncSettings, window: Duration) {
    let _ = timeout(
        window,
        client.sync_with_callback(settings, |_summary| ControlFlow::Continue(())),
    )
    .await;
}

/// A websocket base nothing listens on: every connect is refused.
async fn refused_ws_base() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let base = format!("ws://{}", listener.local_addr().expect("addr"));
    drop(listener);
    base
}

/// ADR-0033: a consumer that does not choose gets push on native targets.
#[test]
fn push_is_the_native_default() {
    assert_eq!(SyncTransport::default(), SyncTransport::WebSocket);
    assert_eq!(SyncSettings::default().transport, SyncTransport::WebSocket);
}

/// The regression push-by-default must not bring: a network that blocks
/// the socket still receives, through the drain.
#[tokio::test]
async fn without_a_socket_a_message_still_arrives() {
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    mount_rooms(&server);
    mount_typing_once(&server, "bob-uuid");
    let ws_base = refused_ws_base().await;
    let (client, _home) = logged_in_client(&server, &ws_base).await;
    let seen = Arc::new(TypingSeen::default());
    client.add_event_handler(seen.clone());

    push_for(
        &client,
        push_settings(Duration::from_millis(100)),
        Duration::from_secs(2),
    )
    .await;

    assert!(
        seen.0
            .lock()
            .expect("typing")
            .contains(&"bob-uuid@example.test".to_string()),
        "the drain delivered the event although no socket opened"
    );
}

/// Without a socket the loop tries again at the push pace — not with a
/// backoff that doubles while receive waits.
#[tokio::test]
async fn without_a_socket_the_loop_retries_at_the_pace() {
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    mount_rooms(&server);
    common::mount_empty_notifications(&server);
    // Accepts the TCP connection and drops it: every upgrade fails.
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let ws_base = format!("ws://{}", listener.local_addr().expect("addr"));
    let attempts = Arc::new(AtomicUsize::new(0));
    let count = attempts.clone();
    let refuser = tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.expect("accept");
            count.fetch_add(1, Ordering::SeqCst);
            drop(stream);
        }
    });
    let (client, _home) = logged_in_client(&server, &ws_base).await;

    push_for(
        &client,
        push_settings(Duration::from_millis(200)),
        Duration::from_millis(1600),
    )
    .await;
    refuser.abort();

    // At the pace: about one attempt per 200 ms. A doubling backoff from
    // 200 ms manages four (0, 0.2, 0.6, 1.4 s).
    assert!(
        attempts.load(Ordering::SeqCst) >= 6,
        "connect attempts in 1.6 s: {}",
        attempts.load(Ordering::SeqCst)
    );
}

/// A server that accepts and closes at once cannot spin the loop: the
/// next connect waits out the pace.
#[tokio::test]
async fn a_connection_closed_at_once_is_not_retried_before_the_pace() {
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    mount_rooms(&server);
    common::mount_empty_notifications(&server);
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let ws_base = format!("ws://{}", listener.local_addr().expect("addr"));
    let accepted_at = Arc::new(Mutex::new(Vec::<Instant>::new()));
    let times = accepted_at.clone();
    let closer = tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.expect("accept");
            let mut ws = tokio_tungstenite::accept_async(stream)
                .await
                .expect("upgrade");
            times.lock().expect("times").push(Instant::now());
            let _ = ws.close(None).await;
        }
    });
    let (client, _home) = logged_in_client(&server, &ws_base).await;

    push_for(
        &client,
        push_settings(Duration::from_millis(500)),
        Duration::from_millis(1300),
    )
    .await;
    closer.abort();

    let times = accepted_at.lock().expect("times").clone();
    assert!(times.len() >= 2, "the loop reconnected: {}", times.len());
    assert!(
        times[1].duration_since(times[0]) >= Duration::from_millis(450),
        "reconnected after {:?}",
        times[1].duration_since(times[0])
    );
}
