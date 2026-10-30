// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 86 — self conversation: establish, join, send (TDD red phase).
//!
//! Pins `docs/plan/86-self-conversation.md` after the live probe of
//! 2026-09-24 against the own instance: Wire lists the user's MLS
//! self conversation (`type: 1`, `epoch: 0`, a `group_id`) in the
//! rooms snapshot but serves **no `GroupInfo`** for it until a member
//! has established the group — `GET …/groupinfo` answers
//! `404 mls-missing-group-info`, so an external commit is impossible
//! at epoch 0. The client therefore establishes the self room itself
//! (the Step-31 path: local group under the snapshot's `group_id`,
//! own other devices Added, or a self-update commit when solo) and
//! external-joins only once `epoch > 0`. Non-self rooms keep the
//! external-commit path unchanged.
//!
//! The "own devices" fan-out pin uses a real second device of the
//! same user (`common::mls::PeerDevice`): its key package is what the
//! self-claim returns, the client's Add-commit carries its Welcome,
//! and it decrypts the client's first message.

use std::sync::{Arc, Mutex};

use httpmock::Method::{GET, POST};
use httpmock::{Mock, MockServer, prelude::HttpMockRequest};
use kupferdraht::storage::Store;
use kupferdraht::{Client, RoomType, SyncSettings};
use openmls::prelude::WireFormat;

mod common;

const DOMAIN: &str = "example.test";
/// The (server-derived) id of the user's MLS self conversation.
const SELF_UUID: &str = "5f98a472-6364-50da-bf25-b77b0b2638e3";
/// b64("grp-self") — the self conversation's MLS group id.
const SELF_GROUP_ID_B64: &str = "Z3JwLXNlbGY=";
const SELF_GROUP_ID: &[u8] = b"grp-self";
/// A regular group conversation for the preservation test.
const GROUP_UUID: &str = "4b1ff12e-52a2-4b58-9bf6-1b40e64ae7a2";
/// b64("grp-grp").
const GROUP_GROUP_ID_B64: &str = "Z3JwLWdycA==";
const GROUP_GROUP_ID: &[u8] = b"grp-grp";
/// Leaf identity of the user's *other* device (same uuid as the
/// login fixture's `user-uuid`, different client id).
const OWN_OTHER_DEVICE: &[u8] = b"user-uuid:dev2@example.test";
const PEER_IDENTITY: &[u8] = b"bob-uuid:bobdev01@example.test";

/// Request bodies captured by fn-pointer matchers, keyed by the
/// request's Host header (httpmock pools servers — take the most
/// recent entry per host, see `step_69_welcome_absorption.rs`).
static COMMIT_BUNDLES: Mutex<Vec<(String, Vec<u8>)>> = Mutex::new(Vec::new());
static MLS_MESSAGES: Mutex<Vec<(String, Vec<u8>)>> = Mutex::new(Vec::new());

fn host_of(req: &HttpMockRequest) -> String {
    req.headers()
        .get("host")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
        .unwrap_or_default()
}

fn capture_commit_bundle(req: &HttpMockRequest) -> bool {
    COMMIT_BUNDLES
        .lock()
        .expect("capture lock")
        .push((host_of(req), req.body_ref().to_vec()));
    true
}

fn capture_mls_message(req: &HttpMockRequest) -> bool {
    MLS_MESSAGES
        .lock()
        .expect("capture lock")
        .push((host_of(req), req.body_ref().to_vec()));
    true
}

fn latest_capture(store: &Mutex<Vec<(String, Vec<u8>)>>, server: &MockServer) -> Vec<u8> {
    let host = server.address().to_string();
    let guard = store.lock().expect("capture lock");
    guard
        .iter()
        .rev()
        .find(|(h, _)| *h == host)
        .map(|(_, body)| body.clone())
        .expect("a request body was captured for this server")
}

fn self_room_id() -> String {
    format!("{SELF_UUID}@{DOMAIN}")
}

/// `OwnConversation` JSON of the self conversation exactly as the
/// live snapshot shows it: `type: 1`, one member (self), no name.
fn self_conv_json(epoch: u64) -> String {
    format!(
        r#"{{
            "qualified_id":{{"id":"{SELF_UUID}","domain":"{DOMAIN}"}},
            "type":1,
            "protocol":"mls",
            "epoch":{epoch},
            "group_id":"{SELF_GROUP_ID_B64}",
            "name":null,
            "members":{{
              "self":{{"qualified_id":{{"id":"user-uuid","domain":"{DOMAIN}"}}}},
              "others":[]
            }}
        }}"#
    )
}

/// A regular group (type 0) the user was added to elsewhere.
fn group_conv_json(epoch: u64) -> String {
    format!(
        r#"{{
            "qualified_id":{{"id":"{GROUP_UUID}","domain":"{DOMAIN}"}},
            "type":0,
            "protocol":"mls",
            "epoch":{epoch},
            "group_id":"{GROUP_GROUP_ID_B64}",
            "name":"G",
            "members":{{
              "self":{{"qualified_id":{{"id":"user-uuid","domain":"{DOMAIN}"}}}},
              "others":[{{"qualified_id":{{"id":"bob-uuid","domain":"{DOMAIN}"}}}}]
            }}
        }}"#
    )
}

/// Rooms metadata listing the given conversations, plus the
/// list-users + empty-notifications mocks `sync_once` walks.
fn mount_rooms(server: &MockServer, convs: &[(&str, String)]) {
    let ids: Vec<String> = convs
        .iter()
        .map(|(uuid, _)| format!(r#"{{"id":"{uuid}","domain":"{DOMAIN}"}}"#))
        .collect();
    let ids_body = format!(
        r#"{{"qualified_conversations":[{}],"has_more":false,"paging_state":"END"}}"#,
        ids.join(",")
    );
    server.mock(move |when, then| {
        when.method(POST).path("/v14/conversations/list-ids");
        then.status(200)
            .header("content-type", "application/json")
            .body(ids_body.clone());
    });
    let found: Vec<String> = convs.iter().map(|(_, json)| json.clone()).collect();
    let list_body = format!(
        r#"{{"found":[{}], "not_found":[], "failed":[]}}"#,
        found.join(",")
    );
    server.mock(move |when, then| {
        when.method(POST).path("/v14/conversations/list");
        then.status(200)
            .header("content-type", "application/json")
            .body(list_body.clone());
    });
    server.mock(|when, then| {
        when.method(POST).path("/v14/list-users");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"found":[]}"#);
    });
    common::mount_empty_notifications(server);
}

/// Self-claim (the user's *other* devices' key packages).
fn mount_self_claim(server: &MockServer, entries_json: String) -> Mock<'_> {
    server.mock(move |when, then| {
        when.method(POST)
            .path(format!("/v14/mls/key-packages/claim/{DOMAIN}/user-uuid"));
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(r#"{{"key_packages":[{entries_json}]}}"#));
    })
}

fn own_device_kp_entry(b64: &str) -> String {
    format!(
        r#"{{"user":"user-uuid","domain":"{DOMAIN}","client":"dev2",
            "key_package":"{b64}","key_package_ref":"cmVm"}}"#
    )
}

/// The external-join surface answering exactly what the live probe
/// saw for a never-established self conversation.
fn mount_groupinfo_missing<'a>(server: &'a MockServer, uuid: &str) -> Mock<'a> {
    server.mock(|when, then| {
        when.method(GET)
            .path(format!("/v14/conversations/{DOMAIN}/{uuid}/groupinfo"));
        then.status(404)
            .header("content-type", "application/json")
            .body(
                r#"{"code":404,"label":"mls-missing-group-info","message":"The conversation has no group information"}"#,
            );
    })
}

fn mount_groupinfo<'a>(server: &'a MockServer, uuid: &str, bytes: Vec<u8>) -> Mock<'a> {
    server.mock(move |when, then| {
        when.method(GET)
            .path(format!("/v14/conversations/{DOMAIN}/{uuid}/groupinfo"));
        then.status(200)
            .header("content-type", "message/mls")
            .body(bytes.clone());
    })
}

fn mount_commit_bundle(server: &MockServer) -> Mock<'_> {
    server.mock(|when, then| {
        when.method(POST)
            .path("/v14/mls/commit-bundles")
            .is_true(capture_commit_bundle);
        then.status(201)
            .header("content-type", "application/json")
            .body(r#"{"events":[],"time":"2026-09-24T10:00:00.000Z"}"#);
    })
}

fn mount_failing_commit_bundle(server: &MockServer) -> Mock<'_> {
    server.mock(|when, then| {
        when.method(POST).path("/v14/mls/commit-bundles");
        then.status(409)
            .header("content-type", "application/json")
            .body(r#"{"code":409,"label":"mls-stale-message","message":"The conversation epoch in a message is too old"}"#);
    })
}

fn mount_message_send(server: &MockServer) -> Mock<'_> {
    server.mock(|when, then| {
        when.method(POST)
            .path("/v14/mls/messages")
            .is_true(capture_mls_message);
        then.status(201)
            .header("content-type", "application/json")
            .body(r#"{"events":[],"time":"2026-09-24T10:00:01.000Z"}"#);
    })
}

async fn logged_in_client(server: &MockServer) -> (Arc<Client>, Arc<dyn Store>) {
    common::mount_full_login_flow(server);
    common::run_login(server).await
}

async fn send_into_self_room(client: &Client, body: &str) {
    let room = client
        .room(&self_room_id())
        .await
        .expect("self room cached");
    room.timeline()
        .expect("timeline")
        .send_text(body.into())
        .await
        .expect("send into the self room");
}

#[tokio::test]
async fn join_all_rooms_establishes_the_epoch_zero_self_room_and_sends() {
    let server = MockServer::start();
    let (client, _store) = logged_in_client(&server).await;
    mount_rooms(&server, &[(SELF_UUID, self_conv_json(0))]);
    let claim = mount_self_claim(&server, String::new());
    let gi = mount_groupinfo_missing(&server, SELF_UUID);
    let commit = mount_commit_bundle(&server);
    let send = mount_message_send(&server);

    client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync");
    let summary = client.join_all_rooms().await.expect("join_all_rooms");
    assert_eq!(summary.attempted, 1);
    assert_eq!(
        summary.joined, 1,
        "the self room counts as joined once established; failures: {:?}",
        summary.failed
    );
    assert!(summary.failed.is_empty(), "{:?}", summary.failed);
    assert_eq!(claim.calls(), 1, "own other devices are claimed (skip_own)");
    assert_eq!(commit.calls(), 1, "one establishing commit bundle uploaded");
    assert_eq!(
        gi.calls(),
        0,
        "an epoch-0 self room must not be external-joined (Wire has no GroupInfo)"
    );
    let segments = common::mls::bundle_segments(&latest_capture(&COMMIT_BUNDLES, &server));
    assert_eq!(
        segments.len(),
        2,
        "solo device → self-update commit: commit || group_info, no Welcome"
    );
    assert_eq!(segments[0].0, WireFormat::PublicMessage);

    // The cache + store carry the established epoch (openmls is the
    // authority, Step 66), and the room is sendable right away.
    let self_room = client.self_room().expect("self_room after establish");
    assert_eq!(
        self_room.epoch(),
        1,
        "epoch 0 → 1 by the establishing commit"
    );
    send_into_self_room(&client, "note to self").await;
    assert_eq!(send.calls(), 1);

    // Idempotent: the local group exists now — no second upload.
    let again = client.join_all_rooms().await.expect("join_all_rooms again");
    assert_eq!(again.already_joined, 1);
    assert_eq!(again.joined, 0);
    assert_eq!(commit.calls(), 1, "no re-establish of a local group");
}

#[tokio::test]
async fn join_room_on_the_epoch_zero_self_room_establishes_instead_of_fetching_groupinfo() {
    // The exact live failure of the probe (`room-join` → 404
    // mls-missing-group-info) — `join_room` itself must branch, so
    // `Room::join`, `room-join`, `send_text_auto_join` all heal it.
    let server = MockServer::start();
    let (client, _store) = logged_in_client(&server).await;
    mount_rooms(&server, &[(SELF_UUID, self_conv_json(0))]);
    let _claim = mount_self_claim(&server, String::new());
    let gi = mount_groupinfo_missing(&server, SELF_UUID);
    let commit = mount_commit_bundle(&server);
    let send = mount_message_send(&server);

    client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync");
    client
        .join_room(&self_room_id())
        .await
        .expect("join_room establishes the epoch-0 self room");
    assert_eq!(gi.calls(), 0);
    assert_eq!(commit.calls(), 1);
    send_into_self_room(&client, "via room-join").await;
    assert_eq!(send.calls(), 1);
}

#[tokio::test]
async fn join_all_rooms_external_joins_an_established_self_room() {
    // Another own device established the self room first (epoch 3):
    // this device takes the unchanged external-commit path.
    let server = MockServer::start();
    let (client, _store) = logged_in_client(&server).await;
    mount_rooms(&server, &[(SELF_UUID, self_conv_json(3))]);
    let claim = mount_self_claim(&server, String::new());
    let gi = mount_groupinfo(
        &server,
        SELF_UUID,
        common::mls::group_info_tls(SELF_GROUP_ID, OWN_OTHER_DEVICE),
    );
    let commit = mount_commit_bundle(&server);
    let send = mount_message_send(&server);

    client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync");
    let summary = client.join_all_rooms().await.expect("join_all_rooms");
    assert_eq!(summary.joined, 1, "{:?}", summary.failed);
    assert_eq!(gi.calls(), 1, "epoch > 0 → external commit");
    assert_eq!(commit.calls(), 1);
    assert_eq!(claim.calls(), 0, "joining claims nothing");
    send_into_self_room(&client, "joined late").await;
    assert_eq!(send.calls(), 1);
}

#[tokio::test]
async fn rejected_establishing_commit_discards_the_local_group_so_the_retry_re_establishes() {
    // The stale-snapshot race: the cache says epoch 0 but Wire
    // rejects the bundle. A local group left behind would make the
    // next join_all_rooms report already_joined on a dead room —
    // the failure must discard it (Step-66 pattern).
    let server = MockServer::start();
    let (client, _store) = logged_in_client(&server).await;
    mount_rooms(&server, &[(SELF_UUID, self_conv_json(0))]);
    let claim = mount_self_claim(&server, String::new());
    let _gi = mount_groupinfo_missing(&server, SELF_UUID);
    let failing = mount_failing_commit_bundle(&server);

    client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync");
    let first = client
        .join_all_rooms()
        .await
        .expect("join_all_rooms tolerates per-room failures");
    assert_eq!(first.joined, 0);
    assert_eq!(first.failed.len(), 1, "{:?}", first.failed);
    // The HTTP layer types `mls-stale-message` as
    // `KupferdrahtError::MlsStaleMessage` (Step 37); its text is what the
    // per-room failure carries.
    assert!(
        first.failed[0].1.contains("epoch is stale"),
        "the rejection surfaces as the typed stale-epoch error: {}",
        first.failed[0].1
    );
    assert_eq!(failing.calls(), 1);

    let second = client.join_all_rooms().await.expect("join_all_rooms again");
    assert_eq!(
        second.already_joined, 0,
        "a rejected establish must not leave a group that masquerades as joined"
    );
    assert_eq!(second.failed.len(), 1);
    assert_eq!(failing.calls(), 2, "the retry re-establishes");
    assert_eq!(claim.calls(), 2, "the retry re-claims");
}

#[tokio::test]
async fn non_self_rooms_keep_the_external_commit_path() {
    // Preservation: a regular group at epoch 0 in the snapshot is
    // never self-established — it goes through /groupinfo as before
    // (a real epoch-0 group would 404 there; the mock serves a
    // GroupInfo so the unchanged path can be pinned end to end).
    let server = MockServer::start();
    let (client, _store) = logged_in_client(&server).await;
    mount_rooms(&server, &[(GROUP_UUID, group_conv_json(0))]);
    let claim = mount_self_claim(&server, String::new());
    let gi = mount_groupinfo(
        &server,
        GROUP_UUID,
        common::mls::group_info_tls(GROUP_GROUP_ID, PEER_IDENTITY),
    );
    let commit = mount_commit_bundle(&server);

    client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync");
    let summary = client.join_all_rooms().await.expect("join_all_rooms");
    assert_eq!(summary.joined, 1, "{:?}", summary.failed);
    assert_eq!(gi.calls(), 1, "non-self rooms still fetch GroupInfo");
    assert_eq!(commit.calls(), 1);
    assert_eq!(claim.calls(), 0, "non-self rooms never self-claim");
    assert!(
        client.self_room().is_none(),
        "no self room in this snapshot"
    );
}

#[tokio::test]
async fn establishing_adds_the_users_other_device_which_reads_the_first_message() {
    // The "own devices" fan-out pin: the self-claim returns the
    // user's second device; the establishing commit Adds it (Welcome
    // in the bundle); that device joins via the Welcome and decrypts
    // the very first message — nothing is lost to forward secrecy.
    let server = MockServer::start();
    let (client, _store) = logged_in_client(&server).await;
    mount_rooms(&server, &[(SELF_UUID, self_conv_json(0))]);
    let mut other_device = common::mls::PeerDevice::new(OWN_OTHER_DEVICE);
    let claim = mount_self_claim(
        &server,
        own_device_kp_entry(&other_device.key_package_b64()),
    );
    let gi = mount_groupinfo_missing(&server, SELF_UUID);
    let commit = mount_commit_bundle(&server);
    let send = mount_message_send(&server);

    client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync");
    let summary = client.join_all_rooms().await.expect("join_all_rooms");
    assert_eq!(summary.joined, 1, "{:?}", summary.failed);
    assert_eq!(claim.calls(), 1);
    assert_eq!(commit.calls(), 1);
    assert_eq!(gi.calls(), 0);

    let segments = common::mls::bundle_segments(&latest_capture(&COMMIT_BUNDLES, &server));
    assert_eq!(
        segments.len(),
        3,
        "Add-commit bundle: commit || group_info || welcome"
    );
    assert_eq!(segments[2].0, WireFormat::Welcome);
    other_device.join_via_welcome(&segments[2].1);
    assert_eq!(
        other_device.epoch(),
        1,
        "the Welcome lands the device at epoch 1"
    );
    assert_eq!(
        client.self_room().expect("self room").epoch(),
        1,
        "the client's snapshot agrees with the device"
    );

    send_into_self_room(&client, "hello other me").await;
    assert_eq!(send.calls(), 1);
    let plaintext =
        other_device.decrypt_application_message(&latest_capture(&MLS_MESSAGES, &server));
    assert!(
        plaintext
            .windows(b"hello other me".len())
            .any(|w| w == b"hello other me"),
        "the other device reads the first message sent into the self room"
    );
}

#[tokio::test]
async fn self_room_is_a_local_cache_lookup() {
    let server = MockServer::start();
    let (client, _store) = logged_in_client(&server).await;
    mount_rooms(
        &server,
        &[
            (GROUP_UUID, group_conv_json(2)),
            (SELF_UUID, self_conv_json(0)),
        ],
    );
    assert!(
        client.self_room().is_none(),
        "nothing cached before the first sync / hydrate"
    );
    client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync");
    // Sync method, no network: the snapshot is the source.
    let room = client
        .self_room()
        .expect("the self room is in the snapshot");
    assert_eq!(room.id(), self_room_id());
    assert_eq!(room.room_type(), RoomType::SelfRoom);
    assert_eq!(
        room.epoch(),
        0,
        "not established yet — lookup does not join"
    );
    let by_id = client.room(&self_room_id()).await.expect("room by id");
    assert!(
        Arc::ptr_eq(&room, &by_id),
        "self_room and room(id) share the cache slot"
    );
}
