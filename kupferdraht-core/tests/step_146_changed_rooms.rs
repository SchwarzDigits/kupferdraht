// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 146 — a sync pass names the rooms that changed (TDD red).
//! `SyncSummary::changed_rooms` replaces the count `rooms_updated`, which was
//! the number of all rooms on every pass and `0` on a push tick. matrix-rust-sdk
//! analog: `sliding_sync::UpdateSummary::rooms`.
#![cfg(not(target_arch = "wasm32"))]

mod common;

use std::sync::Arc;

use common::drain_fixture::{
    BOB, DOMAIN, Notif, ROOM_UUID, drain, joined_client, mls_event, mount_page, room_id,
    text_message,
};
use httpmock::Method::{GET, POST, PUT};
use httpmock::{Mock, MockServer};
use kupferdraht::keystore::{InMemoryKeyStore, SecureKeyStore};
use kupferdraht::{Client, Kupferdraht, ReceiptType, SyncSettings, SyncSummary};
use tempfile::TempDir;

const OTHER_ROOM_UUID: &str = "a5a5a5a5-77aa-4bde-9d10-223344556677";
const ROOM_A: &str = "room-a";
const ROOM_B: &str = "room-b";
const FIXTURE_DOMAIN: &str = "example.test";

fn qualified_room(uuid: &str) -> String {
    format!("{uuid}@{FIXTURE_DOMAIN}")
}

async fn pass(client: &Client) -> SyncSummary {
    client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync_once")
}

// ---- events, on a client joined to the fixture's room ----

fn rename_event(room_uuid: &str, name: &str) -> String {
    format!(
        r#"{{"type":"conversation.rename","data":{{"name":"{name}"}},"time":"2026-09-28T10:00:05.000Z",
             "qualified_conversation":{{"id":"{room_uuid}","domain":"{DOMAIN}"}},
             "qualified_from":{{"id":"{BOB}","domain":"{DOMAIN}"}},"via":"user"}}"#
    )
}

fn delete_event(room_uuid: &str) -> String {
    format!(
        r#"{{"type":"conversation.delete","data":null,"time":"2026-09-28T10:00:06.000Z",
             "qualified_conversation":{{"id":"{room_uuid}","domain":"{DOMAIN}"}},
             "qualified_from":{{"id":"{BOB}","domain":"{DOMAIN}"}},"via":"user"}}"#
    )
}

fn next_page(server: &MockServer, since: &'static str, id: &'static str, events: Vec<String>) {
    mount_page(server, Some(since), &[Notif { id, events }]);
    mount_page(server, Some(id), &[]);
}

#[tokio::test]
async fn a_room_event_names_its_room_and_the_next_pass_nothing() {
    let server = MockServer::start();
    let fx = joined_client(&server).await;
    mount_page(
        &server,
        Some("n1"),
        &[Notif {
            id: "n2",
            events: vec![rename_event(ROOM_UUID, "Renamed")],
        }],
    );
    mount_page(&server, Some("n2"), &[]);

    let first = pass(&fx.client).await;
    let second = pass(&fx.client).await;

    assert_eq!(first.changed_rooms, vec![room_id()]);
    assert_eq!(second.changed_rooms, Vec::<String>::new());
}

#[tokio::test]
async fn two_events_of_one_room_name_it_once() {
    let server = MockServer::start();
    let fx = joined_client(&server).await;
    let events = vec![
        rename_event(ROOM_UUID, "Renamed"),
        rename_event(ROOM_UUID, "Renamed again"),
    ];
    next_page(&server, "n1", "n2", events);

    assert_eq!(pass(&fx.client).await.changed_rooms, vec![room_id()]);
}

#[tokio::test]
async fn events_of_two_rooms_name_both_in_order() {
    let server = MockServer::start();
    let fx = joined_client(&server).await;
    let events = vec![
        rename_event(ROOM_UUID, "Renamed"),
        rename_event(OTHER_ROOM_UUID, "Elsewhere"),
    ];
    next_page(&server, "n1", "n2", events);

    let mut both = vec![room_id(), format!("{OTHER_ROOM_UUID}@{DOMAIN}")];
    both.sort();
    assert_eq!(pass(&fx.client).await.changed_rooms, both);
}

/// The same event twice is one item (Step 145), and changes nothing the
/// second time.
#[tokio::test]
async fn a_redelivered_event_names_nothing() {
    let server = MockServer::start();
    let fx = joined_client(&server).await;
    let renamed = rename_event(ROOM_UUID, "Renamed");
    mount_page(
        &server,
        Some("n1"),
        &[Notif {
            id: "n2",
            events: vec![renamed.clone()],
        }],
    );
    next_page(&server, "n2", "n3", vec![renamed]);

    let first = pass(&fx.client).await;
    let second = pass(&fx.client).await;

    assert_eq!(first.changed_rooms, vec![room_id()]);
    assert_eq!(second.changed_rooms, Vec::<String>::new());
}

/// The deleting device gets its own `conversation.delete` (measured on the
/// local stack), after its own call already took the room off the list.
#[tokio::test]
async fn a_deleted_room_is_named_also_when_it_was_gone_already() {
    let server = MockServer::start();
    let fx = joined_client(&server).await;
    let events = vec![delete_event(ROOM_UUID), delete_event(OTHER_ROOM_UUID)];
    next_page(&server, "n1", "n2", events);

    let mut both = vec![room_id(), format!("{OTHER_ROOM_UUID}@{DOMAIN}")];
    both.sort();
    assert_eq!(pass(&fx.client).await.changed_rooms, both);
}

#[tokio::test]
async fn a_pass_with_only_messages_names_nothing() {
    let server = MockServer::start();
    let mut fx = joined_client(&server).await;
    let message = mls_event(&fx.bob.application_message(&text_message("m1", "hello")));
    next_page(&server, "n1", "n2", vec![message]);

    let summary = pass(&fx.client).await;

    assert_eq!(summary.events_dispatched, 1);
    assert_eq!(summary.changed_rooms, Vec::<String>::new());
}

/// Read state has its own report (`onRoomsChanged`, Step 138).
#[tokio::test]
async fn a_read_names_nothing() {
    let server = MockServer::start();
    let mut fx = joined_client(&server).await;
    let message = mls_event(&fx.bob.application_message(&text_message("m1", "hello")));
    next_page(&server, "n1", "n2", vec![message]);
    drain(&fx.client).await;
    let room = fx.client.room(&room_id()).await.expect("room");
    room.timeline()
        .expect("timeline")
        .mark_as_read("m1".into(), ReceiptType::ReadPrivate)
        .await
        .expect("mark_as_read");

    assert_eq!(pass(&fx.client).await.changed_rooms, Vec::<String>::new());
}

/// A failed pass reports nothing and loses nothing: the next one names it.
#[tokio::test]
async fn a_failed_pass_keeps_its_rooms_for_the_next() {
    let server = MockServer::start();
    let fx = joined_client(&server).await;
    let renamed = rename_event(ROOM_UUID, "Renamed");
    server.mock(move |when, then| {
        when.method(GET)
            .path("/v14/notifications")
            .query_param("since", "n1");
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(
                r#"{{"notifications":[{{"id":"n2","payload":[{renamed}]}}],"has_more":true}}"#
            ));
    });
    let mut broken = server.mock(|when, then| {
        when.method(GET)
            .path("/v14/notifications")
            .query_param("since", "n2");
        then.status(500).body("{}");
    });
    assert!(fx.client.sync_once(SyncSettings::default()).await.is_err());
    broken.delete();
    mount_page(&server, Some("n2"), &[]);

    assert_eq!(pass(&fx.client).await.changed_rooms, vec![room_id()]);
}

// ---- the room list, on a client of its own ----

const QUALIFIED_USER: &str = "user-uuid@example.test";

fn conversation(uuid: &str, name: &str, epoch: u64) -> String {
    format!(
        r#"{{"qualified_id":{{"id":"{uuid}","domain":"{FIXTURE_DOMAIN}"}},"type":0,"protocol":"mls",
             "epoch":{epoch},"group_id":"Z3JwLXt9","name":"{name}",
             "members":{{"self":{{"qualified_id":{{"id":"user-uuid","domain":"{FIXTURE_DOMAIN}"}},
                                 "conversation_role":"wire_admin"}},
                        "others":[{{"qualified_id":{{"id":"bob-uuid","domain":"{FIXTURE_DOMAIN}"}},
                                    "conversation_role":"wire_member"}}]}}}}"#
    )
}

/// What the backend answers about the rooms: their ids, names and epochs, and
/// Bob's display name.
fn mount_list<'a>(server: &'a MockServer, rooms: &[(&str, &str, u64)], bob: &str) -> Vec<Mock<'a>> {
    let ids: Vec<String> = rooms
        .iter()
        .map(|(uuid, _, _)| format!(r#"{{"id":"{uuid}","domain":"{FIXTURE_DOMAIN}"}}"#))
        .collect();
    let found: Vec<String> = rooms
        .iter()
        .map(|(uuid, name, epoch)| conversation(uuid, name, *epoch))
        .collect();
    let ids_body = format!(
        r#"{{"qualified_conversations":[{}],"has_more":false,"paging_state":"END"}}"#,
        ids.join(",")
    );
    let list_body = format!(
        r#"{{"found":[{}],"not_found":[],"failed":[]}}"#,
        found.join(",")
    );
    let users_body = format!(
        r#"{{"found":[{{"qualified_id":{{"id":"bob-uuid","domain":"{FIXTURE_DOMAIN}"}},"name":"{bob}"}}]}}"#
    );
    vec![
        server.mock(move |when, then| {
            when.method(POST).path("/v14/conversations/list-ids");
            then.status(200)
                .header("content-type", "application/json")
                .body(ids_body.clone());
        }),
        server.mock(move |when, then| {
            when.method(POST).path("/v14/conversations/list");
            then.status(200)
                .header("content-type", "application/json")
                .body(list_body.clone());
        }),
        server.mock(move |when, then| {
            when.method(POST).path("/v14/list-users");
            then.status(200)
                .header("content-type", "application/json")
                .body(users_body.clone());
        }),
    ]
}

fn replace(old: Vec<Mock<'_>>) {
    for mut mock in old {
        mock.delete();
    }
}

/// The queue: empty from the start, then one page with an event that says
/// nothing Kupferdraht reads but that the room list may have changed.
fn mount_queue(server: &MockServer) -> Mock<'_> {
    server.mock(|when, then| {
        when.method(GET)
            .path("/v14/notifications")
            .query_param_missing("since");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"notifications":[],"has_more":false}"#);
    })
}

fn mount_list_changed_event(server: &MockServer) -> Mock<'_> {
    server.mock(|when, then| {
        when.method(GET)
            .path("/v14/notifications")
            .query_param_missing("since");
        then.status(200)
            .header("content-type", "application/json")
            .body(
                r#"{"notifications":[{"id":"u1","payload":[{"type":"user.update",
                     "user":{"id":"bob-uuid","name":"Bob Builder"}}]}],"has_more":false}"#,
            );
    })
}

struct Session {
    _home: TempDir,
    app: Kupferdraht,
    client: Arc<Client>,
}

async fn login(server: &MockServer) -> Session {
    common::mount_full_login_flow(server);
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

#[tokio::test]
async fn the_first_pass_of_a_new_device_names_every_room() {
    let server = MockServer::start();
    let _list = mount_list(&server, &[(ROOM_A, "A", 1), (ROOM_B, "B", 1)], "Bob");
    let _queue = mount_queue(&server);
    let session = login(&server).await;

    let first = pass(&session.client).await;
    let second = pass(&session.client).await;

    assert_eq!(
        first.changed_rooms,
        vec![qualified_room(ROOM_A), qualified_room(ROOM_B)]
    );
    assert_eq!(second.changed_rooms, Vec::<String>::new());
}

/// A resumed device compares with what it stored: nothing changed meanwhile.
#[tokio::test]
async fn a_resumed_device_names_only_what_changed_while_it_was_away() {
    let server = MockServer::start();
    let list = mount_list(&server, &[(ROOM_A, "A", 1), (ROOM_B, "B", 1)], "Bob");
    let _queue = mount_queue(&server);
    let session = login(&server).await;
    pass(&session.client).await;
    drop(session.client);
    replace(list);
    let _list = mount_list(
        &server,
        &[(ROOM_A, "A", 1), (ROOM_B, "B renamed", 1)],
        "Bob",
    );

    let resumed = session.app.resume(QUALIFIED_USER).await.expect("resume");

    assert_eq!(
        pass(&resumed).await.changed_rooms,
        vec![qualified_room(ROOM_B)]
    );
}

/// A refresh some event asked for names the rooms whose name or members
/// changed, a new room and a gone one; an epoch alone is no change.
#[tokio::test]
async fn a_refresh_names_what_differs_and_nothing_else() {
    let server = MockServer::start();
    let list = mount_list(
        &server,
        &[
            (ROOM_A, "A", 1),
            (ROOM_B, "B", 1),
            ("room-c", "C", 1),
            ("room-d", "D", 1),
        ],
        "Bob",
    );
    let queue = mount_queue(&server);
    let session = login(&server).await;
    pass(&session.client).await;
    replace(list);
    replace(vec![queue]);
    let _list = mount_list(
        &server,
        &[
            (ROOM_A, "A", 7),
            (ROOM_B, "B renamed", 1),
            ("room-c", "C", 1),
            ("room-e", "E", 1),
        ],
        "Bob",
    );
    let _event = mount_list_changed_event(&server);

    let summary = pass(&session.client).await;

    assert_eq!(
        summary.changed_rooms,
        vec![
            qualified_room(ROOM_B),
            qualified_room("room-d"),
            qualified_room("room-e"),
        ],
        "renamed, gone, new; room-a's epoch and room-c are no change"
    );
}

/// What `members()` shows counts: a member's display name.
#[tokio::test]
async fn a_members_new_name_names_every_room_it_is_in() {
    let server = MockServer::start();
    let list = mount_list(&server, &[(ROOM_A, "A", 1), (ROOM_B, "B", 1)], "Bob");
    let queue = mount_queue(&server);
    let session = login(&server).await;
    pass(&session.client).await;
    replace(list);
    replace(vec![queue]);
    let _list = mount_list(&server, &[(ROOM_A, "A", 1), (ROOM_B, "B", 1)], "Robert");
    let _event = mount_list_changed_event(&server);

    assert_eq!(
        pass(&session.client).await.changed_rooms,
        vec![qualified_room(ROOM_A), qualified_room(ROOM_B)]
    );
}

#[tokio::test]
async fn a_refresh_that_finds_the_same_list_names_nothing() {
    let server = MockServer::start();
    let _list = mount_list(&server, &[(ROOM_A, "A", 1)], "Bob");
    let queue = mount_queue(&server);
    let session = login(&server).await;
    pass(&session.client).await;
    replace(vec![queue]);
    let _event = mount_list_changed_event(&server);

    assert_eq!(
        pass(&session.client).await.changed_rooms,
        Vec::<String>::new()
    );
}

/// The client's own call is named at the next pass, whether or not the
/// backend echoes it: its own list refresh finds the difference.
#[tokio::test]
async fn an_own_rename_is_named_by_the_next_pass_without_its_echo() {
    let server = MockServer::start();
    let list = mount_list(&server, &[(ROOM_A, "A", 1)], "Bob");
    let _queue = mount_queue(&server);
    let session = login(&server).await;
    pass(&session.client).await;
    server.mock(|when, then| {
        when.method(PUT)
            .path(format!("/v14/conversations/{FIXTURE_DOMAIN}/{ROOM_A}/name"));
        then.status(204);
    });
    replace(list);
    let _list = mount_list(&server, &[(ROOM_A, "A renamed", 1)], "Bob");

    session
        .client
        .rename_room(&qualified_room(ROOM_A), "A renamed")
        .await
        .expect("rename_room");

    assert_eq!(
        pass(&session.client).await.changed_rooms,
        vec![qualified_room(ROOM_A)]
    );
    assert_eq!(
        pass(&session.client).await.changed_rooms,
        Vec::<String>::new()
    );
}

// ---- the surface ----

#[test]
fn the_ffi_record_carries_the_rooms() {
    let summary = SyncSummary {
        changed_rooms: vec![room_id()],
        events_dispatched: 1,
    };

    assert_eq!(summary.clone().changed_rooms, vec![room_id()]);
}

fn workspace_file(path: &str) -> String {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace");
    std::fs::read_to_string(root.join(path)).expect(path)
}

#[test]
fn the_browser_and_the_cli_name_the_rooms() {
    let types = workspace_file("bindings/web/kupferdraht-tabs/index.d.ts");
    assert!(types.contains(
        "export interface SyncSummary { changedRooms: string[]; eventsDispatched: number }"
    ));
    assert!(!types.contains("roomsUpdated"));
    let readme = workspace_file("bindings/web/kupferdraht-tabs/README.md");
    assert!(!readme.contains("roomsUpdated"));
    for wasm in [
        "kupferdraht-core/src/wasm/live_sync.rs",
        "kupferdraht-core/src/wasm/manager.rs",
    ] {
        assert!(!workspace_file(wasm).contains("rooms_updated"), "{wasm}");
    }
    assert!(workspace_file("kupferdraht-cli/src/cli/sync.rs").contains("rooms changed:"));
}

#[test]
fn the_docs_say_what_changed() {
    let bindings = workspace_file("docs/bindings.md");
    assert!(bindings.contains("changedRooms"));
    assert!(!bindings.contains("roomsUpdated"));
    let changelog = workspace_file("CHANGELOG.md");
    assert!(changelog.contains("BREAKING: a sync pass names the rooms that changed"));
}
