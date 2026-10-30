// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 62b — `/v14/events` push loop (TDD red phase).
//!
//! Pins the public contract from `docs/plan/62-events-websocket.md`
//! §"Sub-step 62b" against an in-process WebSocket server (frames
//! verbatim from `docs/backend-spec.md` §11.2/§11.3) plus `httpmock`
//! for the HTTP side:
//!
//! * `SyncSettings.transport` defaults to `Polling` — existing callers
//!   keep their behavior.
//! * `SyncTransport::WebSocket` drives the loop variants over the
//!   events socket: per event frame one dispatch through the existing
//!   `EventHandler` pipeline and one callback tick; **no**
//!   `/v14/notifications` call (the tests mount no such mock, so a
//!   REST fallback would 404 and fail loudly).
//! * Ack follows durable processing: when the server holds the ack,
//!   the `last_event_id` cursor is already persisted.
//! * The `synchronization` frame persists the connection's
//!   `sync_marker` echo into `sync_state.sync_marker_uuid`.
//! * A redelivered notification (same `id`, new `delivery_tag`) is
//!   acked but not re-dispatched.
//! * A dropped connection reconnects with a **fresh** `sync_marker`
//!   and without surfacing a hard error.
//! * `sync_once` ignores the transport — one-shot stays a REST drain.

use std::ops::ControlFlow;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt as _, StreamExt as _};
use httpmock::Method::{GET, POST};
use httpmock::MockServer;
use kupferdraht::keystore::{InMemoryKeyStore, SecureKeyStore};
use kupferdraht::storage::Store;
use kupferdraht::sync::TypingEvent;
use kupferdraht::{
    Client, ClientBuilder, EventHandler, Message, ServerConfig, SyncSettings, SyncSummary,
    SyncTransport,
};
use tempfile::TempDir;
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};

mod common;

const TEST_TIMEOUT: Duration = Duration::from_secs(10);

// ── fixtures ───────────────────────────────────────────────────────

/// Like `common::run_login`, but with a `websocket_url` pointing at
/// the in-process WebSocket server instead of the httpmock base.
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
    let store = client.store().expect("client has an SDK-opened store");
    (client, store)
}

/// Minimal Group-conversation JSON for the rooms-metadata mocks
/// (same shape as step_14b's fixture: `members.others` empty so no
/// `/list-users` backfill fires).
fn conv_json(id: &str) -> String {
    format!(
        r#"{{
            "qualified_id":{{"id":"{id}","domain":"example.test"}},
            "type":0,
            "protocol":"mls",
            "epoch":1,
            "group_id":"Z3JwLW9uZQ==",
            "name":"Step 62b push",
            "members":{{
              "self":{{"qualified_id":{{"id":"user-uuid","domain":"example.test"}}}},
              "others":[]
            }}
        }}"#
    )
}

/// Mount the conversations metadata the per-connection refresh needs.
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
            .body(format!(
                r#"{{"found":[{}], "not_found":[], "failed":[]}}"#,
                conv_json("room-a"),
            ));
    });
}

/// ADR-0019 (Step 73): `SyncTransport::WebSocket` negotiates the
/// concrete socket per connection — this file pins the
/// `/v14/events` dialect, so every test declares the feature
/// enabled (mounted BEFORE the login flow; httpmock: first
/// registered wins). The legacy dialect is pinned by
/// `step_73_legacy_await_push.rs`.
fn mount_consumable_enabled(server: &MockServer) {
    server.mock(|when, then| {
        when.method(GET).path("/v14/feature-configs");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"consumableNotifications":{"status":"enabled","lockStatus":"unlocked"}}"#);
    });
}

/// §11.2 `event` frame carrying one `conversation.typing` event —
/// transient, needs no MLS state, and observable via `on_typing`.
fn typing_event_frame(delivery_tag: u64, notif_id: &str) -> String {
    format!(
        r#"{{"type":"event","data":{{"delivery_tag":{delivery_tag},"event":{{"id":"{notif_id}","payload":[{{"type":"conversation.typing","qualified_conversation":{{"id":"room-a","domain":"example.test"}},"qualified_from":{{"id":"bob-uuid","domain":"example.test"}},"data":{{"status":"started"}}}}]}}}}}}"#
    )
}

/// §11.2 `synchronization` marker frame.
fn synchronization_frame(marker_id: &str) -> String {
    format!(r#"{{"type":"synchronization","data":{{"marker_id":"{marker_id}"}}}}"#)
}

/// Accept one WebSocket connection and hand back the server-side
/// stream together with the upgrade-request URI (path + query).
async fn accept_with_uri(listener: &TcpListener) -> (WebSocketStream<TcpStream>, String) {
    let (stream, _) = listener.accept().await.expect("accept");
    let (uri_tx, uri_rx) = tokio::sync::oneshot::channel::<String>();
    // The Err size is tungstenite's Callback trait contract — not
    // ours to shrink.
    #[allow(clippy::result_large_err)]
    let capture = move |req: &Request, resp: Response| {
        uri_tx.send(req.uri().to_string()).expect("send uri");
        Ok(resp)
    };
    let ws = tokio_tungstenite::accept_hdr_async(stream, capture)
        .await
        .expect("upgrade");
    let uri = uri_rx.await.expect("uri captured");
    (ws, uri)
}

/// Pull one query-parameter value out of a request URI.
fn query_param(uri: &str, key: &str) -> Option<String> {
    let (_, query) = uri.split_once('?')?;
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == key).then(|| v.to_string())
    })
}

/// Read frames until the peer goes away — keeps the server side open
/// while the client finishes its assertions.
async fn drain_until_closed(mut ws: WebSocketStream<TcpStream>) {
    while let Some(next) = ws.next().await {
        if next.is_err() {
            return;
        }
    }
}

/// `EventHandler` recording every `on_typing` callback.
#[derive(Default)]
struct RecordingHandler {
    typing: Mutex<Vec<TypingEvent>>,
}

impl EventHandler for RecordingHandler {
    fn on_message(&self, _message: Message) {}
    fn on_room_invited(&self, _room_id: String) {}
    fn on_sync_error(&self, description: String) {
        eprintln!("[step-62b sync error] {description}");
    }
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

// ── contract pins ──────────────────────────────────────────────────

// The polling default this file pinned is gone: push is the native default
// since Step 118 (ADR-0033) — pinned in `step_118_native_push.rs`.

#[tokio::test]
async fn websocket_sync_dispatches_live_event_frames_through_handlers() {
    let server = MockServer::start();
    mount_consumable_enabled(&server);
    common::mount_full_login_flow(&server);
    mount_rooms(&server);
    // Deliberately NO /v14/notifications mock: the push loop must
    // never touch the REST queue while the socket is healthy.

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let ws_base = format!("ws://{}", listener.local_addr().expect("addr"));
    let ws_server = tokio::spawn(async move {
        let (mut ws, uri) = accept_with_uri(&listener).await;
        assert!(uri.starts_with("/v14/events?"), "uri: {uri}");
        ws.send(WsMessage::text(typing_event_frame(1, "push-notif-1")))
            .await
            .expect("send event frame");
        drain_until_closed(ws).await;
    });

    let (client, _store) = run_login_with_ws(&server, &ws_base).await;
    let handler = Arc::new(RecordingHandler::default());
    client.add_event_handler(handler.clone());

    let ticks: Arc<Mutex<Vec<SyncSummary>>> = Arc::new(Mutex::new(Vec::new()));
    let ticks_in_cb = Arc::clone(&ticks);
    timeout(
        TEST_TIMEOUT,
        client.sync_with_callback(websocket_settings(), move |summary| {
            ticks_in_cb.lock().expect("ticks lock").push(summary);
            ControlFlow::Break(())
        }),
    )
    .await
    .expect("push loop must finish within the timeout")
    .expect("Break must exit the push loop with Ok");

    timeout(TEST_TIMEOUT, ws_server)
        .await
        .expect("ws server must finish")
        .expect("ws server task");

    let ticks = ticks.lock().expect("ticks lock");
    assert_eq!(ticks.len(), 1, "one event frame → one callback tick");
    assert_eq!(ticks[0].events_dispatched, 1);
    assert_eq!(
        ticks[0].rooms_updated, 0,
        "frame ticks carry event counts only — the metadata refresh is not a tick"
    );

    let typing = handler.typing.lock().expect("typing lock");
    assert_eq!(typing.len(), 1, "the typing event must reach on_typing");
    assert_eq!(typing[0].room_id, "room-a@example.test");
    assert_eq!(typing[0].typing_users, vec!["bob-uuid@example.test"]);
}

#[tokio::test]
async fn websocket_session_runs_the_key_package_check() {
    // Step 62↔63 integration: the polling loops hit the throttled
    // key-package check via sync_once, which push sessions bypass —
    // the push entry must run it itself, once per session.
    let server = MockServer::start();
    mount_consumable_enabled(&server);
    common::mount_full_login_flow(&server);
    mount_rooms(&server);
    let kp_count = server.mock(|when, then| {
        when.method(GET)
            .path_includes("/mls/key-packages/self/")
            .path_includes("/count")
            .query_param("ciphersuite", "2");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"count":100}"#);
    });

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let ws_base = format!("ws://{}", listener.local_addr().expect("addr"));
    let ws_server = tokio::spawn(async move {
        let (mut ws, _uri) = accept_with_uri(&listener).await;
        ws.send(WsMessage::text(typing_event_frame(1, "push-notif-1")))
            .await
            .expect("send event frame");
        drain_until_closed(ws).await;
    });

    let (client, _store) = run_login_with_ws(&server, &ws_base).await;
    timeout(
        TEST_TIMEOUT,
        client.sync_with_callback(websocket_settings(), move |_| ControlFlow::Break(())),
    )
    .await
    .expect("push loop must finish within the timeout")
    .expect("Break must exit with Ok");

    timeout(TEST_TIMEOUT, ws_server)
        .await
        .expect("ws server must finish")
        .expect("ws server task");
    assert_eq!(
        kp_count.calls(),
        1,
        "a push session must run the throttled key-package check exactly once"
    );
}

#[tokio::test]
async fn websocket_event_frame_is_acked_and_cursor_persisted() {
    let server = MockServer::start();
    mount_consumable_enabled(&server);
    common::mount_full_login_flow(&server);
    mount_rooms(&server);

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let ws_base = format!("ws://{}", listener.local_addr().expect("addr"));
    let ws_server = tokio::spawn(async move {
        let (mut ws, _uri) = accept_with_uri(&listener).await;
        ws.send(WsMessage::text(typing_event_frame(1, "push-notif-1")))
            .await
            .expect("send event frame");
        let ack = ws
            .next()
            .await
            .expect("client must send an ack")
            .expect("ack frame readable");
        let ack: serde_json::Value =
            serde_json::from_str(ack.to_text().expect("text frame")).expect("ack is JSON");
        assert_eq!(
            ack,
            serde_json::json!({
                "type": "ack",
                "data": { "delivery_tag": 1, "multiple": false }
            }),
            "§11.3 ack for the delivered frame"
        );
        drain_until_closed(ws).await;
    });

    let (client, store) = run_login_with_ws(&server, &ws_base).await;
    timeout(
        TEST_TIMEOUT,
        client.sync_with_callback(websocket_settings(), move |_| ControlFlow::Break(())),
    )
    .await
    .expect("push loop must finish within the timeout")
    .expect("Break must exit with Ok");

    // The ack the server asserted above may only ever follow the
    // durable cursor write — by the time the loop returned, both
    // must hold.
    let state = store
        .load_sync_state()
        .await
        .expect("sync_state readable")
        .expect("sync_state row exists");
    assert_eq!(
        state.last_event_id.as_deref(),
        Some("push-notif-1"),
        "cursor must be persisted before the ack releases the frame server-side"
    );

    timeout(TEST_TIMEOUT, ws_server)
        .await
        .expect("ws server must finish")
        .expect("ws server task asserts the ack");
}

#[tokio::test]
async fn websocket_synchronization_frame_persists_connection_marker() {
    let server = MockServer::start();
    mount_consumable_enabled(&server);
    common::mount_full_login_flow(&server);
    mount_rooms(&server);

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let ws_base = format!("ws://{}", listener.local_addr().expect("addr"));
    let ws_server = tokio::spawn(async move {
        let (mut ws, uri) = accept_with_uri(&listener).await;
        let marker = query_param(&uri, "sync_marker").expect("sync_marker query param");
        ws.send(WsMessage::text(synchronization_frame(&marker)))
            .await
            .expect("send synchronization frame");
        ws.send(WsMessage::text(typing_event_frame(1, "push-notif-1")))
            .await
            .expect("send event frame");
        drain_until_closed(ws).await;
        marker
    });

    let (client, store) = run_login_with_ws(&server, &ws_base).await;
    timeout(
        TEST_TIMEOUT,
        client.sync_with_callback(websocket_settings(), move |_| ControlFlow::Break(())),
    )
    .await
    .expect("push loop must finish within the timeout")
    .expect("Break must exit with Ok");

    let marker = timeout(TEST_TIMEOUT, ws_server)
        .await
        .expect("ws server must finish")
        .expect("ws server task");
    let state = store
        .load_sync_state()
        .await
        .expect("sync_state readable")
        .expect("sync_state row exists");
    assert_eq!(
        state.sync_marker_uuid.as_deref(),
        Some(marker.as_str()),
        "the synchronization echo must land in sync_state.sync_marker_uuid"
    );
}

#[tokio::test]
async fn stale_tagged_synchronization_marker_is_acked_but_not_persisted() {
    // Current Wire backends queue marker events durably: a reconnect
    // replays stale markers from earlier connections, tagged and as
    // binary frames (live-observed on the self-hosted instance,
    // 2026-08-12). The client must ack them — the broker's prefetch
    // window stalls otherwise — but only its own echo means "caught
    // up" and may be persisted.
    let server = MockServer::start();
    mount_consumable_enabled(&server);
    common::mount_full_login_flow(&server);
    mount_rooms(&server);

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let ws_base = format!("ws://{}", listener.local_addr().expect("addr"));
    let ws_server = tokio::spawn(async move {
        let (mut ws, _uri) = accept_with_uri(&listener).await;
        let stale = r#"{"data":{"delivery_tag":1,"marker_id":"00000000-0000-0000-0000-00000000dead"},"type":"synchronization"}"#;
        ws.send(WsMessage::binary(stale.as_bytes()))
            .await
            .expect("send stale tagged marker");
        let ack = ws
            .next()
            .await
            .expect("client must ack the stale marker")
            .expect("ack frame readable");
        let ack: serde_json::Value =
            serde_json::from_str(ack.to_text().expect("text frame")).expect("ack is JSON");
        assert_eq!(
            ack,
            serde_json::json!({
                "type": "ack",
                "data": { "delivery_tag": 1, "multiple": false }
            }),
            "the stale marker must be acked or the stream stalls"
        );
        ws.send(WsMessage::text(typing_event_frame(2, "push-notif-1")))
            .await
            .expect("send event frame");
        drain_until_closed(ws).await;
    });

    let (client, store) = run_login_with_ws(&server, &ws_base).await;
    timeout(
        TEST_TIMEOUT,
        client.sync_with_callback(websocket_settings(), move |_| ControlFlow::Break(())),
    )
    .await
    .expect("push loop must finish within the timeout")
    .expect("Break must exit with Ok");

    timeout(TEST_TIMEOUT, ws_server)
        .await
        .expect("ws server must finish")
        .expect("ws server task asserts the marker ack");

    let state = store
        .load_sync_state()
        .await
        .expect("sync_state readable")
        .expect("sync_state row exists (cursor was persisted)");
    assert_eq!(
        state.sync_marker_uuid, None,
        "a foreign marker must never be persisted as our catch-up point"
    );
}

#[tokio::test]
async fn websocket_redelivered_frame_is_acked_but_not_redispatched() {
    let server = MockServer::start();
    mount_consumable_enabled(&server);
    common::mount_full_login_flow(&server);
    mount_rooms(&server);

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let ws_base = format!("ws://{}", listener.local_addr().expect("addr"));
    let ws_server = tokio::spawn(async move {
        let (mut ws, _uri) = accept_with_uri(&listener).await;
        // The §11.3 redelivery shape: same notification id, fresh
        // connection-scoped delivery_tag.
        ws.send(WsMessage::text(typing_event_frame(1, "push-notif-1")))
            .await
            .expect("send frame 1");
        ws.send(WsMessage::text(typing_event_frame(2, "push-notif-1")))
            .await
            .expect("send redelivered frame");
        ws.send(WsMessage::text(typing_event_frame(3, "push-notif-2")))
            .await
            .expect("send frame 3");
        // All three must be acked — the redelivered one included,
        // otherwise it comes back on every reconnect forever.
        for expected_tag in 1..=3_u64 {
            let ack = ws
                .next()
                .await
                .expect("client must ack every frame")
                .expect("ack frame readable");
            let ack: serde_json::Value =
                serde_json::from_str(ack.to_text().expect("text frame")).expect("ack is JSON");
            assert_eq!(ack["data"]["delivery_tag"], expected_tag, "acks in order");
        }
        drain_until_closed(ws).await;
    });

    let (client, _store) = run_login_with_ws(&server, &ws_base).await;
    let handler = Arc::new(RecordingHandler::default());
    client.add_event_handler(handler.clone());

    let ticks: Arc<Mutex<Vec<SyncSummary>>> = Arc::new(Mutex::new(Vec::new()));
    let ticks_in_cb = Arc::clone(&ticks);
    timeout(
        TEST_TIMEOUT,
        client.sync_with_callback(websocket_settings(), move |summary| {
            let mut ticks = ticks_in_cb.lock().expect("ticks lock");
            ticks.push(summary);
            if ticks.len() >= 3 {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        }),
    )
    .await
    .expect("push loop must finish within the timeout")
    .expect("Break must exit with Ok");

    let dispatched: Vec<u32> = ticks
        .lock()
        .expect("ticks lock")
        .iter()
        .map(|s| s.events_dispatched)
        .collect();
    assert_eq!(
        dispatched,
        vec![1, 0, 1],
        "the redelivered frame ticks with zero dispatched events"
    );
    assert_eq!(
        handler.typing.lock().expect("typing lock").len(),
        2,
        "handlers see the deduped notification exactly once"
    );

    timeout(TEST_TIMEOUT, ws_server)
        .await
        .expect("ws server must finish")
        .expect("ws server task asserts the three acks");
}

#[tokio::test]
async fn websocket_reconnects_with_fresh_marker_after_server_close() {
    let server = MockServer::start();
    mount_consumable_enabled(&server);
    common::mount_full_login_flow(&server);
    mount_rooms(&server);

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let ws_base = format!("ws://{}", listener.local_addr().expect("addr"));
    let ws_server = tokio::spawn(async move {
        // Connection 1: orderly close right after the upgrade — the
        // §11.2 "second socket kicks the first" case from the client's
        // point of view.
        let (mut ws1, uri1) = accept_with_uri(&listener).await;
        ws1.close(None).await.expect("close connection 1");
        drop(ws1);
        // Connection 2: deliver one frame so the client can Break.
        let (mut ws2, uri2) = accept_with_uri(&listener).await;
        ws2.send(WsMessage::text(typing_event_frame(1, "push-notif-1")))
            .await
            .expect("send event frame");
        drain_until_closed(ws2).await;
        (
            query_param(&uri1, "sync_marker").expect("marker on connection 1"),
            query_param(&uri2, "sync_marker").expect("marker on connection 2"),
        )
    });

    let (client, _store) = run_login_with_ws(&server, &ws_base).await;
    let result = timeout(
        TEST_TIMEOUT,
        client.sync_with_callback(websocket_settings(), move |_| ControlFlow::Break(())),
    )
    .await
    .expect("push loop must finish within the timeout");
    assert!(
        result.is_ok(),
        "a dropped connection reconnects instead of surfacing a hard error: {result:?}"
    );

    let (marker1, marker2) = timeout(TEST_TIMEOUT, ws_server)
        .await
        .expect("ws server must finish")
        .expect("ws server task");
    assert!(!marker1.is_empty() && !marker2.is_empty());
    assert_ne!(
        marker1, marker2,
        "every connection must carry a fresh sync_marker (§11.5)"
    );
}

#[tokio::test]
async fn sync_once_ignores_websocket_transport_and_drains_rest() {
    let server = MockServer::start();
    mount_consumable_enabled(&server);
    common::mount_full_login_flow(&server);
    mount_rooms(&server);
    server.mock(|when, then| {
        when.method(GET).path("/v14/notifications");
        then.status(200)
            .header("content-type", "application/json")
            .body(
                r#"{"notifications":[{"id":"notif-1","payload":[{"type":"unknown.for-test"}]}],
                     "has_more":false}"#,
            );
    });

    // websocket_url points at the httpmock base — if sync_once tried
    // to open the socket, the http:// scheme alone would error.
    let (client, _store) = common::run_login(&server).await;
    let summary = client
        .sync_once(SyncSettings {
            poll_interval: None,
            transport: SyncTransport::WebSocket,
        })
        .await
        .expect("sync_once stays a one-shot REST drain regardless of transport");
    assert_eq!(summary.events_dispatched, 1);
}
