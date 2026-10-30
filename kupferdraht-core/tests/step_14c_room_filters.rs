// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 14c — filtered room-list accessors.
//!
//! TDD red-phase. Three integration tests pin the public-API
//! contract from `docs/plan/14-matrix-sdk-parity.md` §"TDD starter
//! tests — 14c":
//!
//! * `Client::joined_rooms()` returns the rooms the user is a
//!   member of. In Kupferdraht's current model every cached room is
//!   joined (sync_once only surfaces conversations the user is in),
//!   so `joined_rooms()` is semantically equivalent to `rooms()`.
//! * `Client::invited_rooms()` returns rooms the user has been
//!   invited to but has not yet joined. Kupferdraht does not materialize
//!   pending invitations as `Room` objects today — the method
//!   returns an empty `Vec` until that machinery lands (out-of-scope
//!   for 14c per the plan-doc).
//! * `Client::rooms_filtered<F>(F)` returns rooms matching the
//!   caller-supplied closure. This is the real value of 14c — it
//!   replaces ad-hoc filtering through `rooms()` for callers that
//!   want, say, "all groups" or "all DMs with a particular
//!   counterpart".
//!
//! Typed event handlers (the other half of the original 14c
//! scope) are deferred per the plan-doc's "open design question";
//! they land in a separate follow-up commit.

use std::sync::Arc;

use httpmock::Method::{GET, POST};
use httpmock::MockServer;
use kupferdraht::{Room, RoomType, SyncSettings};

mod common;

async fn logged_in_client(server: &MockServer) -> Arc<kupferdraht::Client> {
    common::mount_full_login_flow(server);
    let (client, _store) = common::run_login(server).await;
    client
}

fn conv_json(id: &str, conv_type: u8, name: &str) -> String {
    format!(
        r#"{{
            "qualified_id":{{"id":"{id}","domain":"example.test"}},
            "type":{conv_type},
            "protocol":"mls",
            "epoch":1,
            "group_id":"Z3JwLW9uZQ==",
            "name":"{name}",
            "members":{{
              "self":{{"qualified_id":{{"id":"user-uuid","domain":"example.test"}}}},
              "others":[]
            }}
        }}"#
    )
}

/// Mount conversation list-ids + list + empty notifications so a
/// single `sync_once` call surfaces the rooms in `rooms` and
/// drains an empty queue.
fn mount_sync_with_rooms(server: &MockServer, rooms: &[(&str, u8, &str)]) {
    let ids = rooms
        .iter()
        .map(|(id, _, _)| format!(r#"{{"id":"{id}","domain":"example.test"}}"#))
        .collect::<Vec<_>>()
        .join(",");
    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list-ids");
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(
                r#"{{"qualified_conversations":[{ids}],"has_more":false,"paging_state":"END"}}"#
            ));
    });
    let found = rooms
        .iter()
        .map(|(id, t, name)| conv_json(id, *t, name))
        .collect::<Vec<_>>()
        .join(",");
    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list");
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(
                r#"{{"found":[{found}],"not_found":[],"failed":[]}}"#
            ));
    });
    server.mock(|when, then| {
        when.method(GET).path("/v14/notifications");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"notifications":[],"has_more":false}"#);
    });
}

#[tokio::test]
async fn joined_rooms_returns_all_cached_rooms() {
    // Kupferdraht's current model: every room sync_once surfaces is a
    // room the user is a member of. `joined_rooms()` therefore
    // contains the same set as `rooms()`.
    let server = MockServer::start();
    let client = logged_in_client(&server).await;
    mount_sync_with_rooms(
        &server,
        &[("room-a", 0, "Group A"), ("room-b", 0, "Group B")],
    );
    client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync");

    let joined = client.joined_rooms();
    assert_eq!(joined.len(), 2, "both groups should appear in joined_rooms");
    let ids: Vec<_> = joined.iter().map(|r| r.id()).collect();
    assert!(ids.contains(&"room-a@example.test".to_string()));
    assert!(ids.contains(&"room-b@example.test".to_string()));
}

#[tokio::test]
async fn invited_rooms_is_empty_in_current_kupferdraht_model() {
    // Kupferdraht does not materialize pending invitations as `Room`
    // objects today — sync_once only surfaces conversations the
    // user is already a member of. `invited_rooms()` therefore
    // returns an empty Vec regardless of what Wire reports.
    // Future work (out of 14c's scope) lands a parallel cache for
    // invitations; at that point this test flips.
    let server = MockServer::start();
    let client = logged_in_client(&server).await;
    mount_sync_with_rooms(&server, &[("room-a", 0, "Group A")]);
    client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync");

    let invited = client.invited_rooms();
    assert!(
        invited.is_empty(),
        "invited_rooms must be empty until Kupferdraht materializes invitations \
         as Room objects (current limitation, out of 14c's scope)"
    );
}

#[tokio::test]
async fn rooms_filtered_with_room_type_predicate_excludes_non_matches() {
    // The closure-based filter is 14c's real value — it lets
    // callers project the cached rooms by any predicate without
    // open-coding the iteration.
    let server = MockServer::start();
    let client = logged_in_client(&server).await;
    mount_sync_with_rooms(
        &server,
        &[
            ("group-a", 0, "Group A"),      // RoomType::Group
            ("dm-a", 2, "DM A"),            // RoomType::DirectMessage
            ("group-b", 0, "Group B"),      // RoomType::Group
            ("self-a", 1, "Notes to Self"), // RoomType::SelfRoom
        ],
    );
    client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync");

    let groups: Vec<Arc<Room>> = client.rooms_filtered(|r| r.room_type() == RoomType::Group);
    assert_eq!(
        groups.len(),
        2,
        "rooms_filtered must return the two groups, not the DM / self-room"
    );
    for r in &groups {
        assert_eq!(r.room_type(), RoomType::Group);
    }
}
