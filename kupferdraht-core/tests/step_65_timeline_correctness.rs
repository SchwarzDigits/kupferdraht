// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 65 — timeline correctness (TDD red phase, integration part).
//!
//! Pins `docs/plan/65-timeline-correctness.md` §Goal 1 and 5 (review
//! findings S4, S12 — `open.md` §66). The hydration overlays (S5,
//! S10) are pinned as unit tests on `TimelineItem::hydrated` in
//! `timeline/items.rs`; the `notify_message` guard (S6) has no cheap
//! test seam (inbound-text dispatch is MLS-decrypt-gated, the
//! gap-B.11 class) and is covered by review + the Step-62 smoke.

use httpmock::Method::POST;
use httpmock::MockServer;
use kupferdraht::storage::TimelineEventRecord;

mod common;

fn mount_one_group_room(server: &MockServer) {
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
                    "type":0,
                    "protocol":"mls",
                    "epoch":1,
                    "group_id":"Z3JwLW9uZQ==",
                    "name":"Step 65",
                    "members":{
                      "self":{"qualified_id":{"id":"user-uuid","domain":"example.test"}},
                      "others":[]
                    }
                }], "not_found":[], "failed":[]}"#,
            );
    });
}

#[tokio::test]
async fn send_reply_without_local_parent_is_invalid_input() {
    // Review S4: a missing parent silently dropped the wire-level
    // quote while still persisting reply_to_id locally — other Wire
    // clients rendered plain text, Kupferdraht rendered a reply. The
    // parent lookup must hard-fail before anything is built or sent.
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    mount_one_group_room(&server);
    let (client, _store) = common::run_login(&server).await;
    client.sync_metadata().await.expect("rooms cached");

    let timeline = client
        .room("room-a@example.test")
        .await
        .expect("room cached")
        .timeline()
        .expect("timeline");
    let err = timeline
        .send_reply("not-stored-anywhere".into(), "hi".into())
        .await
        .expect_err("a reply without its local parent must not be sent");
    assert!(
        matches!(&err, kupferdraht::KupferdrahtError::InvalidInput(msg) if msg.contains("parent")),
        "expected InvalidInput naming the missing parent, got {err:?}"
    );
}

#[tokio::test]
async fn mark_delivery_received_upgrades_a_submitted_row() {
    // Review S12: `Submitted` is documented as "awaiting the Wire
    // echo", but nothing ever upgraded it — group sends stayed
    // pending forever (1:1 is masked by delivery receipts). This is
    // the storage half of the fix; the dispatch half wires it into
    // the self-echo UNIQUE-conflict branch.
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    let (_client, store) = common::run_login(&server).await;

    store
        .insert_timeline_event(&TimelineEventRecord {
            message_id: "m-echo".into(),
            room_id: "room-a@example.test".into(),
            sender_user_id: "user-uuid@example.test".into(),
            kind: "text".into(),
            body: "own message".into(),
            delivery_status: "submitted".into(),
            ..TimelineEventRecord::default()
        })
        .await
        .expect("seed outbound row");

    let upgraded = store
        .mark_delivery_received("room-a@example.test", "m-echo")
        .await
        .expect("mark_delivery_received");
    assert!(upgraded, "a submitted row must upgrade on the echo");

    let row = store
        .load_timeline_event("m-echo")
        .await
        .expect("load")
        .expect("row exists");
    assert_eq!(row.delivery_status, "received");

    // Idempotence: a second echo (redelivery) is a no-op.
    let again = store
        .mark_delivery_received("room-a@example.test", "m-echo")
        .await
        .expect("second call");
    assert!(!again, "already-received rows must not report an upgrade");
}
