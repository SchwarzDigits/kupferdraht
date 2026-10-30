// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 95c — no panics on consumer input. TDD red.
//!
//! An argument the app passes in is runtime data (CLAUDE.md
//! "Assertions"): an empty credential, an empty or malformed id must be
//! refused as `InvalidInput` at the public entry point — never reach a
//! `debug_assert!` below it (which, over FFI, is a panic in the
//! consumer's process).

use std::sync::{Arc, Mutex};

use base64::Engine as _;
use httpmock::Method::{GET, POST};
use httpmock::{MockServer, prelude::HttpMockRequest};
use kupferdraht::keystore::{InMemoryKeyStore, SecureKeyStore};
use kupferdraht::timeline::TaskPatch;
use kupferdraht::{ClientBuilder, Kupferdraht, KupferdrahtError, SyncSettings};

mod common;

use common::mls::PeerGroup;

const DOMAIN: &str = "example.test";
const ROOM_UUID: &str = "95c0dd01-0000-4000-8000-000000000001";
/// b64("grp-95c-in") — the room's MLS group id.
const GROUP_ID_B64: &str = "Z3JwLTk1Yy1pbg==";
const GROUP_ID: &[u8] = b"grp-95c-in";

fn room_id() -> String {
    format!("{ROOM_UUID}@{DOMAIN}")
}

fn assert_invalid<T>(result: Result<T, KupferdrahtError>, what: &str) {
    match result {
        Err(KupferdrahtError::InvalidInput(_)) => {}
        Err(other) => panic!("{what}: expected InvalidInput, got {other:?}"),
        Ok(_) => panic!("{what}: expected InvalidInput, got Ok"),
    }
}

fn builder(server: &MockServer) -> ClientBuilder {
    let home = tempfile::tempdir().expect("tempdir");
    let path = home.path().to_path_buf();
    let _ = Box::leak(Box::new(home));
    let keystore: Arc<dyn SecureKeyStore> = Arc::new(InMemoryKeyStore::new());
    ClientBuilder::new()
        .server(common::server_config(&server.base_url()))
        .permit_http()
        .base_dir(path)
        .keystore(keystore)
}

fn manager() -> Kupferdraht {
    let home = tempfile::tempdir().expect("tempdir");
    let path = home.path().to_path_buf();
    let _ = Box::leak(Box::new(home));
    let keystore: Arc<dyn SecureKeyStore> = Arc::new(InMemoryKeyStore::new());
    Kupferdraht::open(path, keystore)
        .expect("open kupferdraht")
        .permit_http()
}

fn mount_register(server: &MockServer) {
    server.mock(|when, then| {
        when.method(POST).path("/register");
        then.status(201)
            .header("content-type", "application/json")
            .body(
                r#"{"qualified_id":{"id":"00000000-0000-0000-0000-000000000abc","domain":"example.test"},
                    "name":"Alice","accent_id":0,"status":"ephemeral","locale":"en"}"#,
            );
    });
}

/// Empty credentials at every account entry point — `ClientBuilder` and
/// the `Kupferdraht` manager alike.
#[tokio::test]
async fn empty_credentials_are_refused() {
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    mount_register(&server);
    let config = || common::server_config(&server.base_url());

    for (email, password) in [("", "pw"), ("alice@example.test", "")] {
        let what = format!("login({email:?}, {password:?})");
        assert_invalid(
            builder(&server)
                .login(email.into(), password.into(), None)
                .await,
            &what,
        );
        assert_invalid(
            manager()
                .login(config(), email.into(), password.into(), None, None)
                .await,
            &what,
        );
    }
    for (name, email, password) in [
        ("", "alice@example.test", "pw"),
        ("Alice", "", "pw"),
        ("Alice", "alice@example.test", ""),
    ] {
        let what = format!("register({name:?}, {email:?}, {password:?})");
        assert_invalid(
            builder(&server)
                .register(name.into(), email.into(), password.into())
                .await,
            &what,
        );
        assert_invalid(
            manager()
                .register(config(), name.into(), email.into(), password.into())
                .await,
            &what,
        );
    }
    for (email, code, password) in [
        ("", "123456", "pw"),
        ("alice@example.test", "", "pw"),
        ("alice@example.test", "123456", ""),
    ] {
        assert_invalid(
            builder(&server)
                .activate(email.into(), code.into(), password.into())
                .await,
            &format!("activate({email:?}, {code:?}, {password:?})"),
        );
    }
    let pending = builder(&server)
        .register("Alice".into(), "alice@example.test".into(), "pw".into())
        .await
        .expect("register");
    assert_invalid(pending.activate("").await, "PendingClient::activate(\"\")");
    let account = manager()
        .register(
            config(),
            "Alice".into(),
            "alice@example.test".into(),
            "pw".into(),
        )
        .await
        .expect("register");
    assert_invalid(account.activate("").await, "PendingAccount::activate(\"\")");
}

/// SSO: an empty cookie, an empty code, and a bare `wire-` prefix (which
/// strips to an empty `IdP` id).
#[tokio::test]
async fn empty_sso_arguments_are_refused() {
    let server = MockServer::start();
    common::mount_full_login_flow(&server);

    assert_invalid(
        builder(&server).finalize_sso("").await,
        "finalize_sso(\"\")",
    );
    for code in ["", "wire-"] {
        assert_invalid(
            builder(&server)
                .initiate_sso(code, "kupferdraht://ok", "kupferdraht://err")
                .await,
            &format!("initiate_sso({code:?})"),
        );
    }
}

/// `delete_device("")` on a pending login (`ClientBuilder` and manager).
#[tokio::test]
async fn an_empty_device_id_to_delete_is_refused() {
    let server = MockServer::start();
    common::mount_full_login_flow(&server);

    let pending = builder(&server)
        .login("alice@example.test".into(), "pw".into(), None)
        .await
        .expect("login");
    assert_invalid(
        pending.delete_device("").await,
        "PendingLogin::delete_device",
    );
    let pending = manager()
        .login(
            common::server_config(&server.base_url()),
            "alice@example.test".into(),
            "pw".into(),
            None,
            None,
        )
        .await
        .expect("login");
    assert_invalid(pending.delete_device("").await, "manager delete_device");
}

// ---- a joined room (Welcome) for the room-level entry points ----

static UPLOADED_KPS: Mutex<Vec<(String, Vec<String>)>> = Mutex::new(Vec::new());

fn capture_key_package_upload(req: &HttpMockRequest) -> bool {
    let host = req
        .headers()
        .get("host")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
        .unwrap_or_default();
    if let Ok(v) = serde_json::from_slice::<serde_json::Value>(req.body_ref())
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
    let (_, kps) = guard
        .iter()
        .rev()
        .find(|(h, _)| *h == host)
        .expect("register_device uploaded key packages");
    base64::engine::general_purpose::STANDARD
        .decode(kps.first().expect("a key package"))
        .expect("uploaded key package is base64")
}

fn mount_joined_room(server: &MockServer, welcome: &[u8]) {
    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list-ids");
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(
                r#"{{"qualified_conversations":[{{"id":"{ROOM_UUID}","domain":"{DOMAIN}"}}],
                    "has_more":false,"paging_state":"END"}}"#
            ));
    });
    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list");
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(
                r#"{{"found":[{{
                     "qualified_id":{{"id":"{ROOM_UUID}","domain":"{DOMAIN}"}},
                     "type":0,"protocol":"mls","epoch":1,"group_id":"{GROUP_ID_B64}",
                     "name":"Inputs",
                     "members":{{
                       "self":{{"qualified_id":{{"id":"user-uuid","domain":"{DOMAIN}"}},
                               "conversation_role":"wire_admin"}},
                       "others":[]
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
    let body = format!(
        r#"{{"notifications":[{{"id":"95c0e858-8eae-11ef-0000-0000000000dd","payload":[{{
             "type":"conversation.mls-welcome",
             "qualified_conversation":{{"id":"{ROOM_UUID}","domain":"{DOMAIN}"}},
             "qualified_from":{{"id":"bob-uuid","domain":"{DOMAIN}"}},
             "data":"{}","time":"2026-09-26T10:00:00.000Z"}}]}}],"has_more":false}}"#,
        base64::engine::general_purpose::STANDARD.encode(welcome)
    );
    server.mock(move |when, then| {
        when.method(GET).path("/v14/notifications");
        then.status(200)
            .header("content-type", "application/json")
            .body(body.clone());
    });
}

/// Room-level entry points: malformed invitee ids, an empty typing room,
/// an unqualified join-link room id, an empty task id.
#[tokio::test]
async fn malformed_ids_at_room_entry_points_are_refused() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST)
            .path_includes("/v14/mls/key-packages/self/")
            .is_true(capture_key_package_upload);
        then.status(201);
    });
    common::mount_full_login_flow(&server);
    let (client, _store) = common::run_login(&server).await;
    let mut bob = PeerGroup::new(GROUP_ID, b"bob-uuid:bobdev01@example.test");
    let (_commit, welcome) = bob.add_member(&captured_key_package_for(&server));
    mount_joined_room(&server, &welcome);
    client
        .sync_once(SyncSettings::default())
        .await
        .expect("join via Welcome");

    for invitee in ["bob-uuid@", "@example.test"] {
        assert_invalid(
            client
                .invite_members(&room_id(), &[invitee.to_string()])
                .await,
            &format!("invite_members([{invitee:?}])"),
        );
    }
    assert_invalid(
        client.send_typing_started("").await,
        "send_typing_started(\"\")",
    );
    assert_invalid(
        client.send_typing_stopped("").await,
        "send_typing_stopped(\"\")",
    );
    let unqualified = format!("@{DOMAIN}");
    assert_invalid(
        client.create_join_link(&unqualified, None).await,
        "create_join_link(\"@domain\")",
    );
    assert_invalid(
        client.current_join_link(&unqualified).await,
        "current_join_link(\"@domain\")",
    );
    assert_invalid(
        client.revoke_join_link(&unqualified).await,
        "revoke_join_link(\"@domain\")",
    );
    let timeline = client
        .room(&room_id())
        .await
        .expect("room cached")
        .timeline()
        .expect("timeline");
    assert_invalid(
        timeline.update_task("", TaskPatch::default(), None).await,
        "update_task(\"\")",
    );
    assert_invalid(
        timeline.move_task("", None, None, None).await,
        "move_task(\"\")",
    );
}
