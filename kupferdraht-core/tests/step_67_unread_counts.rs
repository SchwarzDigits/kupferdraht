// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 67 — unread counts per room (TDD red phase).
//!
//! Pins `docs/plan/67-unread-counts.md`: a device-local read marker
//! (`room_read_markers`), inbound-only counting with the honest
//! no-marker default (everything inbound counts), a marker that
//! advances even when `ReceiptMode::Off` suppresses the outbound
//! confirmation, and a grouped bulk query for room lists.

use httpmock::Method::{GET, POST};
use httpmock::MockServer;
use kupferdraht::storage::TimelineEventRecord;

mod common;

const SELF_USER: &str = "user-uuid@example.test";
const BOB: &str = "bob-uuid@example.test";

/// Login + `WIRE_RECEIPT_MODE = 0` (receipts off) so `mark_as_read`
/// short-circuits before the MLS-encrypted confirmation — the marker
/// persistence is exactly what must survive that gate.
fn mount_login_receipts_off(server: &MockServer) {
    common::mount_full_login_flow(server);
    server.mock(|when, then| {
        when.method(GET).path("/v14/feature-configs");
        then.status(200).body("{}");
    });
    server.mock(|when, then| {
        when.method(GET).path("/v14/properties");
        then.status(200).body(r#"["WIRE_RECEIPT_MODE"]"#);
    });
    server.mock(|when, then| {
        when.method(GET).path("/v14/properties/WIRE_RECEIPT_MODE");
        then.status(200).body("0");
    });
}

/// Two MLS rooms in the metadata mocks so `Room` handles exist.
fn mount_two_rooms(server: &MockServer) {
    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list-ids");
        then.status(200)
            .header("content-type", "application/json")
            .body(
                r#"{"qualified_conversations":[
                     {"id":"room-a","domain":"example.test"},
                     {"id":"room-b","domain":"example.test"}
                   ],"has_more":false,"paging_state":"END"}"#,
            );
    });
    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list");
        then.status(200)
            .header("content-type", "application/json")
            .body(
                r#"{"found":[
                     {"qualified_id":{"id":"room-a","domain":"example.test"},
                      "type":0,"protocol":"mls","epoch":1,"group_id":"Z3JwLWFh",
                      "name":"A","members":{"self":{"qualified_id":{"id":"user-uuid","domain":"example.test"}},"others":[]}},
                     {"qualified_id":{"id":"room-b","domain":"example.test"},
                      "type":0,"protocol":"mls","epoch":1,"group_id":"Z3JwLWJi",
                      "name":"B","members":{"self":{"qualified_id":{"id":"user-uuid","domain":"example.test"}},"others":[]}}
                   ], "not_found":[], "failed":[]}"#,
            );
    });
    server.mock(|when, then| {
        when.method(POST).path("/v14/list-users");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"found":[]}"#);
    });
}

fn inbound(room_id: &str, message_id: &str, sent_at_ms: i64) -> TimelineEventRecord {
    TimelineEventRecord {
        message_id: message_id.into(),
        room_id: room_id.into(),
        sender_user_id: BOB.into(),
        kind: "text".into(),
        body: format!("msg {message_id}"),
        sent_at_ms,
        delivery_status: "received".into(),
        ..TimelineEventRecord::default()
    }
}

#[tokio::test]
async fn no_marker_counts_all_inbound_excluding_own_and_redacted() {
    let server = MockServer::start();
    mount_login_receipts_off(&server);
    mount_two_rooms(&server);
    let (client, store) = common::run_login(&server).await;
    client.sync_metadata().await.expect("rooms cached");

    let room_id = "room-a@example.test";
    for (id, at) in [("m1", 1_000), ("m2", 2_000), ("m3", 3_000)] {
        store
            .insert_timeline_event(&inbound(room_id, id, at))
            .await
            .expect("seed inbound");
    }
    // Own message — never unread.
    store
        .insert_timeline_event(&TimelineEventRecord {
            sender_user_id: SELF_USER.into(),
            ..inbound(room_id, "m-own", 4_000)
        })
        .await
        .expect("seed own");
    // Redacted inbound — a tombstone is not unread content. (The
    // insert ignores `redacted_at_ms`; redaction is its own store op,
    // same as the production MessageDelete path.)
    store
        .insert_timeline_event(&inbound(room_id, "m-redacted", 4_200))
        .await
        .expect("seed redacted");
    assert!(
        store
            .apply_message_delete(room_id, "m-redacted", 4_500)
            .await
            .expect("redact"),
        "redaction fixture must hit its row"
    );

    let room = client.room(room_id).await.expect("room cached");
    assert_eq!(
        room.unread_count().await.expect("unread_count"),
        3,
        "no marker → every inbound counts; own + redacted excluded"
    );
}

#[tokio::test]
async fn mark_as_read_persists_the_marker_even_with_receipts_off() {
    let server = MockServer::start();
    mount_login_receipts_off(&server);
    mount_two_rooms(&server);
    let (client, store) = common::run_login(&server).await;
    client.sync_metadata().await.expect("rooms cached");

    let room_id = "room-a@example.test";
    for (id, at) in [("m1", 1_000), ("m2", 2_000), ("m3", 3_000)] {
        store
            .insert_timeline_event(&inbound(room_id, id, at))
            .await
            .expect("seed inbound");
    }

    let room = client.room(room_id).await.expect("room cached");
    let timeline = room.timeline().expect("timeline");
    let receipt = timeline
        .mark_as_read("m2".into())
        .await
        .expect("mark_as_read with receipts off");
    assert!(
        receipt.is_none(),
        "receipts off → no outbound confirmation (unchanged contract)"
    );
    assert_eq!(
        room.unread_count().await.expect("unread_count"),
        1,
        "the marker advanced to m2's sent_at — only m3 stays unread"
    );
}

#[tokio::test]
async fn bulk_unread_counts_group_per_room_and_omit_zero_rooms() {
    let server = MockServer::start();
    mount_login_receipts_off(&server);
    mount_two_rooms(&server);
    let (client, store) = common::run_login(&server).await;
    client.sync_metadata().await.expect("rooms cached");

    for (id, at) in [("a1", 1_000), ("a2", 2_000)] {
        store
            .insert_timeline_event(&inbound("room-a@example.test", id, at))
            .await
            .expect("seed a");
    }
    store
        .insert_timeline_event(&inbound("room-b@example.test", "b1", 1_500))
        .await
        .expect("seed b");
    // room-b fully read.
    store
        .set_read_marker("room-b@example.test", 1_500)
        .await
        .expect("marker b");

    let counts = client.unread_counts().await.expect("unread_counts");
    assert_eq!(counts.get("room-a@example.test"), Some(&2));
    assert_eq!(
        counts.get("room-b@example.test"),
        None,
        "zero-unread rooms are absent — consumers treat missing as 0"
    );
}
