// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 10f code-side — the `Timeline::mark_as_read` receipt-gate.
//!
//! ADR-0012 §Decision: when `Client::receipt_mode()` returns
//! `Off`, outbound `Confirmation { type: READ }` emission is
//! suppressed — other Wire clients respect the same setting, so
//! peers don't expect a receipt. The gate fires before
//! `send_read_confirmation` so no `/v14/mls/messages` POST happens.
//!
//! Live-smoke (toggle `WIRE_RECEIPT_MODE` via Wire-Web, observe a live
//! Wire v14 backend receiving zero receipts after toggle) is part of the
//! Step 10f live-smoke pass — the integration test below pins the
//! code-side behaviour.

use httpmock::Method::{GET, POST};
use httpmock::MockServer;
use kupferdraht::SyncSettings;

mod common;

const ROOM_UUID: &str = "dm-ab";
const ROOM_DOMAIN: &str = "example.test";

#[tokio::test]
async fn mark_as_read_returns_none_when_receipt_mode_off() {
    // Drives the full path: login → unlock → sync_metadata → room
    // lookup → mark_as_read. The gate fires before any MLS encrypt
    // / `/v14/mls/messages` POST. The test asserts:
    //   1. Return value is `Ok(None)` (gate fired).
    //   2. No `/v14/mls/messages` POST was issued (no mock for it;
    //      httpmock would return 404 → BackendRejected on accidental
    //      send, which would fail the test).
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    mount_receipt_mode_off(&server);
    mount_sync_metadata_with_one_room(&server);

    let (client, _) = common::run_login(&server).await;
    client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync_metadata");
    let qualified = format!("{ROOM_UUID}@{ROOM_DOMAIN}");
    let room = client
        .room(&qualified)
        .await
        .expect("room from sync_metadata");
    let timeline = room.timeline().expect("timeline");

    let result = timeline
        .mark_as_read("msg-id-under-test".into())
        .await
        .expect("mark_as_read should succeed even when gated");
    assert!(
        result.is_none(),
        "WIRE_RECEIPT_MODE=Off must short-circuit mark_as_read"
    );
}

fn mount_receipt_mode_off(server: &MockServer) {
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

fn mount_sync_metadata_with_one_room(server: &MockServer) {
    common::mount_empty_notifications(server);
    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list-ids");
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(
                r#"{{"qualified_conversations":[{{"id":"{ROOM_UUID}","domain":"{ROOM_DOMAIN}"}}],"has_more":false,"paging_state":"END"}}"#,
            ));
    });
    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list");
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(
                r#"{{"found":[{{
                    "qualified_id":{{"id":"{ROOM_UUID}","domain":"{ROOM_DOMAIN}"}},
                    "type":2,
                    "protocol":"mls",
                    "epoch":5,
                    "group_id":"Z3JwLWFh",
                    "name":"",
                    "members":{{
                        "self":{{"qualified_id":{{"id":"user-uuid","domain":"example.test"}}}},
                        "others":[{{"qualified_id":{{"id":"bob","domain":"example.test"}}}}]
                    }}
                }}], "not_found":[], "failed":[]}}"#,
            ));
    });
    server.mock(|when, then| {
        when.method(POST).path("/v14/list-users");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"found":[{"qualified_id":{"id":"bob","domain":"example.test"},"name":"Bob","handle":"b"}]}"#);
    });
}
