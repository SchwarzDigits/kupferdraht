// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 73 — legacy `/await` push transport (TDD red; ADR-0019).
//!
//! `SyncTransport::WebSocket` must auto-negotiate the concrete
//! socket: `server_supports_push_events()` false (every current
//! wire-server) → the legacy cannon `/await` socket — Bearer-header
//! auth, `client=` param, no URL token, binary frames each carrying
//! one `QueuedNotification` (the `/v14/notifications` item shape,
//! observations §68). Since Step 97b (ADR-0030, superseding
//! ADR-0019's drain-then-stream and cursor-per-frame) a durable frame
//! only rings the REST drain — `step_97b_push_doorbell.rs` covers the
//! doorbell itself. The `/v14/events` dialect stays untouched for
//! feature-enabled servers (day-0 green guard). Harness mirrors
//! `step_62b_push_loop.rs`, extended to capture the `Authorization`
//! header off the upgrade request.

use std::ops::ControlFlow;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use httpmock::Method::{GET, POST};
use httpmock::MockServer;
use kupferdraht::keystore::{InMemoryKeyStore, SecureKeyStore};
use kupferdraht::storage::Store;
use kupferdraht::sync::{EventHandler, Message, TypingEvent};
use kupferdraht::{Client, ClientBuilder, ServerConfig, SyncSettings, SyncTransport};
use tempfile::TempDir;
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};

mod common;

const TEST_TIMEOUT: Duration = Duration::from_secs(10);

async fn run_login_with_ws(server: &MockServer, ws_base: &str) -> (Arc<Client>, Arc<dyn Store>) {
    let home = TempDir::new().expect("TempDir");
    let home_path = home.path().to_path_buf();
    let _ = Box::leak(Box::new(home));
    let keystore: Arc<dyn SecureKeyStore> = Arc::new(InMemoryKeyStore::new());
    let client = ClientBuilder::new()
        .server(ServerConfig {
            api_base_url: server.base_url(),
            websocket_url: ws_base.to_string(),
            title: "mock".to_string(),
        })
        .permit_http()
        .base_dir(home_path)
        .keystore(keystore)
        .login("alice@example.test".into(), "pw".into(), None)
        .await
        .expect("login")
        .register_device()
        .await
        .expect("register device");
    let store = client.store().expect("store");
    (client, store)
}

fn mount_rooms(server: &MockServer) {
    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list-ids");
        then.status(200)
            .header("content-type", "application/json")
            .body(
                r#"{"qualified_conversations":[{"id":"room-a","domain":"example.test"}],
                     "has_more":false,"paging_state":"END"}"#,
            );
    });
    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list");
        then.status(200)
            .header("content-type", "application/json")
            .body(
                r#"{"found":[{
                     "qualified_id":{"id":"room-a","domain":"example.test"},
                     "type":0,"protocol":"mls","epoch":1,"group_id":"Z3JwLW9uZQ==",
                     "name":"Step 73",
                     "members":{"self":{"qualified_id":{"id":"user-uuid","domain":"example.test"}},"others":[]}
                   }], "not_found":[], "failed":[]}"#,
            );
    });
}

fn mount_empty_rest_queue(server: &MockServer) {
    server.mock(|when, then| {
        when.method(GET).path("/v14/notifications");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"notifications":[],"has_more":false}"#);
    });
}

/// A legacy frame IS a `/v14/notifications` item (observations §68):
/// `{id, payload:[event], transient}` — typing here so no MLS state
/// is needed and it is observable via `on_typing`.
fn legacy_typing_frame(notif_id: &str, from_uuid: &str, transient: bool) -> String {
    format!(
        r#"{{"id":"{notif_id}","payload":[{{"type":"conversation.typing","qualified_conversation":{{"id":"room-a","domain":"example.test"}},"qualified_from":{{"id":"{from_uuid}","domain":"example.test"}},"data":{{"status":"started"}}}}],"transient":{transient}}}"#
    )
}

/// `/v14/events` §11.2 frame (the 62b shape) for the enabled-guard.
fn events_typing_frame(delivery_tag: u64, notif_id: &str) -> String {
    format!(
        r#"{{"type":"event","data":{{"delivery_tag":{delivery_tag},"event":{{"id":"{notif_id}","payload":[{{"type":"conversation.typing","qualified_conversation":{{"id":"room-a","domain":"example.test"}},"qualified_from":{{"id":"bob-uuid","domain":"example.test"}},"data":{{"status":"started"}}}}]}}}}}}"#
    )
}

/// Accept one WebSocket connection; hand back the stream, the request
/// URI, and the `Authorization` header (Step 73: the Bearer must ride
/// the header, never the URL).
async fn accept_with_request(
    listener: &TcpListener,
) -> (WebSocketStream<TcpStream>, String, Option<String>) {
    let (stream, _) = listener.accept().await.expect("accept");
    let (tx, rx) = tokio::sync::oneshot::channel::<(String, Option<String>)>();
    #[allow(clippy::result_large_err)]
    let capture = move |req: &Request, resp: Response| {
        let auth = req
            .headers()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        tx.send((req.uri().to_string(), auth)).expect("send req");
        Ok(resp)
    };
    let ws = tokio_tungstenite::accept_hdr_async(stream, capture)
        .await
        .expect("upgrade");
    let (uri, auth) = rx.await.expect("request captured");
    (ws, uri, auth)
}

async fn drain_until_closed(mut ws: WebSocketStream<TcpStream>) {
    while let Some(msg) = ws.next().await {
        if msg.is_err() {
            break;
        }
    }
}

fn query_param(uri: &str, key: &str) -> Option<String> {
    let (_, query) = uri.split_once('?')?;
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == key).then(|| v.to_string())
    })
}

#[derive(Default)]
struct RecordingHandler {
    typing: Mutex<Vec<TypingEvent>>,
}

impl EventHandler for RecordingHandler {
    fn on_message(&self, _message: Message) {}
    fn on_room_invited(&self, _room_id: String) {}
    fn on_sync_error(&self, _description: String) {}
    fn on_typing(&self, event: TypingEvent) {
        self.typing.lock().expect("typing lock").push(event);
    }
}

fn websocket_settings() -> SyncSettings {
    SyncSettings {
        poll_interval: Some(Duration::from_millis(1)),
        transport: SyncTransport::WebSocket,
    }
}

/// Run the push loop until `handler` has recorded `want` typing
/// events (or the timeout trips).
async fn run_until_typing(client: &Arc<Client>, handler: &Arc<RecordingHandler>, want: usize) {
    let h = Arc::clone(handler);
    timeout(
        TEST_TIMEOUT,
        client.sync_with_callback(websocket_settings(), move |_summary| {
            if h.typing.lock().expect("typing lock").len() >= want {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        }),
    )
    .await
    .expect("push loop must finish within the timeout")
    .expect("Break must exit the push loop with Ok");
}

#[tokio::test]
async fn negotiation_connects_await_when_consumable_disabled() {
    let server = MockServer::start();
    // mount_full_login_flow's feature-configs answer carries no
    // consumableNotifications entry → server_supports_push_events()
    // is false → ADR-0019 says: dial the legacy socket.
    common::mount_full_login_flow(&server);
    mount_rooms(&server);
    mount_empty_rest_queue(&server);

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let ws_base = format!("ws://{}", listener.local_addr().expect("addr"));
    let ws_server = tokio::spawn(async move {
        let (mut ws, uri, auth) = accept_with_request(&listener).await;
        assert!(
            uri.starts_with("/await"),
            "feature-disabled server → legacy socket, got: {uri}"
        );
        assert!(
            query_param(&uri, "client").is_some_and(|c| !c.is_empty()),
            "client= must scope the queue: {uri}"
        );
        assert!(
            query_param(&uri, "access_token").is_none(),
            "the token must never ride the URL: {uri}"
        );
        assert!(
            auth.as_deref().is_some_and(|a| a.starts_with("Bearer ")),
            "Bearer header expected, got: {auth:?}"
        );
        // Transient: dispatched directly. A durable frame would only
        // ring the REST drain (ADR-0030) — covered below.
        ws.send(WsMessage::binary(
            legacy_typing_frame("await-n1", "bob-uuid", true).into_bytes(),
        ))
        .await
        .expect("send frame");
        drain_until_closed(ws).await;
    });

    let (client, _store) = run_login_with_ws(&server, &ws_base).await;
    let handler = Arc::new(RecordingHandler::default());
    client.add_event_handler(handler.clone());
    run_until_typing(&client, &handler, 1).await;

    timeout(TEST_TIMEOUT, ws_server)
        .await
        .expect("ws server must finish")
        .expect("ws server task");
    let typing = handler.typing.lock().expect("typing lock");
    assert_eq!(typing.len(), 1, "the legacy frame reached on_typing");
    assert_eq!(typing[0].typing_users, vec!["bob-uuid@example.test"]);
}

/// ADR-0030: a durable frame only rings the REST drain — the drained
/// item dispatches and moves the cursor, the frame's own copy never
/// dispatches a second time; a transient frame dispatches directly
/// and leaves the cursor alone.
#[tokio::test]
async fn durable_frames_ring_the_drain_and_transient_frames_skip_the_cursor() {
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    mount_rooms(&server);
    server.mock(|when, then| {
        when.method(GET).path("/v14/notifications");
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(
                r#"{{"notifications":[{}],"has_more":false}}"#,
                legacy_typing_frame("await-durable-1", "bob-uuid", false)
            ));
    });

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let ws_base = format!("ws://{}", listener.local_addr().expect("addr"));
    let ws_server = tokio::spawn(async move {
        let (mut ws, uri, _auth) = accept_with_request(&listener).await;
        assert!(uri.starts_with("/await"), "uri: {uri}");
        ws.send(WsMessage::binary(
            legacy_typing_frame("await-durable-1", "bob-uuid", false).into_bytes(),
        ))
        .await
        .expect("send durable");
        ws.send(WsMessage::binary(
            legacy_typing_frame("await-transient-2", "carol-uuid", true).into_bytes(),
        ))
        .await
        .expect("send transient");
        drain_until_closed(ws).await;
    });

    let (client, store) = run_login_with_ws(&server, &ws_base).await;
    let handler = Arc::new(RecordingHandler::default());
    client.add_event_handler(handler.clone());
    run_until_typing(&client, &handler, 2).await;

    timeout(TEST_TIMEOUT, ws_server)
        .await
        .expect("ws server must finish")
        .expect("ws server task");

    {
        let typing = handler.typing.lock().expect("typing lock");
        assert_eq!(typing.len(), 2, "bob once through the drain, carol once");
        assert_eq!(typing[0].typing_users, vec!["bob-uuid@example.test"]);
        assert!(
            typing[1]
                .typing_users
                .contains(&"carol-uuid@example.test".to_string()),
            "the transient frame dispatched directly"
        );
    }
    let state = store
        .load_sync_state()
        .await
        .expect("load sync state")
        .expect("sync state persisted");
    assert_eq!(
        state.last_event_id.as_deref(),
        Some("await-durable-1"),
        "the drain moves the cursor; the transient frame must not"
    );
}

/// ADR-0030 d.1: connect, then drain — the REST backlog arrives
/// through the connect drain; a transient frame read meanwhile
/// dispatches after it.
#[tokio::test]
async fn connect_drains_the_rest_backlog() {
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    mount_rooms(&server);
    server.mock(|when, then| {
        when.method(GET).path("/v14/notifications");
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(
                r#"{{"notifications":[{}],"has_more":false}}"#,
                legacy_typing_frame("rest-n1", "rest-uuid", false)
            ));
    });

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let ws_base = format!("ws://{}", listener.local_addr().expect("addr"));
    let ws_server = tokio::spawn(async move {
        let (mut ws, uri, _auth) = accept_with_request(&listener).await;
        assert!(uri.starts_with("/await"), "uri: {uri}");
        ws.send(WsMessage::binary(
            legacy_typing_frame("await-n2", "sock-uuid", true).into_bytes(),
        ))
        .await
        .expect("send socket frame");
        drain_until_closed(ws).await;
    });

    let (client, _store) = run_login_with_ws(&server, &ws_base).await;
    let handler = Arc::new(RecordingHandler::default());
    client.add_event_handler(handler.clone());
    run_until_typing(&client, &handler, 2).await;

    timeout(TEST_TIMEOUT, ws_server)
        .await
        .expect("ws server must finish")
        .expect("ws server task");

    let typing = handler.typing.lock().expect("typing lock");
    assert_eq!(typing.len(), 2);
    assert_eq!(
        typing[0].typing_users,
        vec!["rest-uuid@example.test"],
        "the REST backlog arrives through the connect drain"
    );
    assert!(
        typing[1]
            .typing_users
            .contains(&"sock-uuid@example.test".to_string()),
        "the transient frame follows"
    );
}

#[tokio::test]
async fn negotiation_keeps_events_when_consumable_enabled() {
    let server = MockServer::start();
    // Feature enabled — mounted BEFORE the login flow so this
    // specific mock wins (httpmock: first registered wins).
    server.mock(|when, then| {
        when.method(GET).path("/v14/feature-configs");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"consumableNotifications":{"status":"enabled","lockStatus":"unlocked"}}"#);
    });
    common::mount_full_login_flow(&server);
    mount_rooms(&server);

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let ws_base = format!("ws://{}", listener.local_addr().expect("addr"));
    let ws_server = tokio::spawn(async move {
        let (mut ws, uri, _auth) = accept_with_request(&listener).await;
        assert!(
            uri.starts_with("/v14/events?"),
            "feature-enabled server keeps the ADR-0017 socket, got: {uri}"
        );
        ws.send(WsMessage::text(events_typing_frame(1, "push-n1")))
            .await
            .expect("send event frame");
        drain_until_closed(ws).await;
    });

    let (client, _store) = run_login_with_ws(&server, &ws_base).await;
    let handler = Arc::new(RecordingHandler::default());
    client.add_event_handler(handler.clone());
    run_until_typing(&client, &handler, 1).await;

    timeout(TEST_TIMEOUT, ws_server)
        .await
        .expect("ws server must finish")
        .expect("ws server task");
    let typing = handler.typing.lock().expect("typing lock");
    assert_eq!(typing.len(), 1);
}
