// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 118 — the room list is fetched when it is stale, not every pass.
//! TDD red.
//!
//! Before: every `sync_once` fetched the whole room list (`list-ids`,
//! `list`, `list-users`) before its drain, and the native push loop once
//! per connection — four requests for a pass that found nothing, and a
//! room renamed during a long push connection stayed unseen until the
//! next reconnect. Now the drain marks the room list stale when it hands
//! over an event that may change it, or when this client cannot have
//! seen every event; the list is refreshed then, in the same pass.
//!
//! Observable without MLS state: `conversation/list` answers the room's
//! old name until the queue has served the trigger, its new name after.
//! A refresh before the drain (the old order) still reads the old name.
//! Harness for push: the Step-97b loopback WebSocket server.

use std::ops::ControlFlow;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use httpmock::Method::{GET, POST};
use httpmock::prelude::HttpMockRequest;
use httpmock::{Mock, MockServer};
use kupferdraht::keystore::{InMemoryKeyStore, SecureKeyStore};
use kupferdraht::{Client, ClientBuilder, Kupferdraht, ServerConfig, SyncSettings, SyncTransport};
use tempfile::TempDir;
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message as WsMessage;

mod common;

const ROOM: &str = "room-a";
const DOMAIN: &str = "example.test";
const OLD_NAME: &str = "Before";
const NEW_NAME: &str = "After";

/// The account's home and keystore, kept so a second client can resume
/// on the same databases (the Step-97d precedent).
struct Account {
    home: TempDir,
    keystore: Arc<dyn SecureKeyStore>,
}

async fn logged_in_client(server: &MockServer, ws_base: &str) -> (Arc<Client>, Account) {
    let home = TempDir::new().expect("TempDir");
    let keystore: Arc<dyn SecureKeyStore> = Arc::new(InMemoryKeyStore::new());
    let app = Kupferdraht::open(home.path().to_path_buf(), keystore.clone())
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
    (client, Account { home, keystore })
}

/// A second, independent `Client` on the same account — what an app's
/// notification extension looks like from the databases' side.
async fn second_client(first: &Arc<Client>, account: &Account) -> Arc<Client> {
    let user = first.user_id().expect("user id").as_str().to_string();
    ClientBuilder::new()
        .permit_http()
        .base_dir(account.home.path().to_path_buf())
        .keystore(account.keystore.clone())
        .account(user.as_str())
        .resume()
        .await
        .expect("resume a second client on the account")
}

fn conversation_list_body(name: &str) -> String {
    format!(
        r#"{{"found":[{{
             "qualified_id":{{"id":"{ROOM}","domain":"{DOMAIN}"}},
             "type":0,"protocol":"mls","epoch":1,"group_id":"Z3JwLTExOA==",
             "name":"{name}",
             "members":{{"self":{{"qualified_id":{{"id":"user-uuid","domain":"{DOMAIN}"}}}},"others":[]}}
           }}], "not_found":[], "failed":[]}}"#
    )
}

/// The room list: `list-ids` (returned, to count fetches) and `list`,
/// which answers the new name once `renamed` is set.
fn mount_room_list(server: &MockServer, renamed: Arc<AtomicBool>) -> Mock<'_> {
    server.mock(move |when, then| {
        when.method(POST)
            .path("/v14/conversations/list")
            .is_true(move |_| renamed.load(Ordering::SeqCst));
        then.status(200)
            .header("content-type", "application/json")
            .body(conversation_list_body(NEW_NAME));
    });
    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list");
        then.status(200)
            .header("content-type", "application/json")
            .body(conversation_list_body(OLD_NAME));
    });
    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list-ids");
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(
                r#"{{"qualified_conversations":[{{"id":"{ROOM}","domain":"{DOMAIN}"}}],
                     "has_more":false,"paging_state":"END"}}"#
            ));
    })
}

fn rename_event() -> String {
    format!(
        r#"{{"type":"conversation.rename","qualified_conversation":{{"id":"{ROOM}","domain":"{DOMAIN}"}},"qualified_from":{{"id":"bob-uuid","domain":"{DOMAIN}"}},"data":{{"name":"{NEW_NAME}"}}}}"#
    )
}

/// A Welcome the client cannot absorb: the refresh must not depend on it.
fn garbage_welcome_event() -> String {
    format!(
        r#"{{"type":"conversation.mls-welcome","qualified_conversation":{{"id":"room-b","domain":"{DOMAIN}"}},"qualified_from":{{"id":"bob-uuid","domain":"{DOMAIN}"}},"data":"AAAA"}}"#
    )
}

/// A typing event — dispatched by the drain, changes no room.
fn typing_event() -> String {
    format!(
        r#"{{"type":"conversation.typing","qualified_conversation":{{"id":"{ROOM}","domain":"{DOMAIN}"}},"qualified_from":{{"id":"bob-uuid","domain":"{DOMAIN}"}},"data":{{"status":"started"}}}}"#
    )
}

fn notification(id: &str, event: &str) -> String {
    format!(r#"{{"id":"{id}","payload":[{event}],"transient":false}}"#)
}

fn page(items: &[String]) -> String {
    format!(
        r#"{{"notifications":[{}],"has_more":false}}"#,
        items.join(",")
    )
}

fn since(req: &HttpMockRequest) -> Option<String> {
    req.query_params_map().get("since").cloned()
}

fn is_notifications(req: &HttpMockRequest) -> bool {
    req.method_str() == "GET" && req.uri().path() == "/v14/notifications"
}

/// Serve `notif` (id `id`) once `queued` is set, until the cursor is past
/// it; serving it sets `renamed`, so the room list answers the new name
/// from then on — only a refresh after the drain can read it.
fn mount_trigger(
    server: &MockServer,
    queued: Arc<AtomicBool>,
    renamed: Arc<AtomicBool>,
    id: &'static str,
    notif: String,
) {
    server.mock(move |when, then| {
        when.method(GET)
            .path("/v14/notifications")
            .is_true(move |req| {
                let serve = is_notifications(req)
                    && queued.load(Ordering::SeqCst)
                    && since(req).as_deref() != Some(id);
                if serve {
                    renamed.store(true, Ordering::SeqCst);
                }
                serve
            });
        then.status(200)
            .header("content-type", "application/json")
            .body(page(&[notif]));
    });
}

async fn room_name(client: &Client) -> String {
    client
        .rooms()
        .await
        .iter()
        .find(|room| room.id().starts_with(ROOM))
        .map(|room| room.name())
        .expect("the room is in the room list")
}

async fn pass(client: &Client) {
    client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync pass");
}

/// Login and device registration — the polling tests' start.
async fn polling_setup(server: &MockServer) -> (Arc<Client>, Account) {
    common::mount_full_login_flow(server);
    logged_in_client(server, &server.base_url()).await
}

// ---- polling: sync_once ----

#[tokio::test]
async fn an_idle_pass_fetches_no_room_list() {
    let server = MockServer::start();
    let renamed = Arc::new(AtomicBool::new(false));
    let list_ids = mount_room_list(&server, renamed.clone());
    common::mount_empty_notifications(&server);
    let (client, _account) = polling_setup(&server).await;
    let before = list_ids.calls();

    pass(&client).await;
    pass(&client).await;
    pass(&client).await;

    assert_eq!(
        list_ids.calls() - before,
        1,
        "the first pass fetches the room list, idle passes do not"
    );
}

#[tokio::test]
async fn a_room_event_refreshes_the_room_list_in_the_same_pass() {
    let server = MockServer::start();
    let renamed = Arc::new(AtomicBool::new(false));
    let queued = Arc::new(AtomicBool::new(false));
    mount_trigger(
        &server,
        queued.clone(),
        renamed.clone(),
        "n1",
        notification("n1", &rename_event()),
    );
    let list_ids = mount_room_list(&server, renamed.clone());
    common::mount_empty_notifications(&server);
    let (client, _account) = polling_setup(&server).await;
    pass(&client).await;
    assert_eq!(room_name(&client).await, OLD_NAME);
    let before = list_ids.calls();

    queued.store(true, Ordering::SeqCst);
    pass(&client).await;

    assert_eq!(
        room_name(&client).await,
        NEW_NAME,
        "refreshed after the drain"
    );
    assert_eq!(list_ids.calls() - before, 1, "one refresh for the event");
    pass(&client).await;
    assert_eq!(
        list_ids.calls() - before,
        1,
        "and none for the idle pass after it"
    );
}

#[tokio::test]
async fn a_welcome_refreshes_the_room_list() {
    let server = MockServer::start();
    let renamed = Arc::new(AtomicBool::new(false));
    let queued = Arc::new(AtomicBool::new(false));
    mount_trigger(
        &server,
        queued.clone(),
        renamed.clone(),
        "n1",
        notification("n1", &garbage_welcome_event()),
    );
    mount_room_list(&server, renamed.clone());
    common::mount_empty_notifications(&server);
    let (client, _account) = polling_setup(&server).await;
    pass(&client).await;

    queued.store(true, Ordering::SeqCst);
    pass(&client).await;

    assert_eq!(room_name(&client).await, NEW_NAME);
}

#[tokio::test]
async fn a_lost_cursor_refreshes_the_room_list() {
    let server = MockServer::start();
    let renamed = Arc::new(AtomicBool::new(false));
    // Pass 1 leaves the cursor at `n0`; pass 2 finds the backend lost it.
    let lost = Arc::new(AtomicBool::new(false));
    let lost_seen = lost.clone();
    server.mock(move |when, then| {
        when.method(GET)
            .path("/v14/notifications")
            .is_true(move |req| since(req).is_none() && !lost_seen.load(Ordering::SeqCst));
        then.status(200)
            .header("content-type", "application/json")
            .body(page(&[notification("n0", &typing_event())]));
    });
    let (lost_set, renamed_set) = (lost.clone(), renamed.clone());
    server.mock(move |when, then| {
        when.method(GET)
            .path("/v14/notifications")
            .is_true(move |req| {
                let serve = is_notifications(req) && since(req).as_deref() == Some("n0");
                if serve {
                    lost_set.store(true, Ordering::SeqCst);
                    renamed_set.store(true, Ordering::SeqCst);
                }
                serve
            });
        then.status(404)
            .header("content-type", "application/json")
            .body(r#"{"code":404,"label":"not-found","message":"Some notifications not found"}"#);
    });
    mount_room_list(&server, renamed.clone());
    common::mount_empty_notifications(&server);
    let (client, _account) = polling_setup(&server).await;
    pass(&client).await;
    assert_eq!(room_name(&client).await, OLD_NAME);

    pass(&client).await;

    assert!(lost.load(Ordering::SeqCst), "the pass met the lost cursor");
    assert_eq!(room_name(&client).await, NEW_NAME);
}

#[tokio::test]
async fn a_drain_by_another_client_refreshes_the_room_list() {
    let server = MockServer::start();
    let renamed = Arc::new(AtomicBool::new(false));
    let queued = Arc::new(AtomicBool::new(false));
    mount_trigger(
        &server,
        queued.clone(),
        renamed.clone(),
        "n1",
        notification("n1", &rename_event()),
    );
    let list_ids = mount_room_list(&server, renamed.clone());
    common::mount_empty_notifications(&server);
    let (app, account) = polling_setup(&server).await;
    pass(&app).await;
    let extension = second_client(&app, &account).await;

    queued.store(true, Ordering::SeqCst);
    pass(&extension).await;
    let before = list_ids.calls();
    pass(&app).await;

    assert_eq!(
        room_name(&app).await,
        NEW_NAME,
        "the app saw the extension's drain"
    );
    assert_eq!(list_ids.calls() - before, 1);
}

#[tokio::test]
async fn a_failed_refresh_is_retried_by_the_next_pass() {
    let server = MockServer::start();
    let renamed = Arc::new(AtomicBool::new(false));
    let broken = Arc::new(AtomicBool::new(true));
    let is_broken = broken.clone();
    server.mock(move |when, then| {
        when.method(POST)
            .path("/v14/conversations/list-ids")
            .is_true(move |_| is_broken.load(Ordering::SeqCst));
        then.status(500).body("{}");
    });
    let list_ids = mount_room_list(&server, renamed.clone());
    common::mount_empty_notifications(&server);
    let (client, _account) = polling_setup(&server).await;

    assert!(client.sync_once(SyncSettings::default()).await.is_err());
    broken.store(false, Ordering::SeqCst);
    pass(&client).await;

    assert_eq!(list_ids.calls(), 1, "the failed refresh is retried");
    assert_eq!(room_name(&client).await, OLD_NAME);
}

// ---- push: the legacy doorbell loop ----

fn push_settings() -> SyncSettings {
    SyncSettings {
        poll_interval: Some(Duration::from_millis(1)),
        transport: SyncTransport::WebSocket,
    }
}

/// Run the push loop for `window`, whatever it dispatches.
async fn push_for(client: &Arc<Client>, window: Duration) {
    let _ = timeout(
        window,
        client.sync_with_callback(push_settings(), |_summary| ControlFlow::Continue(())),
    )
    .await;
}

async fn read_until_closed(mut ws: WebSocketStream<TcpStream>) {
    while let Some(msg) = ws.next().await {
        if msg.is_err() {
            break;
        }
    }
}

#[tokio::test]
async fn a_reconnect_without_room_events_fetches_the_room_list_once() {
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    let renamed = Arc::new(AtomicBool::new(false));
    let list_ids = mount_room_list(&server, renamed);
    common::mount_empty_notifications(&server);

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let ws_base = format!("ws://{}", listener.local_addr().expect("addr"));
    let accepted = Arc::new(AtomicUsize::new(0));
    let count = accepted.clone();
    let ws_server = tokio::spawn(async move {
        // The first connection closes at once, the second stays.
        let (stream, _) = listener.accept().await.expect("accept");
        let mut first = tokio_tungstenite::accept_async(stream)
            .await
            .expect("upgrade");
        count.fetch_add(1, Ordering::SeqCst);
        let _ = first.close(None).await;
        let (stream, _) = listener.accept().await.expect("accept");
        let second = tokio_tungstenite::accept_async(stream)
            .await
            .expect("upgrade");
        count.fetch_add(1, Ordering::SeqCst);
        read_until_closed(second).await;
    });

    let (client, _account) = logged_in_client(&server, &ws_base).await;
    let before = list_ids.calls();
    push_for(&client, Duration::from_millis(1500)).await;
    ws_server.abort();

    assert_eq!(accepted.load(Ordering::SeqCst), 2, "the loop reconnected");
    assert_eq!(
        list_ids.calls() - before,
        1,
        "a reconnect without room events fetches no room list"
    );
}

#[tokio::test]
async fn a_room_event_on_a_live_connection_refreshes_the_room_list() {
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    let renamed = Arc::new(AtomicBool::new(false));
    let queued = Arc::new(AtomicBool::new(false));
    mount_trigger(
        &server,
        queued.clone(),
        renamed.clone(),
        "n1",
        notification("n1", &rename_event()),
    );
    mount_room_list(&server, renamed);
    common::mount_empty_notifications(&server);

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let ws_base = format!("ws://{}", listener.local_addr().expect("addr"));
    let accepted = Arc::new(AtomicUsize::new(0));
    let count = accepted.clone();
    let ws_server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept");
        let mut ws = tokio_tungstenite::accept_async(stream)
            .await
            .expect("upgrade");
        count.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(200)).await;
        queued.store(true, Ordering::SeqCst);
        ws.send(WsMessage::binary(
            notification("n1", &rename_event()).into_bytes(),
        ))
        .await
        .expect("ring");
        read_until_closed(ws).await;
    });

    let (client, _account) = logged_in_client(&server, &ws_base).await;
    push_for(&client, Duration::from_millis(1500)).await;
    ws_server.abort();

    assert_eq!(accepted.load(Ordering::SeqCst), 1, "no reconnect needed");
    assert_eq!(room_name(&client).await, NEW_NAME);
}
