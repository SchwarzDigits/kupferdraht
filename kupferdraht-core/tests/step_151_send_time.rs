// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 151 — `sent_at_ms` is the backend's time, `arrived_at_ms` the position
//! (TDD red). The quote hashes are known answers computed outside Kupferdraht:
//! SHA-256 over `FE FF`, the body in UTF-16 BE and the seconds as i64 BE
//! (`docs/observations.md` §33).
#![cfg(not(target_arch = "wasm32"))]

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::drain_fixture::{
    BOB, DOMAIN, Fixture, Notif, ROOM_UUID, b64, drain, joined_client, mount_message_upload,
    mount_page, put_len_delimited, put_varint, room_id, text_message,
};
use httpmock::Method::POST;
use httpmock::MockServer;
use kupferdraht::storage::Store;
use kupferdraht::timeline::{DeliveryStatus, MessagesOptions, TimelineItem, TimelineItemKind};
use kupferdraht::{EventHandler, Message, ReceiptType};

const EVENT_TIME: &str = "2026-09-28T10:00:01.000Z";
const EVENT_TIME_MS: i64 = 1_790_589_601_000;
/// What `mount_message_upload` answers.
const ANSWER_TIME_MS: i64 = 1_790_589_602_000;
const PAST: &str = "2020-01-01T00:00:00.000Z";
const FUTURE: &str = "2099-01-01T00:00:00.000Z";
/// The digest of "first" sent at `EVENT_TIME`.
const QUOTE_OF_INBOUND: &str = "af7501f7196c2792bac63600a798b22e89247ff34892f84387fdb681346c7664";
/// The digest of "mine" sent at the answer's time.
const QUOTE_OF_OWN: &str = "4728d02bbf0fbef5d1ba7b9bad709ad21417bb30b9e6c779c1afd8ca64033aa2";

fn mls_event_at(tls: &[u8], time: &str) -> String {
    format!(
        r#"{{"type":"conversation.mls-message-add",
             "qualified_conversation":{{"id":"{ROOM_UUID}","domain":"{DOMAIN}"}},
             "qualified_from":{{"id":"{BOB}","domain":"{DOMAIN}"}},
             "from_client":"bobdev01",
             "data":"{}","time":"{time}"}}"#,
        b64(tls)
    )
}

fn mls_event_without_time(tls: &[u8]) -> String {
    format!(
        r#"{{"type":"conversation.mls-message-add",
             "qualified_conversation":{{"id":"{ROOM_UUID}","domain":"{DOMAIN}"}},
             "qualified_from":{{"id":"{BOB}","domain":"{DOMAIN}"}},
             "from_client":"bobdev01",
             "data":"{}"}}"#,
        b64(tls)
    )
}

fn rename_event_at(time: &str) -> String {
    format!(
        r#"{{"type":"conversation.rename","data":{{"name":"Renamed"}},"time":"{time}",
             "qualified_conversation":{{"id":"{ROOM_UUID}","domain":"{DOMAIN}"}},
             "qualified_from":{{"id":"{BOB}","domain":"{DOMAIN}"}},"via":"user"}}"#
    )
}

/// `GenericMessage { message_id = 1, knock = 4 { hot_knock = 1 } }`.
fn knock_message(message_id: &str) -> Vec<u8> {
    let mut knock = Vec::new();
    put_varint(1 << 3, &mut knock);
    put_varint(0, &mut knock);
    let mut m = Vec::new();
    put_len_delimited(1, message_id.as_bytes(), &mut m);
    put_len_delimited(4, &knock, &mut m);
    m
}

/// `GenericMessage { message_id = 1, ephemeral = 18 { expire_after_millis = 1,
/// text = 2 { content = 1 } } }`.
fn ephemeral_message(message_id: &str, content: &str) -> Vec<u8> {
    let mut text = Vec::new();
    put_len_delimited(1, content.as_bytes(), &mut text);
    let mut ephemeral = Vec::new();
    put_varint(1 << 3, &mut ephemeral);
    put_varint(60_000, &mut ephemeral);
    put_len_delimited(2, &text, &mut ephemeral);
    let mut m = Vec::new();
    put_len_delimited(1, message_id.as_bytes(), &mut m);
    put_len_delimited(18, &ephemeral, &mut m);
    m
}

/// `GenericMessage { message_id = 1, asset = 11 { original = 1 { mime_type = 1,
/// size = 2 }, uploaded = 4 { otr_key = 1, sha256 = 2, asset_id = 3 } } }`.
fn asset_message(message_id: &str) -> Vec<u8> {
    let mut original = Vec::new();
    put_len_delimited(1, b"text/plain", &mut original);
    put_varint(2 << 3, &mut original);
    put_varint(4, &mut original);
    let mut uploaded = Vec::new();
    put_len_delimited(1, &[7_u8; 32], &mut uploaded);
    put_len_delimited(2, &[8_u8; 32], &mut uploaded);
    put_len_delimited(3, b"3-1-asset", &mut uploaded);
    let mut asset = Vec::new();
    put_len_delimited(1, &original, &mut asset);
    put_len_delimited(4, &uploaded, &mut asset);
    let mut m = Vec::new();
    put_len_delimited(1, message_id.as_bytes(), &mut m);
    put_len_delimited(11, &asset, &mut m);
    m
}

fn next_page(server: &MockServer, since: &'static str, id: &'static str, events: Vec<String>) {
    mount_page(server, Some(since), &[Notif { id, events }]);
    mount_page(server, Some(id), &[]);
}

fn now_ms() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("after the epoch")
            .as_millis(),
    )
    .expect("fits")
}

async fn items(fx: &Fixture) -> Vec<TimelineItem> {
    fx.client
        .messages(&room_id(), &MessagesOptions::all())
        .await
        .expect("messages")
}

async fn item(fx: &Fixture, id: &str) -> TimelineItem {
    items(fx)
        .await
        .into_iter()
        .find(|i| i.item_id == id)
        .expect(id)
}

fn hex(bytes: &str) -> Vec<u8> {
    (0..bytes.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&bytes[i..i + 2], 16).expect("hex"))
        .collect()
}

fn holds(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

// ---- inbound ----

#[tokio::test]
async fn every_inbound_row_carries_the_events_time() {
    let server = MockServer::start();
    let mut fx = joined_client(&server).await;
    let before = now_ms();
    let messages = [
        ("text", text_message("text", "a text")),
        ("knock", knock_message("knock")),
        ("ephemeral", ephemeral_message("ephemeral", "gone soon")),
        ("asset", asset_message("asset")),
    ];
    let events = messages
        .iter()
        .map(|(_, m)| mls_event_at(&fx.bob.application_message(m), EVENT_TIME))
        .collect();
    next_page(&server, "n1", "n2", events);
    drain(&fx.client).await;
    let after = now_ms();

    for (id, _) in messages {
        let row = item(&fx, id).await;
        assert_eq!(row.sent_at_ms, EVENT_TIME_MS, "{id}: the backend's time");
        assert!(
            (before..=after).contains(&row.arrived_at_ms),
            "{id}: this device's clock"
        );
    }
}

#[derive(Default)]
struct Times(Mutex<Vec<(i64, i64)>>);

impl EventHandler for Times {
    fn on_message(&self, message: Message) {
        self.0
            .lock()
            .expect("times")
            .push((message.sent_at_ms, message.received_at_ms));
    }
    fn on_room_invited(&self, _room_id: String) {}
    fn on_sync_error(&self, _description: String) {}
}

#[tokio::test]
async fn on_message_carries_the_send_time_and_the_receive_time() {
    let server = MockServer::start();
    let mut fx = joined_client(&server).await;
    let times = Arc::new(Times::default());
    fx.client.add_event_handler(times.clone());
    let before = now_ms();
    let event = mls_event_at(
        &fx.bob.application_message(&text_message("m1", "first")),
        EVENT_TIME,
    );
    next_page(&server, "n1", "n2", vec![event]);
    drain(&fx.client).await;

    let seen = times.0.lock().expect("times").clone();

    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].0, EVENT_TIME_MS);
    assert!(seen[0].1 >= before, "received_at_ms keeps its meaning");
}

#[tokio::test]
async fn a_time_that_cannot_be_read_falls_back_to_this_devices_clock() {
    let server = MockServer::start();
    let mut fx = joined_client(&server).await;
    let before = now_ms();
    let garbled = mls_event_at(
        &fx.bob.application_message(&text_message("m1", "garbled")),
        "yesterday-ish",
    );
    let absent = mls_event_without_time(&fx.bob.application_message(&text_message("m2", "absent")));
    next_page(&server, "n1", "n2", vec![garbled, absent]);
    drain(&fx.client).await;

    for id in ["m1", "m2"] {
        let row = item(&fx, id).await;
        assert!(row.sent_at_ms >= before, "{id}");
        assert_eq!(row.sent_at_ms, row.arrived_at_ms, "{id}");
    }
    assert_eq!(fx.recorder.errors(), Vec::<String>::new());
}

// ---- the position ----

#[tokio::test]
async fn a_timeline_keeps_the_order_of_arrival_whatever_the_times_say() {
    let server = MockServer::start();
    let mut fx = joined_client(&server).await;
    let events = [("m1", FUTURE), ("m2", PAST), ("m3", EVENT_TIME)]
        .iter()
        .map(|(id, time)| mls_event_at(&fx.bob.application_message(&text_message(id, id)), time))
        .collect();
    next_page(&server, "n1", "n2", events);
    drain(&fx.client).await;

    let all = items(&fx).await;

    let order: Vec<&str> = all.iter().map(|i| i.item_id.as_str()).collect();
    assert_eq!(order, ["m1", "m2", "m3"]);
    assert!(
        all.windows(2)
            .all(|w| w[0].arrived_at_ms <= w[1].arrived_at_ms)
    );
}

/// A message sent long ago but delivered after the read: the position, not
/// the send time, decides.
#[tokio::test]
async fn a_message_sent_before_the_marker_but_arriving_after_it_is_unread() {
    let server = MockServer::start();
    let mut fx = joined_client(&server).await;
    let me = fx.client.user_id().expect("logged in").as_str().to_string();
    let first = mls_event_at(
        &fx.bob.application_message(&text_message("m1", "first")),
        EVENT_TIME,
    );
    mount_page(
        &server,
        Some("n1"),
        &[Notif {
            id: "n2",
            events: vec![first],
        }],
    );
    drain(&fx.client).await;
    let room = fx.client.room(&room_id()).await.expect("room");
    room.timeline()
        .expect("timeline")
        .mark_as_read("m1".into(), ReceiptType::ReadPrivate)
        .await
        .expect("mark_as_read");
    tokio::time::sleep(Duration::from_millis(5)).await;
    let late = mls_event_at(
        &fx.bob.application_message(&text_message("m2", "late")),
        PAST,
    );
    next_page(&server, "n2", "n3", vec![late]);
    drain(&fx.client).await;

    assert_eq!(
        item(&fx, "m2").await.sent_at_ms,
        1_577_836_800_000,
        "delivered"
    );
    assert_eq!(
        fx.store
            .unread_count_for_room(&room_id(), &me)
            .await
            .expect("count"),
        1
    );
}

/// A server clock ahead must not set the marker past the next message.
#[tokio::test]
async fn a_read_on_a_message_with_a_future_time_leaves_the_next_one_unread() {
    let server = MockServer::start();
    let mut fx = joined_client(&server).await;
    let me = fx.client.user_id().expect("logged in").as_str().to_string();
    let ahead = mls_event_at(
        &fx.bob.application_message(&text_message("m1", "ahead")),
        FUTURE,
    );
    mount_page(
        &server,
        Some("n1"),
        &[Notif {
            id: "n2",
            events: vec![ahead],
        }],
    );
    drain(&fx.client).await;
    let room = fx.client.room(&room_id()).await.expect("room");
    room.timeline()
        .expect("timeline")
        .mark_as_read("m1".into(), ReceiptType::ReadPrivate)
        .await
        .expect("mark_as_read");
    let marker = fx.store.read_marker(&room_id()).await.expect("marker");
    assert_eq!(marker, Some(item(&fx, "m1").await.arrived_at_ms));
    tokio::time::sleep(Duration::from_millis(5)).await;
    let next = mls_event_at(
        &fx.bob.application_message(&text_message("m2", "next")),
        EVENT_TIME,
    );
    next_page(&server, "n2", "n3", vec![next]);
    drain(&fx.client).await;

    assert_eq!(
        fx.store
            .unread_count_for_room(&room_id(), &me)
            .await
            .expect("count"),
        1
    );
}

// ---- the quote ----

#[tokio::test]
async fn a_reply_to_an_inbound_message_hashes_the_backends_time() {
    let server = MockServer::start();
    let uploads = mount_message_upload(&server);
    let mut fx = joined_client(&server).await;
    let parent = mls_event_at(
        &fx.bob.application_message(&text_message("m1", "first")),
        EVENT_TIME,
    );
    next_page(&server, "n1", "n2", vec![parent]);
    drain(&fx.client).await;
    let room = fx.client.room(&room_id()).await.expect("room");

    room.timeline()
        .expect("timeline")
        .send_reply("m1".into(), "a reply".into())
        .await
        .expect("send_reply");

    let sent = uploads.lock().expect("uploads")[0].clone();
    let plaintext = fx.bob.decrypt_application_message(&sent);
    assert!(
        holds(&plaintext, &hex(QUOTE_OF_INBOUND)),
        "the digest over the body and the event's time"
    );
}

#[tokio::test]
async fn a_reply_to_an_own_message_hashes_the_answers_time() {
    let server = MockServer::start();
    let uploads = mount_message_upload(&server);
    let mut fx = joined_client(&server).await;
    let room = fx.client.room(&room_id()).await.expect("room");
    let timeline = room.timeline().expect("timeline");
    let own = timeline.send_text("mine".into()).await.expect("send_text");

    timeline
        .send_reply(own.message_id, "a reply".into())
        .await
        .expect("send_reply");

    let sent = uploads.lock().expect("uploads");
    let _parent = fx.bob.decrypt_application_message(&sent[0]);
    let plaintext = fx.bob.decrypt_application_message(&sent[1]);
    assert!(holds(&plaintext, &hex(QUOTE_OF_OWN)));
}

// ---- own messages ----

#[tokio::test]
async fn every_own_send_takes_the_time_of_the_answer() {
    let server = MockServer::start();
    let _uploads = mount_message_upload(&server);
    let fx = joined_client(&server).await;
    let room = fx.client.room(&room_id()).await.expect("room");
    let timeline = room.timeline().expect("timeline");
    let before = now_ms();

    let handles = [
        ("text", timeline.send_text("a text".into()).await),
        ("knock", timeline.send_knock(false).await),
        (
            "location",
            timeline.send_location(52.5, 13.4, None, None).await,
        ),
        (
            "ephemeral",
            timeline
                .send_ephemeral_text("gone soon".into(), Duration::from_secs(60))
                .await,
        ),
    ];

    for (name, handle) in handles {
        let handle = handle.expect(name);
        assert_eq!(handle.sent_at_ms, ANSWER_TIME_MS, "{name}: the handle");
        let row = item(&fx, &handle.message_id).await;
        assert_eq!(row.sent_at_ms, ANSWER_TIME_MS, "{name}: the row");
        assert!(row.arrived_at_ms >= before, "{name}: this device's clock");
        assert_eq!(row.delivery_status, DeliveryStatus::Submitted, "{name}");
    }
}

#[tokio::test]
async fn an_own_file_takes_the_time_of_the_answer() {
    let server = MockServer::start();
    let _uploads = mount_message_upload(&server);
    server.mock(|when, then| {
        when.method(POST).path("/v14/assets");
        then.status(201)
            .header("content-type", "application/json")
            .body(r#"{"key":"3-1-asset","token":"token"}"#);
    });
    let fx = joined_client(&server).await;
    let room = fx.client.room(&room_id()).await.expect("room");
    let home = tempfile::tempdir().expect("tempdir");
    let path = home.path().join("note.txt");
    std::fs::write(&path, b"a file").expect("write");

    let handle = room
        .timeline()
        .expect("timeline")
        .send_file(&path, "text/plain")
        .await
        .expect("send_file");

    assert_eq!(handle.sent_at_ms, ANSWER_TIME_MS);
    assert_eq!(
        item(&fx, &handle.message_id).await.sent_at_ms,
        ANSWER_TIME_MS
    );
}

#[tokio::test]
async fn an_answer_without_a_readable_time_leaves_this_devices_clock() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/v14/mls/messages");
        then.status(201).body("");
    });
    let fx = joined_client(&server).await;
    let room = fx.client.room(&room_id()).await.expect("room");
    let before = now_ms();

    let handle = room
        .timeline()
        .expect("timeline")
        .send_text("a text".into())
        .await
        .expect("a sent message is sent, whatever the answer's body");

    assert!(handle.sent_at_ms >= before);
    assert_eq!(
        item(&fx, &handle.message_id).await.sent_at_ms,
        handle.sent_at_ms
    );
}

#[tokio::test]
async fn a_failed_send_stays_on_this_devices_clock_and_a_resend_takes_the_answers() {
    let server = MockServer::start();
    let mut refusal = server.mock(|when, then| {
        when.method(POST).path("/v14/mls/messages");
        then.status(500).body("{}");
    });
    let fx = joined_client(&server).await;
    let room = fx.client.room(&room_id()).await.expect("room");
    let timeline = room.timeline().expect("timeline");
    let before = now_ms();
    assert!(timeline.send_text("a text".into()).await.is_err());
    let failed = items(&fx).await.remove(0);
    assert_eq!(failed.delivery_status, DeliveryStatus::Failed);
    assert!(failed.sent_at_ms >= before);
    refusal.delete();
    let _uploads = mount_message_upload(&server);
    tokio::time::sleep(Duration::from_millis(5)).await;

    let handle = timeline
        .resend(failed.item_id.clone())
        .await
        .expect("resend");

    let resent = item(&fx, &failed.item_id).await;
    assert_eq!(handle.sent_at_ms, ANSWER_TIME_MS);
    assert_eq!(resent.sent_at_ms, ANSWER_TIME_MS);
    assert!(resent.arrived_at_ms > failed.arrived_at_ms, "to the tail");
}

// ---- room events (Step 145) ----

#[tokio::test]
async fn a_room_events_item_shows_the_events_time_and_stands_where_it_arrived() {
    let server = MockServer::start();
    let mut fx = joined_client(&server).await;
    let first = mls_event_at(
        &fx.bob.application_message(&text_message("m1", "first")),
        EVENT_TIME,
    );
    next_page(&server, "n1", "n2", vec![first, rename_event_at(PAST)]);
    drain(&fx.client).await;

    let all = items(&fx).await;

    assert_eq!(all.len(), 2);
    assert_eq!(all[0].item_id, "m1");
    assert!(matches!(
        all[1].kind,
        TimelineItemKind::RoomNameChange { .. }
    ));
    assert_eq!(all[1].sent_at_ms, 1_577_836_800_000, "the event's time");
    assert!(all[1].arrived_at_ms >= all[0].arrived_at_ms);
}

// ---- the surface ----

fn workspace_file(path: &str) -> String {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace");
    std::fs::read_to_string(root.join(path)).expect(path)
}

#[test]
fn the_ffi_carries_both_clocks() {
    let item = TimelineItem {
        item_id: "i".into(),
        room_id: "r@d".into(),
        sender_user_id: "u@d".into(),
        sender_device_id: String::new(),
        sent_at_ms: 1,
        arrived_at_ms: 2,
        kind: TimelineItemKind::Redacted,
        edited_at_ms: None,
        redacted: false,
        reactions: Vec::new(),
        read_by: Vec::new(),
        delivered_to: Vec::new(),
        delivery_status: DeliveryStatus::Received,
        expires_at_ms: None,
    };

    let ffi = kupferdraht::ffi::TimelineItem::from(&item);

    assert_eq!((ffi.sent_at_ms, ffi.arrived_at_ms), (1, 2));
}

#[test]
fn v35_keeps_every_rows_position() {
    let migration =
        workspace_file("kupferdraht-core/src/storage/migrations/V35__step_151_arrived_at.sql");
    assert!(migration.contains("ADD COLUMN arrived_at_ms"));
    assert!(migration.contains("SET arrived_at_ms = sent_at_ms"));
}

#[test]
fn the_browser_carries_both_clocks_and_its_types_say_so() {
    let live = workspace_file("kupferdraht-core/src/wasm/live_sync.rs");
    assert!(live.contains("sent_at_ms: m.sent_at_ms"), "onMessage");
    let manager = workspace_file("kupferdraht-core/src/wasm/manager.rs");
    assert!(
        manager.contains("arrived_at_ms: it.arrived_at_ms"),
        "messages()"
    );
    let types = workspace_file("bindings/web/kupferdraht-tabs/index.d.ts");
    assert!(types.contains("arrivedAtMs: number"));
    assert!(types.contains("sentAtMs: number; receivedAtMs: number"));
}

#[test]
fn the_docs_say_what_changed() {
    let changelog = workspace_file("CHANGELOG.md");
    assert!(changelog.contains("BREAKING: `sent_at_ms` is the backend's time"));
    let bindings = workspace_file("docs/bindings.md");
    assert!(bindings.contains("`arrivedAtMs`"));
    let threat_model = workspace_file("docs/THREAT_MODEL.md");
    assert!(threat_model.contains("can backdate"));
    assert!(
        !workspace_file("docs/library.md").contains("`sent_at_ms` is when this device processed"),
        "Step 145's sentence on the clock is gone"
    );
}
