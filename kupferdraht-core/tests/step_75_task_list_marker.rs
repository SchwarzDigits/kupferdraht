// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 75 — task-list room marker + invite state re-broadcast
//! (TDD red, ADR-0020).
//!
//! Send side: `Room::set_task_list` sends the full marker state
//! (`GenericMessage` field 262002, no fallback) from ANY member and
//! folds it locally into `Room::is_task_list()`. Receive side: marker
//! messages fold last-writer-wins in event order, from any member,
//! and redacted rows drop out. Invite hook: after the Add-commit the
//! inviter re-sends the union of the room's anchors — case snapshot
//! (`Room::case()` `Some`) and/or marker (`is_task_list`) — plus
//! every task's current folded state ONCE (never 2N), including done
//! tasks; best-effort with a warning on failure. Unmarked rooms that
//! merely contain a task keep ADR-0014's empty-on-join behaviour.
//!
//! Fixtures mirror `step_72b_case_fold.rs` / `step_72c_case_send.rs`:
//! the client is welcomed into a fixture peer's group; inbound
//! payloads are hand-emitted protobuf (`wire::proto` is
//! crate-private, ADR-0009); outbound re-broadcasts are counted as
//! `POST /v14/mls/messages` hits (the payload-shape contract lives in
//! `wire/proto.rs` unit tests — MLS ciphertext is opaque here).

use std::sync::{Arc, Mutex};

use base64::Engine as _;
use httpmock::Method::{GET, POST};
use httpmock::{Mock, MockServer, prelude::HttpMockRequest};
use kupferdraht::storage::Store;
use kupferdraht::timeline::{CasePatch, TaskDraft, TaskPatch, TaskStatus};
use kupferdraht::{ClientBuilder, SyncSettings};

mod common;

const ROOM_UUID: &str = "3d81cc72-9e44-4a01-b7a3-55e0d1c2ab90";
const DOMAIN: &str = "example.test";
/// b64("grp-marker")
const GROUP_ID_B64: &str = "Z3JwLW1hcmtlcg==";
const GROUP_ID: &[u8] = b"grp-marker";
const BOB: &str = "bob-uuid";
const BOB_IDENTITY: &[u8] = b"bob-uuid:bobdev01@example.test";
const DAVE: &str = "dave-uuid";

// ---- minimal protobuf writer (proto2 wire format, 72b precedent) ----

fn put_varint(mut v: u64, out: &mut Vec<u8>) {
    loop {
        let byte = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(byte);
            break;
        }
        out.push(byte | 0x80);
    }
}

fn put_tag(field: u32, wire_type: u8, out: &mut Vec<u8>) {
    put_varint((u64::from(field) << 3) | u64::from(wire_type), out);
}

fn put_len_delimited(field: u32, bytes: &[u8], out: &mut Vec<u8>) {
    put_tag(field, 2, out);
    put_varint(bytes.len() as u64, out);
    out.extend_from_slice(bytes);
}

fn put_uint(field: u32, value: u64, out: &mut Vec<u8>) {
    put_tag(field, 0, out);
    put_varint(value, out);
}

/// `GenericMessage { message_id = 1, task_list_marker = 262002 }` —
/// no content oneof (markers carry no text fallback, ADR-0020).
fn marker_generic_message(message_id: &str, marked: bool) -> Vec<u8> {
    let mut payload = Vec::new();
    put_uint(1, u64::from(marked), &mut payload);
    let mut m = Vec::new();
    put_len_delimited(1, message_id.as_bytes(), &mut m);
    put_len_delimited(262_002, &payload, &mut m);
    m
}

/// `GenericMessage { message_id = 1, task = 262000 }` with the
/// minimal required `TaskPayload { task_id = 1, title = 2,
/// status = 3 }` — mode a (no fallback).
fn task_generic_message(message_id: &str, task_id: &str, title: &str, status_raw: u64) -> Vec<u8> {
    let mut payload = Vec::new();
    put_len_delimited(1, task_id.as_bytes(), &mut payload);
    put_len_delimited(2, title.as_bytes(), &mut payload);
    put_uint(3, status_raw, &mut payload);
    let mut m = Vec::new();
    put_len_delimited(1, message_id.as_bytes(), &mut m);
    put_len_delimited(262_000, &payload, &mut m);
    m
}

/// `GenericMessage { message_id = 1, case = 262001 }` with the
/// minimal `CasePayload { revision = 1, case_ref = 2, title = 4,
/// state = 6 }` — the 72b core.
fn case_generic_message(message_id: &str, revision: u64, case_ref: &str, title: &str) -> Vec<u8> {
    let mut payload = Vec::new();
    put_uint(1, revision, &mut payload);
    put_len_delimited(2, case_ref.as_bytes(), &mut payload);
    put_len_delimited(4, title.as_bytes(), &mut payload);
    put_uint(6, 1 /* ACTIVE */, &mut payload);
    let mut m = Vec::new();
    put_len_delimited(1, message_id.as_bytes(), &mut m);
    put_len_delimited(262_001, &payload, &mut m);
    m
}

// ---- mocks (Step-69 shapes) ----

static UPLOADED_KPS: Mutex<Vec<(String, Vec<String>)>> = Mutex::new(Vec::new());

fn capture_key_package_upload(req: &HttpMockRequest) -> bool {
    let host = req
        .headers()
        .get("host")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
        .unwrap_or_default();
    let body = req.body_ref();
    if let Ok(v) = serde_json::from_slice::<serde_json::Value>(body)
        && let Some(arr) = v.get("key_packages").and_then(serde_json::Value::as_array)
    {
        let kps: Vec<String> = arr
            .iter()
            .filter_map(|s| s.as_str().map(String::from))
            .collect();
        UPLOADED_KPS.lock().expect("capture lock").push((host, kps));
    }
    true
}

fn captured_key_package_for(server: &MockServer) -> Vec<u8> {
    let host = server.address().to_string();
    let guard = UPLOADED_KPS.lock().expect("capture lock");
    // Most recent per host — httpmock pools servers (LEARNINGS §5.13).
    let (_, kps) = guard
        .iter()
        .rev()
        .find(|(h, _)| *h == host)
        .expect("register_device uploaded key packages");
    let first = kps.first().expect("at least one key package uploaded");
    base64::engine::general_purpose::STANDARD
        .decode(first)
        .expect("uploaded key package is base64")
}

/// Room metadata: `self_role` for the client, `bob_role` for the
/// fixture peer — both matter here (any-member marker vs. the case
/// fold's admin gate).
fn mount_room(server: &MockServer, self_role: &str, bob_role: &str) {
    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list-ids");
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(
                r#"{{"qualified_conversations":[
                     {{"id":"{ROOM_UUID}","domain":"{DOMAIN}"}}
                   ],"has_more":false,"paging_state":"END"}}"#
            ));
    });
    let self_role = self_role.to_string();
    let bob_role = bob_role.to_string();
    server.mock(move |when, then| {
        when.method(POST).path("/v14/conversations/list");
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(
                r#"{{"found":[{{
                     "qualified_id":{{"id":"{ROOM_UUID}","domain":"{DOMAIN}"}},
                     "type":0,"protocol":"mls","epoch":1,"group_id":"{GROUP_ID_B64}",
                     "name":"Markers",
                     "members":{{
                       "self":{{"qualified_id":{{"id":"user-uuid","domain":"{DOMAIN}"}},
                               "conversation_role":"{self_role}"}},
                       "others":[
                         {{"qualified_id":{{"id":"{BOB}","domain":"{DOMAIN}"}},
                          "conversation_role":"{bob_role}"}}
                       ]
                     }}
                   }}], "not_found":[], "failed":[]}}"#
            ));
    });
    server.mock(|when, then| {
        when.method(POST).path("/v14/list-users");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"found":[]}"#);
    });
}

fn mount_notifications_page(server: &MockServer, events: &[String]) {
    let payload = events.join(",");
    let body = format!(
        r#"{{"notifications":[{{"id":"9100e858-8eae-11ef-0000-0000075cafe0","payload":[{payload}]}}],"has_more":false}}"#
    );
    server.mock(move |when, then| {
        when.method(GET).path("/v14/notifications");
        then.status(200)
            .header("content-type", "application/json")
            .body(body.clone());
    });
}

fn welcome_event(data_b64: &str) -> String {
    format!(
        r#"{{"type":"conversation.mls-welcome",
             "qualified_conversation":{{"id":"{ROOM_UUID}","domain":"{DOMAIN}"}},
             "qualified_from":{{"id":"{BOB}","domain":"{DOMAIN}"}},
             "data":"{data_b64}",
             "time":"2026-08-25T10:00:00.000Z"}}"#
    )
}

fn message_event(data_b64: &str) -> String {
    format!(
        r#"{{"type":"conversation.mls-message-add",
             "qualified_conversation":{{"id":"{ROOM_UUID}","domain":"{DOMAIN}"}},
             "qualified_from":{{"id":"{BOB}","domain":"{DOMAIN}"}},
             "data":"{data_b64}",
             "time":"2026-08-25T10:00:01.000Z"}}"#
    )
}

fn mount_message_send(server: &MockServer) -> Mock<'_> {
    server.mock(|when, then| {
        when.method(POST).path("/v14/mls/messages");
        then.status(201)
            .header("content-type", "application/json")
            .body(r#"{"events":[],"time":"2026-08-25T10:00:02.000Z"}"#);
    })
}

fn mount_message_send_failing(server: &MockServer) -> Mock<'_> {
    server.mock(|when, then| {
        when.method(POST).path("/v14/mls/messages");
        then.status(500)
            .header("content-type", "application/json")
            .body(r#"{"label":"server-error","message":"boom"}"#);
    })
}

/// Claim + commit-bundle mocks for the invite of dave (72c shapes).
fn mount_invite_machinery(server: &MockServer) -> Mock<'_> {
    server.mock(|when, then| {
        when.method(POST)
            .path(format!("/v14/mls/key-packages/claim/{DOMAIN}/user-uuid"));
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"key_packages":[]}"#);
    });
    let dave_kp = common::mls::key_package_b64(b"dave-uuid:davedev01@example.test");
    server.mock(move |when, then| {
        when.method(POST)
            .path(format!("/v14/mls/key-packages/claim/{DOMAIN}/{DAVE}"));
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(
                r#"{{"key_packages":[{{"user":"{DAVE}","domain":"{DOMAIN}","client":"davedev01",
                     "key_package":"{dave_kp}","key_package_ref":"cmVm"}}]}}"#
            ));
    });
    server.mock(|when, then| {
        when.method(POST).path("/v14/mls/commit-bundles");
        then.status(201)
            .header("content-type", "application/json")
            .body(r#"{"events":[],"time":"2026-08-25T10:00:03.000Z"}"#);
    })
}

async fn logged_in_client(server: &MockServer) -> (Arc<kupferdraht::Client>, Arc<dyn Store>) {
    server.mock(|when, then| {
        when.method(POST)
            .path_includes("/v14/mls/key-packages/self/")
            .is_true(capture_key_package_upload);
        then.status(201);
    });
    common::mount_full_login_flow(server);
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().to_path_buf();
    let _ = Box::leak(Box::new(dir));
    let keystore: Arc<dyn kupferdraht::keystore::SecureKeyStore> =
        Arc::new(kupferdraht::keystore::InMemoryKeyStore::new());
    let client = ClientBuilder::new()
        .server(common::server_config(&server.base_url()))
        .permit_http()
        .base_dir(path)
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

/// Login + welcome into the fixture group. `extra_messages` are
/// peer-encrypted `GenericMessage` plaintexts delivered after the
/// welcome, in slice order (arrival order = fold event order).
async fn welcomed_client(
    server: &MockServer,
    self_role: &str,
    bob_role: &str,
    extra_messages: &[Vec<u8>],
) -> (
    Arc<kupferdraht::Client>,
    Arc<dyn Store>,
    Arc<kupferdraht::rooms::Room>,
) {
    let (client, store) = logged_in_client(server).await;
    mount_room(server, self_role, bob_role);
    let mut peer = common::mls::PeerGroup::new(GROUP_ID, BOB_IDENTITY);
    let b64 = |bytes: Vec<u8>| base64::engine::general_purpose::STANDARD.encode(bytes);
    let welcome = b64(peer.welcome_over_key_package(&captured_key_package_for(server)));
    let mut events = vec![welcome_event(&welcome)];
    for plaintext in extra_messages {
        events.push(message_event(&b64(peer.application_message(plaintext))));
    }
    mount_notifications_page(server, &events);
    client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync");
    let room = client
        .room(&format!("{ROOM_UUID}@{DOMAIN}"))
        .await
        .expect("room cached");
    (client, store, room)
}

fn draft(title: &str) -> TaskDraft {
    TaskDraft {
        title: title.into(),
        description: None,
        assignee: None,
        due_at_ms: None,
        priority: None,
    }
}

// ---- send side: any-member marker, local fold ----

#[tokio::test]
async fn set_task_list_toggles_and_folds_locally_for_any_member() {
    let server = MockServer::start();
    // Deliberately a plain member — the marker has NO admin gate
    // (ADR-0020 decision 1; contrast `set_case_rejects_non_admin_…`).
    let (_client, _store, room) = welcomed_client(&server, "wire_member", "wire_admin", &[]).await;
    let send = mount_message_send(&server);

    assert!(
        !room.is_task_list().await.expect("read"),
        "a room starts unmarked"
    );
    room.set_task_list(true).await.expect("any member marks");
    assert_eq!(send.calls(), 1, "one full marker state went out");
    assert!(
        room.is_task_list().await.expect("read"),
        "own sends fold locally"
    );

    room.set_task_list(false).await.expect("any member clears");
    assert_eq!(send.calls(), 2);
    assert!(
        !room.is_task_list().await.expect("read"),
        "clearing folds LWW in event order"
    );
}

// ---- receive side: LWW fold, persistence, redaction ----

#[tokio::test]
async fn inbound_markers_fold_lww_from_any_member() {
    let server = MockServer::start();
    // bob is a plain member — his marker must fold anyway.
    let (_client, store, room) = welcomed_client(
        &server,
        "wire_admin",
        "wire_member",
        &[
            marker_generic_message("marker-m1", true),
            marker_generic_message("marker-m2", false),
            marker_generic_message("marker-m3", true),
        ],
    )
    .await;

    assert!(
        room.is_task_list().await.expect("read"),
        "last write wins in event order (true, false, true → true)"
    );
    // Persisted as history like case rows (the audit trail).
    let rows = store
        .load_timeline_events(&room.id())
        .await
        .expect("load rows");
    assert_eq!(
        rows.iter().filter(|r| r.kind == "task_list_marker").count(),
        3,
        "every marker message is persisted as a timeline row"
    );
    // Positive kind pin (Case precedent): every marker row surfaces
    // as `TimelineItemKind::TaskListMarker` — the audit trail of the
    // deliberately forgeable any-member flag stays visible; the
    // library never silently filters it, consumers decide.
    let items = room
        .messages(&kupferdraht::timeline::MessagesOptions {
            limit: None,
            before: None,
        })
        .await
        .expect("messages");
    let marker_flags: Vec<bool> = items
        .iter()
        .filter_map(|i| match i.kind {
            kupferdraht::timeline::TimelineItemKind::TaskListMarker { marked } => Some(marked),
            _ => None,
        })
        .collect();
    assert_eq!(
        marker_flags.len(),
        3,
        "every marker row surfaces as TimelineItemKind::TaskListMarker"
    );
    assert_eq!(
        marker_flags.iter().filter(|m| **m).count(),
        2,
        "the item carries the full marker state (true, false, true)"
    );
    assert!(
        !items
            .iter()
            .any(|i| matches!(i.kind, kupferdraht::timeline::TimelineItemKind::Text { .. })),
        "no marker row degrades to a Text item"
    );
}

#[tokio::test]
async fn redacting_the_marker_row_refolds_to_unmarked() {
    let server = MockServer::start();
    let (_client, store, room) = welcomed_client(
        &server,
        "wire_admin",
        "wire_member",
        &[marker_generic_message("marker-m1", true)],
    )
    .await;

    assert!(room.is_task_list().await.expect("read"), "marker folded");
    assert!(
        store
            .apply_message_delete(&room.id(), "marker-m1", 4_500)
            .await
            .expect("redact"),
        "redaction fixture must hit its row"
    );
    assert!(
        !room.is_task_list().await.expect("read"),
        "redacted marker rows drop out of the fold"
    );
}

/// Preservation pin (green from the start): ADR-0020 leans on
/// ADR-0014's LWW idempotence — a re-sent current task state must not
/// duplicate the fold, while still being real timeline traffic.
#[tokio::test]
async fn resending_a_task_state_is_a_fold_noop_but_real_timeline_traffic() {
    let server = MockServer::start();
    let (_client, store, room) = welcomed_client(
        &server,
        "wire_admin",
        "wire_member",
        &[
            task_generic_message("task-m1", "task-x", "Same board state", 0),
            task_generic_message("task-m2", "task-x", "Same board state", 0),
        ],
    )
    .await;

    let tasks = room.tasks().await.expect("tasks");
    assert_eq!(tasks.len(), 1, "the fold never duplicates a task_id");
    assert_eq!(tasks[0].item.title, "Same board state");
    let rows = store
        .load_timeline_events(&room.id())
        .await
        .expect("load rows");
    assert_eq!(
        rows.iter().filter(|r| r.kind == "task").count(),
        2,
        "both re-sends are persisted timeline rows (real traffic)"
    );
}

// ---- invite hook: union of anchors + task batch, once ----

#[tokio::test]
async fn invite_into_marked_room_rebroadcasts_marker_and_all_tasks() {
    let server = MockServer::start();
    // A plain member invites — the marker branch must land from ANY
    // inviter (ADR-0020: no §72-class hole for the task family).
    let (client, _store, room) = welcomed_client(&server, "wire_member", "wire_admin", &[]).await;
    let send = mount_message_send(&server);
    let commit = mount_invite_machinery(&server);

    room.set_task_list(true).await.expect("mark");
    let timeline = room.timeline().expect("timeline");
    timeline
        .send_task(draft("Task A"), Some(false))
        .await
        .expect("task a");
    timeline
        .send_task(draft("Task B"), Some(false))
        .await
        .expect("task b");
    let task_b = room
        .tasks()
        .await
        .expect("tasks")
        .into_iter()
        .find(|t| t.item.title == "Task B")
        .expect("task b folded");
    timeline
        .update_task(
            &task_b.item.task_id,
            TaskPatch {
                status: Some(TaskStatus::Done),
                ..TaskPatch::default()
            },
            Some(false),
        )
        .await
        .expect("done b");
    assert_eq!(send.calls(), 4, "marker + A + B + done(B)");

    client
        .invite_members(&room.id(), &[format!("{DAVE}@{DOMAIN}")])
        .await
        .expect("invite");
    assert_eq!(commit.calls(), 1, "the Add-commit went out");
    // Re-broadcast: marker + BOTH tasks — the done task too (ADR-0020
    // decision 3: all tasks, the board is an audit trail). Open-only
    // would leave hits at 6.
    assert_eq!(
        send.calls(),
        7,
        "invite re-sends marker + every task's folded state (4 + 3)"
    );
}

#[tokio::test]
async fn invite_into_case_room_rebroadcasts_case_and_tasks() {
    let server = MockServer::start();
    let (client, _store, room) = welcomed_client(&server, "wire_admin", "wire_member", &[]).await;
    let send = mount_message_send(&server);
    let commit = mount_invite_machinery(&server);

    room.set_case(CasePatch {
        title: Some("Fall X".into()),
        ..CasePatch::default()
    })
    .await
    .expect("set_case");
    room.timeline()
        .expect("timeline")
        .send_task(draft("Task A"), Some(false))
        .await
        .expect("task a");
    assert_eq!(send.calls(), 2);

    client
        .invite_members(&room.id(), &[format!("{DAVE}@{DOMAIN}")])
        .await
        .expect("invite");
    assert_eq!(commit.calls(), 1);
    // The case anchor alone triggers the task batch (ADR-0020
    // decision 2: EITHER anchor → every task once) — the room is not
    // marked, yet its board must reach the joiner.
    assert_eq!(
        send.calls(),
        4,
        "invite re-sends case snapshot + the task (2 + 2)"
    );
}

#[tokio::test]
async fn invite_into_case_and_marked_room_sends_the_board_once() {
    let server = MockServer::start();
    let (client, _store, room) = welcomed_client(&server, "wire_admin", "wire_member", &[]).await;
    let send = mount_message_send(&server);
    let commit = mount_invite_machinery(&server);

    room.set_case(CasePatch {
        title: Some("Case + Board".into()),
        ..CasePatch::default()
    })
    .await
    .expect("set_case");
    room.set_task_list(true).await.expect("mark");
    let timeline = room.timeline().expect("timeline");
    timeline
        .send_task(draft("Task A"), Some(false))
        .await
        .expect("task a");
    timeline
        .send_task(draft("Task B"), Some(false))
        .await
        .expect("task b");
    assert_eq!(send.calls(), 4, "case + marker + A + B");

    client
        .invite_members(&room.id(), &[format!("{DAVE}@{DOMAIN}")])
        .await
        .expect("invite");
    assert_eq!(commit.calls(), 1);
    // Union rule (ADR-0020 decision 2): case + marker + N tasks —
    // the task helper runs ONCE. A per-anchor helper would send 2N
    // tasks and land at 10.
    assert_eq!(
        send.calls(),
        8,
        "invite re-sends case + marker + each task once (4 + 4, never 2N)"
    );
}

/// Preservation pin (green from the start): no anchor, no batch —
/// ADR-0014's empty-on-join behaviour for plain rooms survives.
#[tokio::test]
async fn invite_into_unmarked_room_with_a_task_rebroadcasts_nothing() {
    let server = MockServer::start();
    let (client, _store, room) = welcomed_client(&server, "wire_member", "wire_admin", &[]).await;
    let send = mount_message_send(&server);
    let commit = mount_invite_machinery(&server);

    room.timeline()
        .expect("timeline")
        .send_task(draft("Delegated in a plain chat"), Some(true))
        .await
        .expect("task");
    assert_eq!(send.calls(), 1);

    client
        .invite_members(&room.id(), &[format!("{DAVE}@{DOMAIN}")])
        .await
        .expect("invite");
    assert_eq!(commit.calls(), 1);
    assert_eq!(
        send.calls(),
        1,
        "an unmarked, case-less room re-broadcasts nothing on invite"
    );
}

#[tokio::test]
async fn non_admin_inviter_still_sends_case_and_task_rebroadcast() {
    let server = MockServer::start();
    // The case + task arrived from bob (the admin); the NON-admin
    // self now invites. The hook is role-blind: case + task go out
    // (the joiner's fold discarding the non-admin case snapshot is
    // the accepted §72 residual, pinned receive-side in 72b).
    let (client, _store, room) = welcomed_client(
        &server,
        "wire_member",
        "wire_admin",
        &[
            case_generic_message("case-m1", 1, "case-1", "Fall von Bob"),
            task_generic_message("task-m1", "task-x", "Bobs Aufgabe", 0),
        ],
    )
    .await;
    let send = mount_message_send(&server);
    let commit = mount_invite_machinery(&server);

    assert!(
        room.case().await.expect("read").is_some(),
        "bob's admin snapshot folded"
    );
    assert_eq!(room.tasks().await.expect("tasks").len(), 1);

    client
        .invite_members(&room.id(), &[format!("{DAVE}@{DOMAIN}")])
        .await
        .expect("invite");
    assert_eq!(commit.calls(), 1);
    assert_eq!(
        send.calls(),
        2,
        "case snapshot + task state go out even from a non-admin inviter"
    );
}

#[tokio::test]
async fn invite_succeeds_when_the_rebroadcast_fails() {
    let server = MockServer::start();
    let (client, _store, room) = welcomed_client(
        &server,
        "wire_member",
        "wire_admin",
        &[
            marker_generic_message("marker-m1", true),
            task_generic_message("task-m1", "task-x", "Bobs Aufgabe", 0),
        ],
    )
    .await;
    let failing_send = mount_message_send_failing(&server);
    let commit = mount_invite_machinery(&server);

    // Best-effort (ADR-0020 decision 2, like 72c): membership
    // succeeded, a failed re-send only warns.
    client
        .invite_members(&room.id(), &[format!("{DAVE}@{DOMAIN}")])
        .await
        .expect("invite must succeed despite the failed re-broadcast");
    assert_eq!(commit.calls(), 1, "the Add-commit went out");
    assert_eq!(
        failing_send.calls(),
        2,
        "marker + task re-sends were ATTEMPTED (then tolerated)"
    );
}
