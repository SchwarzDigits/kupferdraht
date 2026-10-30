// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 14a — DM convenience: `Client::create_dm` + `Client::dm_room`
//! + `Client::dm_with`.
//!
//! TDD red-phase. The eight integration tests below pin the
//! public-API contract from `docs/plan/14-matrix-sdk-parity.md`
//! §"TDD starter tests — 14a" and CHANGELOG.md §42:
//!
//! * All three methods take a single `peer: &str` in qualified
//!   `<uuid>@<domain>` form. The HTTP layer parses via
//!   `split_once('@')` and emits Wire's native two-part
//!   `{ id, domain }` JSON.
//! * `create_dm` drives `GET /v14/one2one-conversations/{d}/{u}`
//!   (server-side idempotent: creates the 1:1 on the first call
//!   against a fresh `(self, peer)` pair, returns the existing one
//!   on subsequent calls), then `sync_metadata`, then returns the
//!   cached `Arc<Room>`. Wire production does not expose the
//!   swagger-listed POST `/one2one-conversations` route — recorded
//!   in `docs/wire/api-notes.md`. Since Step 68 the fetch is only
//!   the reservation: this file's fixtures report `epoch: 1`, so
//!   `create_dm` additionally external-joins the established group
//!   (`mount_external_join`); the establishment branches themselves
//!   are pinned by `step_68_dm_establishment.rs`.
//! * `dm_room` is a strict local-cache projection — no network
//!   call. Filters by `RoomType::DirectMessage` AND counterpart
//!   match.
//! * `dm_with` is the lookup-or-create convenience: cache hit →
//!   return without HTTP; cache miss → fall through to `create_dm`
//!   exactly once; two concurrent callers de-duplicate the POST.
//!
//! All 7 feature tests panic via `todo!()` at red-phase; the
//! `create_dm_rejects_unqualified_peer` input-guard test passes
//! because the stub already returns `InvalidInput` for an
//! `@`-less peer string (matches the documented `Err` shape).
//! Step 14a green-phase implements the three methods + the
//! `dm_with` in-flight dedup.

use std::sync::Arc;
use std::time::Duration;

use httpmock::Method::{GET, POST};
use httpmock::MockServer;
use kupferdraht::{RoomType, SyncSettings};
use tokio::time::timeout;

mod common;

async fn logged_in_client(server: &MockServer) -> Arc<kupferdraht::Client> {
    common::mount_full_login_flow(server);
    common::mount_empty_notifications(server);
    let (client, _store) = common::run_login(server).await;
    client
}

/// JSON for a single `OwnConversation` entry in
/// `POST /v14/conversations/list`'s `found` bucket. `conv_type`
/// is 0=Group, 2=DirectMessage. `others` is the list of non-self
/// members; for a 1:1 it carries exactly the peer.
fn conv_json(
    conv_id: &str,
    conv_domain: &str,
    conv_type: u8,
    name: &str,
    others: &[(&str, &str)],
) -> String {
    let others_json: Vec<String> = others
        .iter()
        .map(|(i, d)| format!(r#"{{"qualified_id":{{"id":"{i}","domain":"{d}"}}}}"#))
        .collect();
    let name_field = if name.is_empty() {
        r#""name":null"#.to_string()
    } else {
        format!(r#""name":"{name}""#)
    };
    format!(
        r#"{{
            "qualified_id":{{"id":"{conv_id}","domain":"{conv_domain}"}},
            "type":{conv_type},
            "protocol":"mls",
            "epoch":1,
            "group_id":"Z3JwLW9uZQ==",
            {name_field},
            "members":{{
              "self":{{"qualified_id":{{"id":"user-uuid","domain":"example.test"}}}},
              "others":[{}]
            }}
        }}"#,
        others_json.join(",")
    )
}

/// JSON for Wire's `MlsOne2OneConversation` envelope body —
/// the shape returned by `GET /v14/one2one-conversations/{d}/{u}`:
/// `{ conversation: OwnConversation, public_keys: MlsKeysByPurpose }`.
fn one2one_envelope(conv_id: &str, peer_uuid: &str) -> String {
    format!(
        r#"{{
            "conversation": {conv},
            "public_keys": {{
              "removal": {{
                "ed25519":"ZWQ=",
                "ecdsa_secp256r1_sha256":"cDI1Ng==",
                "ecdsa_secp384r1_sha384":"cDM4NA==",
                "ecdsa_secp521r1_sha512":"cDUyMQ=="
              }}
            }}
        }}"#,
        conv = conv_json(conv_id, "wire.com", 2, "", &[(peer_uuid, "wire.com")]),
    )
}

/// Mount the three sync-side mocks Wire would emit when a DM
/// conversation with `peer-uuid@wire.com` is the only room visible
/// to the caller. Hits land on list-ids → list → list-users in the
/// order `sync_metadata` walks them.
fn mount_sync_with_single_dm(server: &MockServer, conv_id: &str) {
    common::mount_empty_notifications(server);
    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list-ids");
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(
                r#"{{"qualified_conversations":[
                     {{"id":"{conv_id}","domain":"wire.com"}}
                   ],"has_more":false,"paging_state":"END"}}"#
            ));
    });
    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list");
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(
                r#"{{"found":[{}], "not_found":[], "failed":[]}}"#,
                conv_json(conv_id, "wire.com", 2, "", &[("peer-uuid", "wire.com")]),
            ));
    });
    server.mock(|when, then| {
        when.method(POST).path("/v14/list-users");
        then.status(200)
            .header("content-type", "application/json")
            .body(
                r#"{"found":[
                     {"qualified_id":{"id":"peer-uuid","domain":"wire.com"},
                      "name":"Peer Person","handle":"peerp"}
                   ]}"#,
            );
    });
}

/// Mount the two endpoints Step 68's `epoch > 0` branch drives:
/// `create_dm` now external-joins an already-established 1:1
/// (`conv_json` reports epoch 1) instead of only mirroring it
/// locally. The `GroupInfo` is a real openmls export under the
/// fixture group id `grp-one`.
fn mount_external_join(server: &MockServer) {
    let group_info = common::mls::group_info_tls(b"grp-one", b"peer-uuid:peerdev01@wire.com");
    server.mock(move |when, then| {
        when.method(GET)
            .path("/v14/conversations/wire.com/dm-uuid/groupinfo");
        then.status(200)
            .header("content-type", "message/mls")
            .body(group_info.clone());
    });
    server.mock(|when, then| {
        when.method(POST).path("/v14/mls/commit-bundles");
        then.status(201)
            .header("content-type", "application/json")
            .body(r#"{"events":[],"time":"2026-08-14T10:00:00.000Z"}"#);
    });
}

#[tokio::test]
async fn create_dm_drives_get_then_returns_room_for_peer() {
    let server = MockServer::start();
    let client = logged_in_client(&server).await;

    let get_one2one = server.mock(|when, then| {
        when.method(GET)
            .path("/v14/one2one-conversations/wire.com/peer-uuid")
            .header("authorization", "Bearer rebound.jwt.token");
        then.status(200)
            .header("content-type", "application/json")
            .body(one2one_envelope("dm-uuid", "peer-uuid"));
    });
    mount_sync_with_single_dm(&server, "dm-uuid");
    mount_external_join(&server);

    let room = client
        .create_dm("peer-uuid@wire.com")
        .await
        .expect("create_dm must succeed");
    assert_eq!(room.id(), "dm-uuid@wire.com");
    assert_eq!(room.room_type(), RoomType::DirectMessage);
    assert_eq!(
        room.counterpart().expect("counterpart").user_id,
        "peer-uuid@wire.com"
    );
    get_one2one.assert_calls(1);

    // After create_dm, dm_room returns the same conversation —
    // Arc identity holds because both share the cache slot.
    let looked_up = client
        .dm_room("peer-uuid@wire.com")
        .expect("dm_room must find the just-created DM");
    assert!(
        Arc::ptr_eq(&room, &looked_up),
        "create_dm and dm_room must yield the same Arc<Room>"
    );
}

#[tokio::test]
async fn create_dm_rejects_unqualified_peer() {
    let server = MockServer::start();
    let client = logged_in_client(&server).await;

    let err = client
        .create_dm("peer-uuid")
        .await
        .expect_err("unqualified peer (no @) must be rejected client-side");
    assert!(
        matches!(err, kupferdraht::KupferdrahtError::InvalidInput(_)),
        "expected InvalidInput, got {err:?}"
    );
    let msg = format!("{err}");
    assert!(
        msg.contains("peer must be qualified"),
        "error message must mention 'peer must be qualified', got: {msg}"
    );
}

#[tokio::test]
async fn dm_room_unknown_user_returns_none() {
    let server = MockServer::start();
    let client = logged_in_client(&server).await;
    // No rooms ever populated.
    assert!(
        client.dm_room("unknown-uuid@wire.com").is_none(),
        "dm_room must return None when no matching DM is cached"
    );
}

#[tokio::test]
async fn dm_room_ignores_group_room_with_peer_as_member() {
    // Defensive: a Group conversation that happens to include the
    // queried user must NOT be returned by `dm_room`. The lookup
    // is filtered both by RoomType::DirectMessage AND counterpart
    // match.
    let server = MockServer::start();
    let client = logged_in_client(&server).await;

    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list-ids");
        then.status(200)
            .header("content-type", "application/json")
            .body(
                r#"{"qualified_conversations":[
                     {"id":"group-uuid","domain":"wire.com"}
                   ],"has_more":false,"paging_state":"END"}"#,
            );
    });
    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list");
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(
                r#"{{"found":[{}], "not_found":[], "failed":[]}}"#,
                conv_json(
                    "group-uuid",
                    "wire.com",
                    0,
                    "Engineering",
                    &[("peer-uuid", "wire.com")]
                ),
            ));
    });
    server.mock(|when, then| {
        when.method(POST).path("/v14/list-users");
        then.status(200)
            .header("content-type", "application/json")
            .body(
                r#"{"found":[
                     {"qualified_id":{"id":"peer-uuid","domain":"wire.com"},
                      "name":"Peer Person","handle":"peerp"}
                   ]}"#,
            );
    });

    client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync");
    // Sanity-check that the group room IS in the cache.
    let rooms = client.rooms().await;
    assert_eq!(rooms.len(), 1);
    assert_eq!(rooms[0].room_type(), RoomType::Group);
    // But dm_room must NOT return it.
    assert!(
        client.dm_room("peer-uuid@wire.com").is_none(),
        "dm_room must NOT return a Group room even when the peer is a member"
    );
}

#[tokio::test]
async fn dm_with_returns_cached_room_without_http_call() {
    // Pre-populate the cache via sync_metadata — the DM already
    // exists on the server, so dm_with(peer) hits the cache and
    // returns without any GET traffic on /v14/one2one-conversations.
    // The endpoint is intentionally NOT mocked: any attempt to call
    // it surfaces as 404 (httpmock returns 404 for unmocked routes)
    // and fails the test.
    let server = MockServer::start();
    let client = logged_in_client(&server).await;

    mount_sync_with_single_dm(&server, "dm-uuid");
    client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync");
    let cached = client
        .dm_room("peer-uuid@wire.com")
        .expect("dm_room must find the pre-existing DM");

    let returned = client
        .dm_with("peer-uuid@wire.com")
        .await
        .expect("dm_with must succeed on cache hit");
    assert!(
        Arc::ptr_eq(&cached, &returned),
        "dm_with must return the same Arc<Room> that dm_room would on a cache hit"
    );
}

#[tokio::test]
async fn dm_with_falls_through_to_create_dm_on_cache_miss() {
    // Cache starts empty. dm_with(peer) calls dm_room (None) and
    // then create_dm (GET on the 1:1 endpoint exactly once).
    let server = MockServer::start();
    let client = logged_in_client(&server).await;

    let get_one2one = server.mock(|when, then| {
        when.method(GET)
            .path("/v14/one2one-conversations/wire.com/peer-uuid");
        then.status(200)
            .header("content-type", "application/json")
            .body(one2one_envelope("dm-uuid", "peer-uuid"));
    });
    mount_sync_with_single_dm(&server, "dm-uuid");
    mount_external_join(&server);

    let room = client
        .dm_with("peer-uuid@wire.com")
        .await
        .expect("dm_with must succeed via create_dm fallthrough");
    assert_eq!(room.id(), "dm-uuid@wire.com");
    assert_eq!(room.room_type(), RoomType::DirectMessage);
    get_one2one.assert_calls(1);
}

#[tokio::test]
async fn dm_with_dedupes_concurrent_calls() {
    // Two concurrent dm_with(peer) calls against the same peer
    // must call Wire only once — the second caller observes the
    // in-flight GET and receives the same Arc<Room>.
    let server = MockServer::start();
    let client = logged_in_client(&server).await;

    let get_one2one = server.mock(|when, then| {
        when.method(GET)
            .path("/v14/one2one-conversations/wire.com/peer-uuid");
        then.status(200)
            .header("content-type", "application/json")
            // 50 ms delay so the second caller is guaranteed to
            // start while the first GET is in flight.
            .delay(Duration::from_millis(50))
            .body(one2one_envelope("dm-uuid", "peer-uuid"));
    });
    mount_sync_with_single_dm(&server, "dm-uuid");
    mount_external_join(&server);

    let c1 = client.clone();
    let c2 = client.clone();
    let t1 = tokio::spawn(async move { c1.dm_with("peer-uuid@wire.com").await });
    let t2 = tokio::spawn(async move { c2.dm_with("peer-uuid@wire.com").await });

    // Cap the wait so a hung in-flight dedup surface manifests as
    // a test timeout rather than blocking the suite.
    let r1 = timeout(Duration::from_secs(5), t1)
        .await
        .expect("task 1 must finish")
        .expect("task 1 join")
        .expect("task 1 dm_with");
    let r2 = timeout(Duration::from_secs(5), t2)
        .await
        .expect("task 2 must finish")
        .expect("task 2 join")
        .expect("task 2 dm_with");

    assert!(
        Arc::ptr_eq(&r1, &r2),
        "concurrent dm_with calls must yield the same Arc<Room>"
    );
    get_one2one.assert_calls(1);
}
