// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 97b — the legacy `/await` socket is a doorbell (ADR-0030,
//! review blocker B6). TDD red.
//!
//! Before: drain `GET /notifications` first, connect afterwards, and let
//! every durable frame dispatch itself and advance the cursor. An event
//! stored between the drain and the socket registration was never
//! pushed, and the next frame moved the cursor past it — lost for good.
//! Now: connect first, then drain; a durable frame only triggers a drain
//! (frames during a running drain coalesce); only the drain advances the
//! cursor; transient frames dispatch directly.
//!
//! Harness: the Step-73 one — a loopback WebSocket server plays cannon,
//! httpmock plays the REST queue; the REST answers depend on whether the
//! socket is already registered and on the `since` cursor.

use std::ops::ControlFlow;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use httpmock::Method::{GET, POST};
use httpmock::MockServer;
use httpmock::prelude::HttpMockRequest;
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
                     "name":"Step 97b",
                     "members":{"self":{"qualified_id":{"id":"user-uuid","domain":"example.test"}},"others":[]}
                   }], "not_found":[], "failed":[]}"#,
            );
    });
}

/// A notification (REST item and legacy frame alike, observations §68)
/// carrying one typing event from `from_uuid` — observable via
/// `on_typing` without MLS state.
fn typing_notification(notif_id: &str, from_uuid: &str, transient: bool) -> String {
    format!(
        r#"{{"id":"{notif_id}","payload":[{{"type":"conversation.typing","qualified_conversation":{{"id":"room-a","domain":"example.test"}},"qualified_from":{{"id":"{from_uuid}","domain":"example.test"}},"data":{{"status":"started"}}}}],"transient":{transient}}}"#
    )
}

fn page(items: &[String]) -> String {
    format!(
        r#"{{"notifications":[{}],"has_more":false}}"#,
        items.join(",")
    )
}

/// The `since` cursor of a `GET /notifications` request.
fn since(req: &HttpMockRequest) -> Option<String> {
    req.query_params_map().get("since").cloned()
}

/// Accept one WebSocket connection; `registered` flips inside the
/// handshake — before the client's connect returns — the moment the
/// socket counts as registered with the push service.
async fn accept_registering(
    listener: &TcpListener,
    registered: Arc<AtomicBool>,
) -> WebSocketStream<TcpStream> {
    let (stream, _) = listener.accept().await.expect("accept");
    #[allow(clippy::result_large_err)]
    let callback = move |_req: &Request, resp: Response| {
        registered.store(true, Ordering::SeqCst);
        Ok(resp)
    };
    tokio_tungstenite::accept_hdr_async(stream, callback)
        .await
        .expect("upgrade")
}

async fn drain_until_closed(mut ws: WebSocketStream<TcpStream>) {
    while let Some(msg) = ws.next().await {
        if msg.is_err() {
            break;
        }
    }
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

impl RecordingHandler {
    /// Every user id any recorded typing event named.
    fn typing_users(&self) -> Vec<String> {
        self.typing
            .lock()
            .expect("typing lock")
            .iter()
            .flat_map(|e| e.typing_users.clone())
            .collect()
    }
}

fn websocket_settings() -> SyncSettings {
    SyncSettings {
        poll_interval: Some(Duration::from_millis(1)),
        transport: SyncTransport::WebSocket,
    }
}

/// Run the push loop until `handler` has seen typing from `user` (or the
/// timeout trips — a red test fails there).
async fn run_until_typing_from(client: &Arc<Client>, handler: &Arc<RecordingHandler>, user: &str) {
    let h = Arc::clone(handler);
    let user = format!("{user}@example.test");
    timeout(
        TEST_TIMEOUT,
        client.sync_with_callback(websocket_settings(), move |_summary| {
            if h.typing_users().contains(&user) {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        }),
    )
    .await
    .expect("the awaited typing event must arrive within the timeout")
    .expect("Break must exit the push loop with Ok");
}

/// Run the push loop for `window`, whatever it dispatches.
async fn run_for(client: &Arc<Client>, window: Duration) {
    let _ = timeout(
        window,
        client.sync_with_callback(websocket_settings(), |_summary| ControlFlow::Continue(())),
    )
    .await;
}

// ---- tests ----

/// The B6 loss: `gap-n2` is stored after the old pre-connect drain and
/// before the socket is registered; the push service only pushes the
/// later `live-n3`. Before 97b the frame dispatched itself, moved the
/// cursor to `live-n3`, and `gap-n2` was never delivered. With connect
/// first and the frame as a doorbell, the drain returns both.
#[tokio::test]
async fn an_event_stored_while_the_socket_connects_is_delivered() {
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    mount_rooms(&server);
    let registered = Arc::new(AtomicBool::new(false));
    let (r1, r2) = (registered.clone(), registered.clone());
    server.mock(move |when, then| {
        when.method(GET)
            .path("/v14/notifications")
            .is_true(move |req| {
                r1.load(Ordering::SeqCst)
                    && !matches!(since(req).as_deref(), Some("gap-n2" | "live-n3"))
            });
        then.status(200)
            .header("content-type", "application/json")
            .body(page(&[
                typing_notification("gap-n2", "gap-uuid", false),
                typing_notification("live-n3", "live-uuid", false),
            ]));
    });
    server.mock(move |when, then| {
        when.method(GET)
            .path("/v14/notifications")
            .is_true(move |req| {
                r2.load(Ordering::SeqCst) && since(req).as_deref() == Some("gap-n2")
            });
        then.status(200)
            .header("content-type", "application/json")
            .body(page(&[typing_notification("live-n3", "live-uuid", false)]));
    });
    common::mount_empty_notifications(&server);

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let ws_base = format!("ws://{}", listener.local_addr().expect("addr"));
    let reg = registered.clone();
    let ws_server = tokio::spawn(async move {
        let mut ws = accept_registering(&listener, reg).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        ws.send(WsMessage::binary(
            typing_notification("live-n3", "live-uuid", false).into_bytes(),
        ))
        .await
        .expect("send the live frame");
        drain_until_closed(ws).await;
    });

    let (client, _store) = run_login_with_ws(&server, &ws_base).await;
    let handler = Arc::new(RecordingHandler::default());
    client.add_event_handler(handler.clone());
    run_until_typing_from(&client, &handler, "gap-uuid").await;
    ws_server.abort();

    assert!(
        handler
            .typing_users()
            .contains(&"gap-uuid@example.test".to_string()),
        "the event stored while the socket connected is delivered"
    );
}

/// ADR-0030 d.2: the frame only rings the doorbell — what is dispatched
/// comes from the drain. The frame `n5` carries a payload the REST item
/// `n5` does not; only the REST one may surface.
#[tokio::test]
async fn the_drain_decides_what_is_dispatched_not_the_frame() {
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    mount_rooms(&server);
    let frame_sent = Arc::new(AtomicBool::new(false));
    let sent = frame_sent.clone();
    server.mock(move |when, then| {
        when.method(GET)
            .path("/v14/notifications")
            .is_true(move |req| sent.load(Ordering::SeqCst) && since(req).as_deref() != Some("n5"));
        then.status(200)
            .header("content-type", "application/json")
            .body(page(&[typing_notification("n5", "rest-uuid", false)]));
    });
    common::mount_empty_notifications(&server);

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let ws_base = format!("ws://{}", listener.local_addr().expect("addr"));
    let flag = frame_sent.clone();
    let ws_server = tokio::spawn(async move {
        let mut ws = accept_registering(&listener, Arc::new(AtomicBool::new(false))).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        flag.store(true, Ordering::SeqCst);
        ws.send(WsMessage::binary(
            typing_notification("n5", "frame-uuid", false).into_bytes(),
        ))
        .await
        .expect("send the frame");
        drain_until_closed(ws).await;
    });

    let (client, _store) = run_login_with_ws(&server, &ws_base).await;
    let handler = Arc::new(RecordingHandler::default());
    client.add_event_handler(handler.clone());
    run_until_typing_from(&client, &handler, "rest-uuid").await;
    ws_server.abort();

    assert!(
        !handler
            .typing_users()
            .contains(&"frame-uuid@example.test".to_string()),
        "a durable frame's own payload is never dispatched"
    );
}

/// ADR-0030 d.3: only the drain advances the cursor. A durable frame the
/// REST queue never returns (`ghost-n9`) must not become the cursor.
#[tokio::test]
async fn only_the_drain_advances_the_cursor() {
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    mount_rooms(&server);
    let registered = Arc::new(AtomicBool::new(false));
    let reg = registered.clone();
    server.mock(move |when, then| {
        when.method(GET)
            .path("/v14/notifications")
            .is_true(move |req| reg.load(Ordering::SeqCst) && since(req).as_deref() != Some("n1"));
        then.status(200)
            .header("content-type", "application/json")
            .body(page(&[typing_notification("n1", "drained-uuid", false)]));
    });
    common::mount_empty_notifications(&server);

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let ws_base = format!("ws://{}", listener.local_addr().expect("addr"));
    let reg = registered.clone();
    let ws_server = tokio::spawn(async move {
        let mut ws = accept_registering(&listener, reg).await;
        ws.send(WsMessage::binary(
            typing_notification("ghost-n9", "ghost-uuid", false).into_bytes(),
        ))
        .await
        .expect("send the ghost frame");
        drain_until_closed(ws).await;
    });

    let (client, store) = run_login_with_ws(&server, &ws_base).await;
    run_for(&client, Duration::from_millis(1500)).await;
    ws_server.abort();

    let cursor = store
        .load_sync_state()
        .await
        .expect("load sync state")
        .and_then(|s| s.last_event_id);
    assert_eq!(
        cursor.as_deref(),
        Some("n1"),
        "the cursor is the last drained id, never a frame id"
    );
}

/// ADR-0030 d.2: frames arriving while a drain runs coalesce into one
/// follow-up drain — one REST round trip per burst, not per frame, and
/// at least one after the connect drain.
#[tokio::test]
async fn frames_during_a_running_drain_coalesce_into_one_follow_up_drain() {
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    mount_rooms(&server);
    let registered = Arc::new(AtomicBool::new(false));
    let drains_after_registration = Arc::new(AtomicUsize::new(0));
    let (reg, count) = (registered.clone(), drains_after_registration.clone());
    server.mock(move |when, then| {
        when.method(GET)
            .path("/v14/notifications")
            .is_true(move |_| {
                let after = reg.load(Ordering::SeqCst);
                if after {
                    count.fetch_add(1, Ordering::SeqCst);
                }
                after
            });
        then.status(200)
            .delay(Duration::from_millis(400))
            .header("content-type", "application/json")
            .body(page(&[]));
    });
    common::mount_empty_notifications(&server);

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let ws_base = format!("ws://{}", listener.local_addr().expect("addr"));
    let reg = registered.clone();
    let ws_server = tokio::spawn(async move {
        let mut ws = accept_registering(&listener, reg).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        for n in 1..=5 {
            ws.send(WsMessage::binary(
                typing_notification(&format!("burst-{n}"), "burst-uuid", false).into_bytes(),
            ))
            .await
            .expect("send a burst frame");
        }
        drain_until_closed(ws).await;
    });

    let (client, _store) = run_login_with_ws(&server, &ws_base).await;
    run_for(&client, Duration::from_millis(2500)).await;
    ws_server.abort();

    let drains = drains_after_registration.load(Ordering::SeqCst);
    assert!(
        (2..=3).contains(&drains),
        "the connect drain plus one (at most two) follow-up drains for a burst of five frames, got {drains}"
    );
}

/// ADR-0030 d.4 (pin — already true): a transient frame (typing) is
/// dispatched directly and triggers no drain.
#[tokio::test]
async fn a_transient_frame_is_dispatched_without_a_drain() {
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    mount_rooms(&server);
    let registered = Arc::new(AtomicBool::new(false));
    let drains_after_registration = Arc::new(AtomicUsize::new(0));
    let (reg, count) = (registered.clone(), drains_after_registration.clone());
    server.mock(move |when, then| {
        when.method(GET)
            .path("/v14/notifications")
            .is_true(move |_| {
                let after = reg.load(Ordering::SeqCst);
                if after {
                    count.fetch_add(1, Ordering::SeqCst);
                }
                after
            });
        then.status(200)
            .header("content-type", "application/json")
            .body(page(&[]));
    });
    common::mount_empty_notifications(&server);

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let ws_base = format!("ws://{}", listener.local_addr().expect("addr"));
    let reg = registered.clone();
    let ws_server = tokio::spawn(async move {
        let mut ws = accept_registering(&listener, reg).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        ws.send(WsMessage::binary(
            typing_notification("typing-t1", "typist-uuid", true).into_bytes(),
        ))
        .await
        .expect("send the transient frame");
        drain_until_closed(ws).await;
    });

    let (client, _store) = run_login_with_ws(&server, &ws_base).await;
    let handler = Arc::new(RecordingHandler::default());
    client.add_event_handler(handler.clone());
    run_until_typing_from(&client, &handler, "typist-uuid").await;
    ws_server.abort();

    assert!(
        drains_after_registration.load(Ordering::SeqCst) <= 1,
        "a transient frame triggers no drain (at most the connect drain ran)"
    );
}

/// Pin (open.md gap-B.16 residual): a transport-class drain failure
/// (`503`) is reported through `on_sync_error` and reconnects the
/// socket — the next connect drains again; it never aborts the loop.
#[tokio::test]
async fn a_failing_drain_reconnects_the_socket() {
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    mount_rooms(&server);
    let registered = Arc::new(AtomicBool::new(false));
    let reg = registered.clone();
    server.mock(move |when, then| {
        when.method(GET)
            .path("/v14/notifications")
            .is_true(move |_| reg.load(Ordering::SeqCst));
        then.status(503).body("unavailable");
    });
    common::mount_empty_notifications(&server);

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let ws_base = format!("ws://{}", listener.local_addr().expect("addr"));
    let connects = Arc::new(AtomicUsize::new(0));
    let (reg, count) = (registered.clone(), connects.clone());
    let ws_server = tokio::spawn(async move {
        loop {
            let ws = accept_registering(&listener, reg.clone()).await;
            count.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(drain_until_closed(ws));
        }
    });

    let (client, _store) = run_login_with_ws(&server, &ws_base).await;
    let errors = Arc::new(ErrorRecorder::default());
    client.add_event_handler(errors.clone());
    run_for(&client, Duration::from_millis(1500)).await;
    ws_server.abort();

    assert!(
        connects.load(Ordering::SeqCst) >= 2,
        "a failed drain reconnects, got {} connects",
        connects.load(Ordering::SeqCst)
    );
    assert!(
        errors.contains("push drain (legacy)"),
        "the failed drain reaches on_sync_error"
    );
}

#[derive(Default)]
struct ErrorRecorder {
    errors: Mutex<Vec<String>>,
}

impl EventHandler for ErrorRecorder {
    fn on_message(&self, _message: Message) {}
    fn on_room_invited(&self, _room_id: String) {}
    fn on_sync_error(&self, description: String) {
        self.errors.lock().expect("errors lock").push(description);
    }
}

impl ErrorRecorder {
    fn contains(&self, needle: &str) -> bool {
        self.errors
            .lock()
            .expect("errors lock")
            .iter()
            .any(|e| e.contains(needle))
    }
}
