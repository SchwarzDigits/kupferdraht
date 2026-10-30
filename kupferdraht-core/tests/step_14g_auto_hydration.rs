// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 14g — auto-hydration convenience.
//!
//! TDD red-phase. Tests pin the contract from
//! `docs/plan/14-matrix-sdk-parity.md` §"TDD starter tests — 14g":
//!
//! * `Client::room_or_load(room_id: String) -> Option<Arc<Room>>` —
//!   cache-first; on miss, falls back to `load_rooms_from_store` and
//!   retries the cache lookup. Errors from the store are swallowed
//!   (returning `None`) so the surface matches `Client::room`'s
//!   `Option` shape. Closes
//!   `v1-api-ergonomics-audit.md §1.1` — `Client::room` returning
//!   `None` on cold-cache after `resume()`.
//!
//! * `Timeline::send_text_auto_join(body: String) -> Result<MessageHandle, _>`
//!   — **eager** auto-join: probes `MlsClient::group_exists` via the
//!   crate-internal `Client::has_mls_group` helper; on missing-group
//!   calls `Client::join_room` first, then `send_text`. The probe
//!   replaces the original "lazy + error-string match" plan because
//!   matching on `KupferdrahtError::InvalidInput(msg).contains(...)` is
//!   fragile — instead, a typed `KupferdrahtError::MlsGroupMissing`
//!   variant is introduced in `error.rs` (replacing
//!   `MlsClient::encrypt_text_message`'s previous
//!   `InvalidInput("...group not in storage...")` emit) so callers
//!   that want to handle the case explicitly can do so by typed
//!   match. Closes `v1-api-ergonomics-audit.md §1.2`.
//!
//! Three behaviour tests + one compile-pin cover the contract.
//! At red-phase both methods are stubbed via `todo!()` under their
//! green-phase signature so the file compiles; the behaviour tests
//! panic on first call. Green-phase replaces the bodies.

use std::sync::Arc;

use httpmock::Method::POST;
use httpmock::MockServer;
use kupferdraht::keystore::{InMemoryKeyStore, SecureKeyStore};
use kupferdraht::{ClientBuilder, SyncSettings};

mod common;

const ROOM_UUID: &str = "room-uuid";
const ROOM_DOMAIN: &str = "wire.com";

fn room_id_str() -> String {
    format!("{ROOM_UUID}@{ROOM_DOMAIN}")
}

/// Logged-in client sharing the supplied `base_dir` + keystore so a
/// second client instance can be brought up against the same
/// persistence and exhibit the "cold cache, warm store" condition Step
/// 14g's `room_or_load` is designed to hydrate.
async fn logged_in_client_sharing_persistence(
    server: &MockServer,
    base_dir: &std::path::Path,
    keystore: Arc<dyn SecureKeyStore>,
) -> Arc<kupferdraht::Client> {
    common::mount_full_login_flow(server);
    ClientBuilder::new()
        .server(common::server_config(&server.base_url()))
        .permit_http()
        .base_dir(base_dir.to_path_buf())
        .keystore(keystore)
        .login("alice@example.test".into(), "pw".into(), None)
        .await
        .expect("login")
        .register_device()
        .await
        .expect("register device")
}

/// Mounts the three sync endpoints so `sync_once` populates one
/// MLS group room keyed by `room-uuid@wire.com`. Includes the
/// `empty_notifications` cursor mock.
fn mount_sync_with_one_room(server: &MockServer) {
    common::mount_empty_notifications(server);
    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list-ids");
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(
                r#"{{"qualified_conversations":[
                     {{"id":"{ROOM_UUID}","domain":"{ROOM_DOMAIN}"}}
                   ],"has_more":false,"paging_state":"END"}}"#
            ));
    });
    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list");
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(
                r#"{{"found":[
                     {{
                       "qualified_id":{{"id":"{ROOM_UUID}","domain":"{ROOM_DOMAIN}"}},
                       "type":0,
                       "protocol":"mls",
                       "epoch":0,
                       "group_id":"Z3JwLWFh",
                       "name":"Engineering",
                       "members":{{
                         "self":{{"qualified_id":{{"id":"user-uuid","domain":"example.test"}}}},
                         "others":[]
                       }}
                     }}
                   ], "not_found":[], "failed":[]}}"#
            ));
    });
    server.mock(|when, then| {
        when.method(POST).path("/v14/list-users");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"found":[]}"#);
    });
}

// ---- 14g — Client::room_or_load --------------------------------

#[tokio::test]
async fn room_or_load_returns_cached_room_when_present() {
    // Happy path: cache hit returns immediately, no store fallback
    // needed. Identical to `Client::room`'s contract for the
    // already-warm case.
    let home = tempfile::tempdir().expect("home");
    let keystore: Arc<dyn SecureKeyStore> = Arc::new(InMemoryKeyStore::new());
    let server = MockServer::start();
    let client = logged_in_client_sharing_persistence(&server, home.path(), keystore.clone()).await;
    mount_sync_with_one_room(&server);

    client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync");

    let room = client
        .room_or_load(&room_id_str())
        .await
        .expect("room_or_load must return Some on cache hit");
    assert_eq!(room.id(), room_id_str());
    assert_eq!(room.name(), "Engineering");
}

#[tokio::test]
async fn room_or_load_hydrates_from_store_on_cache_miss() {
    // The cold-cache, warm-store case — the one §1.1 of the audit
    // calls out. A second `Client` instance shares the same
    // `SqliteStore` + keystore + storage_path as the first; the
    // first runs `sync_once` to populate persistence, the second
    // boots fresh (no `load_rooms_from_store` call) and must still
    // surface the room via `room_or_load` lazily hydrating from
    // the store.
    let home = tempfile::tempdir().expect("home");
    let keystore: Arc<dyn SecureKeyStore> = Arc::new(InMemoryKeyStore::new());
    let server = MockServer::start();

    let first_client =
        logged_in_client_sharing_persistence(&server, home.path(), keystore.clone()).await;
    mount_sync_with_one_room(&server);
    first_client
        .sync_once(SyncSettings::default())
        .await
        .expect("sync");
    drop(first_client);

    // Second client — fresh in-process state (empty rooms cache),
    // same persistence on disk.
    let fresh_client = logged_in_client_sharing_persistence(&server, home.path(), keystore).await;
    assert!(
        fresh_client.rooms().await.is_empty(),
        "second-client cache must start empty (no hydration yet)",
    );
    assert!(
        fresh_client.room(&room_id_str()).await.is_none(),
        "Client::room must report None on cold cache (existing contract — \
             this is exactly the §1.1 case room_or_load addresses)",
    );

    let room = fresh_client
        .room_or_load(&room_id_str())
        .await
        .expect("room_or_load must lazily hydrate from the SqliteStore");
    assert_eq!(room.id(), room_id_str());
    assert_eq!(room.name(), "Engineering");

    // After room_or_load, a subsequent Client::room call (strict)
    // also finds the room — the lazy hydrate is persistent across
    // the rest of the session.
    let strict_lookup = fresh_client
        .room(&room_id_str())
        .await
        .expect("post-hydrate Client::room must succeed");
    assert!(
        Arc::ptr_eq(&room, &strict_lookup),
        "room_or_load return and follow-up Client::room must yield the same Arc",
    );
}

#[tokio::test]
async fn room_or_load_returns_none_on_true_miss() {
    // No sync ran, store is empty — the room genuinely does not
    // exist. `room_or_load` must report None, NOT raise an error.
    let home = tempfile::tempdir().expect("home");
    let keystore: Arc<dyn SecureKeyStore> = Arc::new(InMemoryKeyStore::new());
    let server = MockServer::start();
    let client = logged_in_client_sharing_persistence(&server, home.path(), keystore).await;

    assert!(
        client.room_or_load(&room_id_str()).await.is_none(),
        "room_or_load on a fully empty cache+store must return None, not error",
    );
}

// ---- 14g — Timeline::send_text_auto_join compile-pin ------------

/// Type-level pin for `Timeline::send_text_auto_join(body: String) ->
/// Result<MessageHandle, KupferdrahtError>` and `KupferdrahtError::MlsGroupMissing`.
/// Never invoked — its presence forces the green-phase signature
/// and the new typed-error variant.
///
/// Behavioural coverage for the eager auto-join path
/// (`has_mls_group` probe → conditional `join_room` → `send_text`)
/// lives at live-smoke level: the full MLS / commit-bundles setup
/// needed to reach the "room cached but no MLS group" condition is
/// heavy (Alice + Bob signers, group_id alignment, /groupinfo +
/// commit-bundles mocks) and is already exercised by step_3 /
/// step_4's integration flows. The Step-14g code path is a thin
/// "probe → join → send" sequence over those — the type-pin
/// ensures the wrapper exists with the right shape; the eager
/// fallback is what live-smoke validates.
#[allow(dead_code)]
async fn _send_text_auto_join_signature_compile(timeline: Arc<kupferdraht::Timeline>) {
    let _: kupferdraht::MessageHandle = timeline
        .send_text_auto_join("hello".into())
        .await
        .expect("never reached");
    // Force the typed variant onto the public surface — green-phase
    // adds it to `error.rs`.
    let _: kupferdraht::KupferdrahtError = kupferdraht::KupferdrahtError::MlsGroupMissing;
}
