// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 152 — `Client::refresh_account`: the own account and team, read on
//! request (TDD red). `team.update` reaches only a team's admins and is not
//! stored (`docs/observations.md` §99), so reading is the way; the embedder
//! decides when.
#![cfg(not(target_arch = "wasm32"))]

use std::sync::Arc;

use httpmock::Method::{GET, POST, PUT};
use httpmock::{Mock, MockServer};
use kupferdraht::keystore::{InMemoryKeyStore, SecureKeyStore};
use kupferdraht::{Client, Kupferdraht, SyncSettings};
use tempfile::TempDir;

mod common;

const QUALIFIED: &str = "user-uuid@example.test";
const TEAM: &str = "6ba7b810-9dad-41d1-80b4-00c04fd430c8";

fn self_body(name: &str, handle: &str, team: Option<&str>, avatar: Option<&str>) -> String {
    let team = team.map_or(String::new(), |t| format!(r#","team":"{t}""#));
    let assets = avatar.map_or(String::new(), |key| {
        format!(
            r#","assets":[{{"key":"{key}","size":"preview","type":"image"}},{{"key":"{key}","size":"complete","type":"image"}}]"#
        )
    });
    format!(
        r#"{{"id":"user-uuid","qualified_id":{{"id":"user-uuid","domain":"example.test"}},"handle":"{handle}","name":"{name}","email":"{handle}@example.test"{team}{assets}}}"#
    )
}

fn team_body(name: &str, icon: &str) -> String {
    format!(
        r#"{{"binding":true,"creator":"creator-uuid","icon":"{icon}","id":"{TEAM}","name":"{name}","splash_screen":"default"}}"#
    )
}

fn mount_self<'a>(server: &'a MockServer, status: u16, body: &str) -> Mock<'a> {
    server.mock(|when, then| {
        when.method(GET).path("/v14/self");
        then.status(status)
            .header("content-type", "application/json")
            .body(body);
    })
}

fn mount_team<'a>(server: &'a MockServer, status: u16, body: &str) -> Mock<'a> {
    let path = format!("/v14/teams/{TEAM}");
    server.mock(|when, then| {
        when.method(GET).path(path);
        then.status(status)
            .header("content-type", "application/json")
            .body(body);
    })
}

/// `api-version` and `login`, without `/self`, which each test mounts itself.
fn mount_auth(server: &MockServer) {
    server.mock(|when, then| {
        when.method(GET).path("/api-version");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"supported":[14],"development":[]}"#);
    });
    server.mock(|when, then| {
        when.method(POST).path("/v14/login");
        then.status(200)
            .header("content-type", "application/json")
            .header("set-cookie", "zuid=test; HttpOnly; Secure; Path=/access")
            .body(r#"{"access_token":"initial.jwt.token","token_type":"Bearer","expires_in":900,"user":"user-uuid"}"#);
    });
}

struct Session {
    _home: TempDir,
    app: Kupferdraht,
    client: Arc<Client>,
}

async fn login(server: &MockServer) -> Session {
    mount_auth(server);
    common::login_mocks::mount_device_registration(server);
    let home = TempDir::new().expect("tempdir");
    let keystore: Arc<dyn SecureKeyStore> = Arc::new(InMemoryKeyStore::new());
    let app = Kupferdraht::open(home.path().to_path_buf(), keystore)
        .expect("open kupferdraht")
        .permit_http();
    let client = app
        .login(
            common::server_config(&server.base_url()),
            "alice@example.test".into(),
            "pw".into(),
            None,
            None,
        )
        .await
        .expect("login")
        .register_device()
        .await
        .expect("register device");
    Session {
        _home: home,
        app,
        client,
    }
}

/// What the account says, in one value to compare.
#[derive(Debug, PartialEq)]
struct Values {
    name: String,
    handle: Option<String>,
    email: Option<String>,
    team_id: Option<String>,
    team_name: Option<String>,
    team_icon: Option<String>,
    avatar: Option<String>,
}

fn values(client: &Client) -> Values {
    let account = client.account();
    Values {
        name: account.display_name().to_string(),
        handle: account.handle().map(str::to_string),
        email: account.email().map(str::to_string),
        team_id: account.team_id().map(str::to_string),
        team_name: account.team_name().map(str::to_string),
        team_icon: account.team_avatar_key().map(str::to_string),
        avatar: account.avatar_keys().thumbnail.clone(),
    }
}

fn some(value: &str) -> Option<String> {
    Some(value.to_string())
}

#[tokio::test]
async fn a_refresh_replaces_every_value_and_a_resume_keeps_them() {
    let server = MockServer::start();
    let mut me = mount_self(&server, 200, &self_body("Alice", "alice", Some(TEAM), None));
    let mut team = mount_team(&server, 200, &team_body("Team", "default"));
    let session = login(&server).await;
    me.delete();
    team.delete();
    mount_self(
        &server,
        200,
        &self_body("Alice B", "aliceb", Some(TEAM), Some("3-1-me")),
    );
    mount_team(&server, 200, &team_body("Team Renamed", "3-1-icon"));

    session
        .client
        .refresh_account()
        .await
        .expect("refresh_account");

    let expected = Values {
        name: "Alice B".to_string(),
        handle: some("aliceb"),
        email: some("aliceb@example.test"),
        team_id: some(TEAM),
        team_name: some("Team Renamed"),
        team_icon: some("3-1-icon"),
        avatar: some("3-1-me"),
    };
    assert_eq!(values(&session.client), expected);
    drop(session.client);
    let resumed = session.app.resume(QUALIFIED).await.expect("resume");
    assert_eq!(values(&resumed), expected, "the row holds them");
}

#[tokio::test]
async fn a_failed_self_request_changes_nothing() {
    let server = MockServer::start();
    let mut me = mount_self(&server, 200, &self_body("Alice", "alice", Some(TEAM), None));
    let _team = mount_team(&server, 200, &team_body("Team", "default"));
    let session = login(&server).await;
    let before = values(&session.client);
    me.delete();
    mount_self(&server, 500, "{}");

    assert!(session.client.refresh_account().await.is_err());

    assert_eq!(values(&session.client), before);
    drop(session.client);
    let resumed = session.app.resume(QUALIFIED).await.expect("resume");
    assert_eq!(values(&resumed), before);
}

/// All or nothing: a short failure must not erase a stored team name.
#[tokio::test]
async fn a_failed_team_request_changes_nothing_and_keeps_the_name() {
    let server = MockServer::start();
    let mut me = mount_self(&server, 200, &self_body("Alice", "alice", Some(TEAM), None));
    let mut team = mount_team(&server, 200, &team_body("Team", "default"));
    let session = login(&server).await;
    let before = values(&session.client);
    me.delete();
    team.delete();
    mount_self(
        &server,
        200,
        &self_body("Alice B", "aliceb", Some(TEAM), None),
    );
    mount_team(&server, 503, "{}");

    assert!(session.client.refresh_account().await.is_err());

    assert_eq!(values(&session.client), before);
}

#[tokio::test]
async fn a_self_of_another_user_is_refused() {
    let server = MockServer::start();
    let mut me = mount_self(&server, 200, &self_body("Alice", "alice", None, None));
    let session = login(&server).await;
    let before = values(&session.client);
    me.delete();
    mount_self(
        &server,
        200,
        r#"{"id":"eve-uuid","qualified_id":{"id":"eve-uuid","domain":"example.test"},"handle":"eve","name":"Eve"}"#,
    );

    assert!(session.client.refresh_account().await.is_err());

    assert_eq!(values(&session.client), before);
}

/// A user who left the team: the three team fields go, and no team is asked.
#[tokio::test]
async fn a_self_without_a_team_clears_the_team_and_asks_no_team() {
    let server = MockServer::start();
    let mut me = mount_self(&server, 200, &self_body("Alice", "alice", Some(TEAM), None));
    let team = mount_team(&server, 200, &team_body("Team", "3-1-icon"));
    let session = login(&server).await;
    me.delete();
    mount_self(&server, 200, &self_body("Alice", "alice", None, None));

    session
        .client
        .refresh_account()
        .await
        .expect("refresh_account");

    let account = session.client.account();
    assert_eq!(
        (
            account.team_id(),
            account.team_name(),
            account.team_avatar_key()
        ),
        (None, None, None)
    );
    team.assert_calls(1);
}

/// An empty icon is none, as `"default"` is at login.
#[tokio::test]
async fn an_empty_icon_is_none() {
    let server = MockServer::start();
    let _me = mount_self(&server, 200, &self_body("Alice", "alice", Some(TEAM), None));
    let mut team = mount_team(&server, 200, &team_body("Team", "3-1-icon"));
    let session = login(&server).await;
    team.delete();
    mount_team(&server, 200, &team_body("Team", ""));

    session
        .client
        .refresh_account()
        .await
        .expect("refresh_account");

    assert_eq!(session.client.account().team_avatar_key(), None);
}

/// A login whose team request failed has no name; the first refresh fills it.
#[tokio::test]
async fn a_name_the_login_could_not_fetch_is_filled() {
    let server = MockServer::start();
    let _me = mount_self(&server, 200, &self_body("Alice", "alice", Some(TEAM), None));
    let mut team = mount_team(&server, 500, "{}");
    let session = login(&server).await;
    assert_eq!(
        session.client.account().team_name(),
        None,
        "the login goes on"
    );
    team.delete();
    mount_team(&server, 200, &team_body("Team", "default"));

    session
        .client
        .refresh_account()
        .await
        .expect("refresh_account");

    assert_eq!(session.client.account().team_name(), Some("Team"));
}

/// The getters follow the client's own writes now, not only the row.
#[tokio::test]
async fn own_writes_reach_the_account() {
    let server = MockServer::start();
    let _me = mount_self(
        &server,
        200,
        &self_body("Alice", "alice", None, Some("3-1-me")),
    );
    let session = login(&server).await;
    server.mock(|when, then| {
        when.method(PUT).path("/v14/self");
        then.status(200);
    });
    server.mock(|when, then| {
        when.method(PUT).path("/v14/self/handle");
        then.status(200);
    });

    session
        .client
        .set_display_name("Alice B")
        .await
        .expect("name");
    session.client.set_handle("aliceb").await.expect("handle");
    session.client.remove_avatar().await.expect("remove_avatar");

    let account = session.client.account();
    assert_eq!(account.display_name(), "Alice B");
    assert_eq!(account.handle(), Some("aliceb"));
    assert_eq!(account.avatar_keys().thumbnail, None);
}

#[tokio::test]
async fn a_refused_own_write_leaves_the_account() {
    let server = MockServer::start();
    let _me = mount_self(&server, 200, &self_body("Alice", "alice", None, None));
    let session = login(&server).await;
    server.mock(|when, then| {
        when.method(PUT).path("/v14/self");
        then.status(500).body("{}");
    });

    assert!(session.client.set_display_name("Alice B").await.is_err());

    assert_eq!(session.client.account().display_name(), "Alice");
}

fn mount_rooms(server: &MockServer) -> Mock<'_> {
    let ids = server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list-ids");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"qualified_conversations":[],"has_more":false,"paging_state":"END"}"#);
    });
    server.mock(|when, then| {
        when.method(GET).path("/v14/notifications");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"notifications":[],"has_more":false}"#);
    });
    ids
}

/// A changed name or handle shows in the own `members()` row only after the
/// room list is read again, so a refresh that changed one marks it stale.
#[tokio::test]
async fn a_changed_profile_refreshes_the_room_list_and_an_unchanged_one_not() {
    let server = MockServer::start();
    let mut me = mount_self(&server, 200, &self_body("Alice", "alice", None, None));
    let session = login(&server).await;
    let list = mount_rooms(&server);
    session
        .client
        .sync_once(SyncSettings::default())
        .await
        .expect("first pass");
    list.assert_calls(1);

    session.client.refresh_account().await.expect("unchanged");
    session
        .client
        .sync_once(SyncSettings::default())
        .await
        .expect("pass");
    list.assert_calls(1);

    me.delete();
    mount_self(&server, 200, &self_body("Alice B", "alice", None, None));
    session.client.refresh_account().await.expect("changed");
    session
        .client
        .sync_once(SyncSettings::default())
        .await
        .expect("pass");
    list.assert_calls(2);
}

// ---- the surface ----

fn workspace_file(path: &str) -> String {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace");
    std::fs::read_to_string(root.join(path)).expect(path)
}

#[test]
fn every_surface_has_the_call() {
    assert!(
        workspace_file("kupferdraht-core/src/ffi/mod.rs").contains("pub async fn refresh_account")
    );
    assert!(
        workspace_file("kupferdraht-core/src/wasm/manager.rs").contains("js_name = refreshAccount")
    );
    let protocol = workspace_file("bindings/web/kupferdraht-tabs/protocol.js");
    assert!(protocol.contains(r#"["Client", "refreshAccount", "idempotent", "$getters"]"#));
    let types = workspace_file("bindings/web/kupferdraht-tabs/index.d.ts");
    assert!(types.contains("refreshAccount(): Promise<void>;"));
    assert!(workspace_file("kupferdraht-cli/src/main.rs").contains("AccountRefresh"));
}

#[test]
fn the_docs_say_how() {
    let bindings = workspace_file("docs/bindings.md");
    assert!(bindings.contains("refreshAccount()"));
    let changelog = workspace_file("CHANGELOG.md");
    assert!(changelog.contains("`Client::refresh_account`"));
    assert!(changelog.contains("`Account::avatar_keys`"));
}
