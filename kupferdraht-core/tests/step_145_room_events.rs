// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 145 — room events as timeline items (TDD red). The event JSON is
//! the shape measured on the wire-server-rs stack (`docs/wire/api-notes.md`,
//! "Step 145 discovery"). The browser's projection cannot run here: its
//! surface is pinned by reading the sources, its names in
//! `timeline/room_event.rs`.
#![cfg(not(target_arch = "wasm32"))]

mod common;

use std::sync::{Arc, Mutex};

use common::drain_fixture::{
    BOB, DOMAIN, Fixture, Notif, ROOM_UUID, cursor, drain, joined_client, mls_event,
    mount_message_upload, mount_page, put_len_delimited, reaction_message, room_id, text_message,
};
use httpmock::MockServer;
use kupferdraht::KupferdrahtError;
use kupferdraht::OutgoingContent;
use kupferdraht::ReceiptType;
use kupferdraht::SyncSettings;
use kupferdraht::storage::Store;
use kupferdraht::timeline::{
    DeliveryStatus, MembershipChange, MessagesOptions, RoomMembershipChange, TimelineDiff,
    TimelineItem, TimelineItemKind, TimelineListener,
};

const OTHER_ROOM_UUID: &str = "a5a5a5a5-77aa-4bde-9d10-223344556677";
const CAROL: &str = "carol-uuid";
const DAVE: &str = "dave-uuid";
const TIME: &str = "2026-09-28T10:00:05.000Z";

fn qualified(user: &str) -> String {
    format!("{user}@{DOMAIN}")
}

fn event_in(conversation: &str, ty: &str, from: &str, time: &str, data: &str) -> String {
    format!(
        r#"{{"conversation":"{conversation}","data":{data},"from":"{from}",
             "qualified_conversation":{{"domain":"{DOMAIN}","id":"{conversation}"}},
             "qualified_from":{{"domain":"{DOMAIN}","id":"{from}"}},
             "time":"{time}","type":"{ty}","via":"user"}}"#
    )
}

fn event(ty: &str, from: &str, data: &str) -> String {
    event_in(ROOM_UUID, ty, from, TIME, data)
}

fn join_data(users: &[&str]) -> String {
    let ids: Vec<String> = users.iter().map(|u| format!(r#""{u}""#)).collect();
    let members: Vec<String> = users
        .iter()
        .map(|u| {
            format!(
                r#"{{"conversation_role":"wire_member","id":"{u}",
                     "qualified_id":{{"domain":"{DOMAIN}","id":"{u}"}}}}"#
            )
        })
        .collect();
    format!(
        r#"{{"add_type":"internal_add","user_ids":[{}],"users":[{}]}}"#,
        ids.join(","),
        members.join(",")
    )
}

fn leave_data(users: &[&str], reason: &str) -> String {
    let ids: Vec<String> = users.iter().map(|u| format!(r#""{u}""#)).collect();
    let qualified: Vec<String> = users
        .iter()
        .map(|u| format!(r#"{{"domain":"{DOMAIN}","id":"{u}"}}"#))
        .collect();
    format!(
        r#"{{"qualified_user_ids":[{}],"reason":"{reason}","user_ids":[{}]}}"#,
        qualified.join(","),
        ids.join(",")
    )
}

fn create_data(name: &str) -> String {
    format!(
        r#"{{"access":["invite"],"creator":"{BOB}","group_id":"Z3JwLTk3ZQ==","id":"{ROOM_UUID}",
             "name":"{name}","type":0}}"#
    )
}

fn rename_event(name: &str) -> String {
    event(
        "conversation.rename",
        BOB,
        &format!(r#"{{"name":"{name}"}}"#),
    )
}

/// One more page on the queue, and an empty one after it.
fn next_page(server: &MockServer, since: &'static str, id: &'static str, events: Vec<String>) {
    mount_page(server, Some(since), &[Notif { id, events }]);
    mount_page(server, Some(id), &[]);
}

/// A client joined to the fixture's room with `events` drained.
async fn drained(server: &MockServer, events: Vec<String>) -> Fixture {
    let fx = joined_client(server).await;
    next_page(server, "n1", "n2", events);
    drain(&fx.client).await;
    fx
}

async fn items(fx: &Fixture) -> Vec<TimelineItem> {
    fx.client
        .messages(&room_id(), &MessagesOptions::all())
        .await
        .expect("messages")
}

async fn only_item(fx: &Fixture) -> TimelineItem {
    let mut all = items(fx).await;
    assert_eq!(all.len(), 1, "one item: {all:?}");
    all.remove(0)
}

fn own_user(fx: &Fixture) -> String {
    fx.client.user_id().expect("logged in").as_str().to_string()
}

fn membership(change: MembershipChange, actor: &str, users: &[&str]) -> TimelineItemKind {
    TimelineItemKind::MembershipChange(RoomMembershipChange {
        change,
        actor: qualified(actor),
        users: users.iter().map(|u| qualified(u)).collect(),
    })
}

/// `GenericMessage { message_id = 1, deleted = 14 { message_id = 1 } }`.
fn delete_message(message_id: &str, target: &str) -> Vec<u8> {
    let mut delete = Vec::new();
    put_len_delimited(1, target.as_bytes(), &mut delete);
    let mut m = Vec::new();
    put_len_delimited(1, message_id.as_bytes(), &mut m);
    put_len_delimited(14, &delete, &mut m);
    m
}

/// `GenericMessage { message_id = 1, edited = 15 { replacing_message_id = 1,
/// text = 2 { content = 1 } } }`.
fn edit_message(message_id: &str, target: &str, content: &str) -> Vec<u8> {
    let mut text = Vec::new();
    put_len_delimited(1, content.as_bytes(), &mut text);
    let mut edit = Vec::new();
    put_len_delimited(1, target.as_bytes(), &mut edit);
    put_len_delimited(2, &text, &mut edit);
    let mut m = Vec::new();
    put_len_delimited(1, message_id.as_bytes(), &mut m);
    put_len_delimited(15, &edit, &mut m);
    m
}

#[derive(Default)]
struct Appends(Mutex<Vec<String>>);

impl TimelineListener for Appends {
    fn on_update(&self, diff: TimelineDiff) {
        if let TimelineDiff::Append { item, .. } = diff {
            self.0.lock().expect("appends").push(item.item_id);
        }
    }
}

// ---- the four events ----

#[tokio::test]
async fn a_created_room_is_one_item_with_its_name_and_creator() {
    let server = MockServer::start();
    let events = vec![event(
        "conversation.create",
        BOB,
        &create_data("Work queue"),
    )];
    let fx = drained(&server, events).await;

    let item = only_item(&fx).await;

    assert_eq!(
        item.kind,
        TimelineItemKind::RoomCreated {
            name: Some("Work queue".into())
        }
    );
    assert_eq!(item.sender_user_id, qualified(BOB));
    assert_eq!(item.sender_device_id, "", "a server event has no device");
    assert_eq!(item.delivery_status, DeliveryStatus::Received);
    assert!(!item.redacted);
}

#[tokio::test]
async fn a_rename_is_one_item_with_the_new_name() {
    let server = MockServer::start();
    let fx = drained(&server, vec![rename_event("Renamed")]).await;

    let item = only_item(&fx).await;

    assert_eq!(
        item.kind,
        TimelineItemKind::RoomNameChange {
            name: "Renamed".into()
        }
    );
    assert_eq!(item.sender_user_id, qualified(BOB));
}

#[tokio::test]
async fn a_join_by_its_only_user_is_joined() {
    let server = MockServer::start();
    let events = vec![event(
        "conversation.member-join",
        CAROL,
        &join_data(&[CAROL]),
    )];
    let fx = drained(&server, events).await;

    assert_eq!(
        only_item(&fx).await.kind,
        membership(MembershipChange::Joined, CAROL, &[CAROL])
    );
}

#[tokio::test]
async fn an_add_names_the_actor_and_every_user_in_one_item() {
    let server = MockServer::start();
    let events = vec![event(
        "conversation.member-join",
        BOB,
        &join_data(&[CAROL, DAVE]),
    )];
    let fx = drained(&server, events).await;

    assert_eq!(
        only_item(&fx).await.kind,
        membership(MembershipChange::Added, BOB, &[CAROL, DAVE])
    );
}

/// `dm-create` on the stack: one join naming both, by the one who opened it,
/// and no `conversation.create`.
#[tokio::test]
async fn a_direct_rooms_first_join_is_an_add_of_both_users() {
    let server = MockServer::start();
    let events = vec![event(
        "conversation.member-join",
        BOB,
        &join_data(&[BOB, CAROL]),
    )];
    let fx = drained(&server, events).await;

    assert_eq!(
        only_item(&fx).await.kind,
        membership(MembershipChange::Added, BOB, &[BOB, CAROL])
    );
}

#[tokio::test]
async fn a_user_who_left_is_left() {
    let server = MockServer::start();
    let events = vec![event(
        "conversation.member-leave",
        CAROL,
        &leave_data(&[CAROL], "left"),
    )];
    let fx = drained(&server, events).await;

    assert_eq!(
        only_item(&fx).await.kind,
        membership(MembershipChange::Left, CAROL, &[CAROL])
    );
}

#[tokio::test]
async fn a_user_removed_by_another_is_kicked() {
    let server = MockServer::start();
    let events = vec![event(
        "conversation.member-leave",
        BOB,
        &leave_data(&[CAROL], "removed"),
    )];
    let fx = drained(&server, events).await;

    assert_eq!(
        only_item(&fx).await.kind,
        membership(MembershipChange::Kicked, BOB, &[CAROL])
    );
}

/// The stack names the deleted user as `from`, so "kicked" would read as the
/// user removing themself.
#[tokio::test]
async fn a_deleted_account_is_left() {
    let server = MockServer::start();
    let events = vec![event(
        "conversation.member-leave",
        CAROL,
        &leave_data(&[CAROL], "user-deleted"),
    )];
    let fx = drained(&server, events).await;

    assert_eq!(
        only_item(&fx).await.kind,
        membership(MembershipChange::Left, CAROL, &[CAROL])
    );
}

// ---- what the sync does with them ----

#[tokio::test]
async fn a_malformed_event_is_skipped_and_the_sync_goes_on() {
    let no_from = r#"{"type":"conversation.rename","time":"2026-09-28T10:00:05.000Z",
        "qualified_conversation":{"id":"97e0aa31-77aa-4bde-9d10-223344556677","domain":"example.test"},
        "data":{"name":"x"}}"#;
    let no_time = r#"{"type":"conversation.rename",
        "qualified_from":{"id":"bob-uuid","domain":"example.test"},
        "qualified_conversation":{"id":"97e0aa31-77aa-4bde-9d10-223344556677","domain":"example.test"},
        "data":{"name":"x"}}"#;
    let no_room = r#"{"type":"conversation.rename","time":"2026-09-28T10:00:05.000Z",
        "qualified_from":{"id":"bob-uuid","domain":"example.test"},"data":{"name":"x"}}"#;
    let no_data = r#"{"type":"conversation.create","time":"2026-09-28T10:00:05.000Z",
        "qualified_from":{"id":"bob-uuid","domain":"example.test"},
        "qualified_conversation":{"id":"97e0aa31-77aa-4bde-9d10-223344556677","domain":"example.test"}}"#;
    let mut events: Vec<String> = [no_from, no_time, no_room, no_data]
        .iter()
        .map(ToString::to_string)
        .collect();
    events.extend([
        event("conversation.rename", BOB, r#"{}"#),
        event("conversation.rename", BOB, r#"{"name":7}"#),
        event("conversation.member-join", BOB, r#"{"user_ids":[]}"#),
        event("conversation.member-join", BOB, r#"{"users":[]}"#),
        event(
            "conversation.member-join",
            BOB,
            r#"{"users":[{"id":"carol-uuid"}]}"#,
        ),
        event(
            "conversation.member-leave",
            BOB,
            &leave_data(&[CAROL], "lost-in-space"),
        ),
        event("conversation.member-leave", BOB, r#"{"reason":"left"}"#),
        event(
            "conversation.member-leave",
            BOB,
            r#"{"qualified_user_ids":[],"reason":"left"}"#,
        ),
        rename_event("The one that counts"),
    ]);
    let server = MockServer::start();
    let fx = drained(&server, events).await;

    let item = only_item(&fx).await;

    assert_eq!(
        item.kind,
        TimelineItemKind::RoomNameChange {
            name: "The one that counts".into()
        }
    );
    assert_eq!(fx.recorder.errors(), Vec::<String>::new());
    assert_eq!(cursor(&fx.store).await.as_deref(), Some("n2"));
}

#[tokio::test]
async fn the_same_event_twice_keeps_one_row_and_one_append() {
    let server = MockServer::start();
    let fx = joined_client(&server).await;
    let room = fx.client.room(&room_id()).await.expect("room");
    let appends = Arc::new(Appends::default());
    let _subscription = room
        .timeline()
        .expect("timeline")
        .subscribe(appends.clone());
    let again = rename_event("Renamed");
    mount_page(
        &server,
        Some("n1"),
        &[
            Notif {
                id: "n2",
                events: vec![again.clone()],
            },
            Notif {
                id: "n3",
                events: vec![again.clone()],
            },
        ],
    );
    next_page(&server, "n3", "n4", vec![again]);
    drain(&fx.client).await;
    drain(&fx.client).await;

    let item = only_item(&fx).await;

    assert_eq!(cursor(&fx.store).await.as_deref(), Some("n4"), "delivered");
    assert_eq!(*appends.0.lock().expect("appends"), vec![item.item_id]);
}

/// A known answer: the id must not change between versions, or a redelivered
/// event would stand twice. Computed outside Kupferdraht: the first 16 bytes
/// of SHA-256 over the JSON array of a version tag, the event type, the room,
/// the actor, the event's time and the row's body, as a UUID of version 8.
#[tokio::test]
async fn an_items_id_comes_from_the_events_content() {
    let server = MockServer::start();
    let fx = drained(&server, vec![rename_event("Renamed")]).await;

    assert_eq!(
        only_item(&fx).await.item_id,
        "70d6a473-12e5-8b44-b8f2-0d33a8bdda01"
    );
}

#[tokio::test]
async fn the_same_change_at_another_time_is_another_item() {
    let server = MockServer::start();
    let renamed = r#"{"name":"Renamed"}"#;
    let events = vec![
        event_in(ROOM_UUID, "conversation.rename", BOB, TIME, renamed),
        event_in(
            ROOM_UUID,
            "conversation.rename",
            BOB,
            "2026-09-28T10:00:05.004Z",
            renamed,
        ),
    ];
    let fx = drained(&server, events).await;

    assert_eq!(items(&fx).await.len(), 2);
}

#[tokio::test]
async fn an_event_for_a_room_not_stored_yet_is_kept_for_that_room() {
    let server = MockServer::start();
    let events = vec![event_in(
        OTHER_ROOM_UUID,
        "conversation.member-join",
        BOB,
        TIME,
        &join_data(&[CAROL]),
    )];
    let fx = drained(&server, events).await;
    let other_room = format!("{OTHER_ROOM_UUID}@{DOMAIN}");

    let kept = fx
        .client
        .messages(&other_room, &MessagesOptions::all())
        .await
        .expect("messages");

    assert!(fx.client.room(&other_room).await.is_none());
    assert_eq!(kept.len(), 1);
    assert_eq!(items(&fx).await, Vec::new(), "not in the fixture's room");
}

/// The position is this device's clock, on every row (Step 151): by the
/// event's `time` an item would stand before everything a catch-up drain
/// delivers, or past the next message and over a read marker set on it.
#[tokio::test]
async fn an_item_stands_where_it_arrived_whatever_the_servers_clock_says() {
    let server = MockServer::start();
    let mut fx = joined_client(&server).await;
    let first = mls_event(&fx.bob.application_message(&text_message("m1", "first")));
    let second = mls_event(&fx.bob.application_message(&text_message("m2", "second")));
    let third = mls_event(&fx.bob.application_message(&text_message("m3", "third")));
    let renamed = r#"{"name":"Renamed"}"#;
    let past = event_in(
        ROOM_UUID,
        "conversation.rename",
        BOB,
        "2020-01-01T00:00:00.000Z",
        renamed,
    );
    let future = event_in(
        ROOM_UUID,
        "conversation.rename",
        BOB,
        "2099-01-01T00:00:00.000Z",
        renamed,
    );
    next_page(
        &server,
        "n1",
        "n2",
        vec![first, past, second, future, third],
    );
    drain(&fx.client).await;

    let all = items(&fx).await;

    let order: Vec<bool> = all
        .iter()
        .map(|i| matches!(i.kind, TimelineItemKind::RoomNameChange { .. }))
        .collect();
    assert_eq!(order, [false, true, false, true, false]);
    assert!(
        all.windows(2)
            .all(|w| w[0].arrived_at_ms <= w[1].arrived_at_ms),
        "one clock: {:?}",
        all.iter().map(|i| i.arrived_at_ms).collect::<Vec<_>>()
    );
}

/// The browser has no timeline diffs: its signal is `onSync`'s `changedRooms`.
#[tokio::test]
async fn a_pass_with_a_room_event_names_its_room() {
    let server = MockServer::start();
    let fx = joined_client(&server).await;
    next_page(&server, "n1", "n2", vec![rename_event("Renamed")]);

    let summary = fx
        .client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync_once");

    assert_eq!(summary.changed_rooms, vec![room_id()], "{summary:?}");
    assert_eq!(items(&fx).await.len(), 1);
    assert_eq!(
        fx.client
            .sync_once(SyncSettings::default())
            .await
            .expect("sync_once")
            .changed_rooms,
        Vec::<String>::new(),
        "the next pass names nothing"
    );
}

#[tokio::test]
async fn an_item_reaches_timeline_listeners_but_is_no_message() {
    let server = MockServer::start();
    let fx = joined_client(&server).await;
    let room = fx.client.room(&room_id()).await.expect("room");
    let appends = Arc::new(Appends::default());
    let _subscription = room
        .timeline()
        .expect("timeline")
        .subscribe(appends.clone());
    next_page(&server, "n1", "n2", vec![rename_event("Renamed")]);
    drain(&fx.client).await;

    let item = only_item(&fx).await;

    assert_eq!(
        *appends.0.lock().expect("appends"),
        vec![item.item_id.clone()]
    );
    assert_eq!(fx.recorder.count(&item.item_id), 0, "no on_message");
}

// ---- what the items are not ----

#[tokio::test]
async fn an_item_after_the_marker_is_not_unread_in_either_query() {
    let server = MockServer::start();
    let mut fx = joined_client(&server).await;
    fx.store
        .set_read_marker(&room_id(), 1)
        .await
        .expect("marker");
    let message = mls_event(&fx.bob.application_message(&text_message("m1", "unread")));
    let events = vec![
        event("conversation.create", BOB, &create_data("Work queue")),
        rename_event("Renamed"),
        event("conversation.member-join", BOB, &join_data(&[CAROL])),
        event(
            "conversation.member-leave",
            BOB,
            &leave_data(&[CAROL], "removed"),
        ),
        message,
    ];
    next_page(&server, "n1", "n2", events);
    drain(&fx.client).await;
    let me = own_user(&fx);

    let one_room = fx
        .store
        .unread_count_for_room(&room_id(), &me)
        .await
        .expect("count");
    let all_rooms = fx.store.unread_counts(&me).await.expect("counts");

    assert_eq!(items(&fx).await.len(), 5);
    assert_eq!(one_room, 1, "the message alone");
    assert_eq!(all_rooms.get(&room_id()), Some(&1));
}

#[tokio::test]
async fn mark_as_read_on_an_item_moves_the_marker_and_sends_nothing() {
    let server = MockServer::start();
    let uploads = mount_message_upload(&server);
    let fx = drained(&server, vec![rename_event("Renamed")]).await;
    let room = fx.client.room(&room_id()).await.expect("room");
    let timeline = room.timeline().expect("timeline");
    let item = only_item(&fx).await;

    let read = timeline
        .mark_as_read(item.item_id.clone(), ReceiptType::Read)
        .await
        .expect("mark_as_read");

    assert_eq!(read.map(|h| h.message_id), None);
    assert_eq!(
        fx.store.read_marker(&room_id()).await.expect("marker"),
        Some(item.arrived_at_ms)
    );
    assert_eq!(uploads.lock().expect("uploads").len(), 0);
}

#[tokio::test]
async fn mark_as_read_on_a_message_after_an_item_still_tells_the_room() {
    let server = MockServer::start();
    let uploads = mount_message_upload(&server);
    let mut fx = joined_client(&server).await;
    let message = mls_event(&fx.bob.application_message(&text_message("m1", "first")));
    next_page(&server, "n1", "n2", vec![rename_event("Renamed"), message]);
    drain(&fx.client).await;
    let room = fx.client.room(&room_id()).await.expect("room");

    let read = room
        .timeline()
        .expect("timeline")
        .mark_as_read("m1".into(), ReceiptType::Read)
        .await
        .expect("mark_as_read");

    assert!(read.is_some());
    assert_eq!(uploads.lock().expect("uploads").len(), 1);
}

/// The item's actor is the own user here, or `edit` and `redact` would refuse
/// it as another user's.
#[tokio::test]
async fn a_reaction_reply_edit_or_delete_on_an_item_is_refused() {
    let server = MockServer::start();
    let uploads = mount_message_upload(&server);
    let fx = joined_client(&server).await;
    let me = own_user(&fx);
    let own_localpart = me.split('@').next().expect("localpart").to_string();
    next_page(
        &server,
        "n1",
        "n2",
        vec![event(
            "conversation.rename",
            &own_localpart,
            r#"{"name":"Renamed"}"#,
        )],
    );
    drain(&fx.client).await;
    let room = fx.client.room(&room_id()).await.expect("room");
    let timeline = room.timeline().expect("timeline");
    let id = only_item(&fx).await.item_id;

    let results = [
        timeline.toggle_reaction(id.clone(), "👍".into()).await,
        timeline.send_reply(id.clone(), "a reply".into()).await,
        timeline.edit(id.clone(), "an edit".into()).await,
        timeline.redact(&id).await,
        timeline
            .send_keyed(
                "key-reaction".into(),
                OutgoingContent::Reaction {
                    item_id: id.clone(),
                    emoji: "👍".into(),
                },
            )
            .await,
        timeline
            .send_keyed(
                "key-reply".into(),
                OutgoingContent::Reply {
                    parent_id: id.clone(),
                    body: "a reply".into(),
                },
            )
            .await,
    ];

    for (i, result) in results.into_iter().enumerate() {
        assert!(
            matches!(result, Err(KupferdrahtError::InvalidInput(_))),
            "call {i}: {result:?}"
        );
    }
    assert_eq!(uploads.lock().expect("uploads").len(), 0);
    assert_eq!(
        only_item(&fx).await.kind,
        TimelineItemKind::RoomNameChange {
            name: "Renamed".into()
        }
    );
}

/// An item's id can be computed by anyone who saw the event, and ADR-0028
/// would let the actor edit or delete a row it sent: "Bob removed Carol"
/// must not be Bob's to rewrite, and no one's to react to.
#[tokio::test]
async fn an_inbound_edit_delete_or_reaction_on_an_item_has_no_effect() {
    let server = MockServer::start();
    let mut fx = joined_client(&server).await;
    let removal = event(
        "conversation.member-leave",
        BOB,
        &leave_data(&[CAROL], "removed"),
    );
    mount_page(
        &server,
        Some("n1"),
        &[Notif {
            id: "n2",
            events: vec![removal],
        }],
    );
    drain(&fx.client).await;
    let id = only_item(&fx).await.item_id;
    let edit = mls_event(
        &fx.bob
            .application_message(&edit_message("e1", &id, "rewritten")),
    );
    let delete = mls_event(&fx.bob.application_message(&delete_message("d1", &id)));
    let react = mls_event(
        &fx.bob
            .application_message(&reaction_message("r1", &id, "👍")),
    );
    next_page(&server, "n2", "n3", vec![edit, delete, react]);
    drain(&fx.client).await;

    let item = only_item(&fx).await;

    assert_eq!(cursor(&fx.store).await.as_deref(), Some("n3"), "delivered");
    assert_eq!(
        item.kind,
        membership(MembershipChange::Kicked, BOB, &[CAROL])
    );
    assert!(!item.redacted);
    assert_eq!(item.edited_at_ms, None);
    assert_eq!(item.reactions, Vec::new());
    assert_eq!(fx.recorder.errors(), Vec::<String>::new());
}

/// The control of the one above: the same three reach a message of Bob's.
#[tokio::test]
async fn the_same_inbound_edit_and_reaction_do_reach_a_message() {
    let server = MockServer::start();
    let mut fx = joined_client(&server).await;
    let message = mls_event(&fx.bob.application_message(&text_message("m1", "first")));
    let edit = mls_event(
        &fx.bob
            .application_message(&edit_message("e1", "m1", "rewritten")),
    );
    let react = mls_event(
        &fx.bob
            .application_message(&reaction_message("r1", "m1", "👍")),
    );
    next_page(&server, "n1", "n2", vec![message, edit, react]);
    drain(&fx.client).await;

    let item = only_item(&fx).await;

    assert!(item.edited_at_ms.is_some());
    assert_eq!(item.reactions.len(), 1);
}

/// A storage failure is a persistence failure like a message's: the cursor
/// stays and the next pass delivers the event again (Step 97e).
#[tokio::test]
async fn an_item_whose_write_failed_comes_with_the_next_pass() {
    let server = MockServer::start();
    let fx = joined_client(&server).await;
    next_page(&server, "n1", "n2", vec![rename_event("Renamed")]);
    fx.store.fail_when(Box::new(|method| {
        method == "insert_timeline_event_with_mentions"
    }));
    let _ = fx.client.sync_once(SyncSettings::default()).await;
    assert_eq!(items(&fx).await.len(), 0);
    assert_eq!(cursor(&fx.store).await.as_deref(), Some("n1"));

    fx.store.heal();
    drain(&fx.client).await;

    assert_eq!(items(&fx).await.len(), 1);
    assert_eq!(cursor(&fx.store).await.as_deref(), Some("n2"));
}

// ---- the surface ----

#[test]
fn the_kinds_cross_the_ffi() {
    use kupferdraht::ffi::TimelineItemKind as Ffi;

    let change = RoomMembershipChange {
        change: MembershipChange::Added,
        actor: qualified(BOB),
        users: vec![qualified(CAROL)],
    };

    assert_eq!(
        Ffi::from(&TimelineItemKind::MembershipChange(change.clone())),
        Ffi::MembershipChange { change }
    );
    assert_eq!(
        Ffi::from(&TimelineItemKind::RoomNameChange { name: "n".into() }),
        Ffi::RoomNameChange { name: "n".into() }
    );
    assert_eq!(
        Ffi::from(&TimelineItemKind::RoomCreated { name: None }),
        Ffi::RoomCreated { name: None }
    );
}

fn workspace_file(path: &str) -> String {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace");
    std::fs::read_to_string(root.join(path)).expect(path)
}

#[test]
fn the_browser_projects_the_kinds_and_its_types_say_so() {
    let projection = workspace_file("kupferdraht-core/src/wasm/manager.rs");
    for kind in ["membership_change", "room_name_change", "room_created"] {
        assert!(
            projection.contains(&format!("\"{kind}\"")),
            "wasm projection: {kind}"
        );
    }
    let types = workspace_file("bindings/web/kupferdraht-tabs/index.d.ts");
    for field in [
        "change: 'joined' | 'added' | 'left' | 'kicked' | null",
        "actor: string | null",
        "users: string[] | null",
        "name: string | null",
    ] {
        assert!(types.contains(field), "index.d.ts: {field}");
    }
}

#[test]
fn the_docs_say_what_an_embedder_has_to_know() {
    let bindings = workspace_file("docs/bindings.md");
    for rule in [
        "`membership_change`",
        "`room_name_change`",
        "`room_created`",
        "history starts with the device",
    ] {
        assert!(bindings.contains(rule), "docs/bindings.md: {rule}");
    }
    let threat_model = workspace_file("docs/THREAT_MODEL.md");
    assert!(threat_model.contains("Alice removed Bob"));
    let changelog = workspace_file("CHANGELOG.md");
    assert!(changelog.contains("BREAKING: `TimelineItemKind` has three new kinds"));
}
