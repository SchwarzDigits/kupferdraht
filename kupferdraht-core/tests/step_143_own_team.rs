// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 143 — the own team on the account: id and name (TDD red).
//!
//! `GET /self` names the team by id and nothing fetched its name. A login now
//! asks for it once, an enrichment like the feature configs (ADR-0012): any
//! failure is a warning and the login stands, with the id and without the
//! name. `Account` is a snapshot, so the values come back from the
//! `self_user` row at `resume` and a profile write must not drop them.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use httpmock::Method::{GET, POST, PUT};
use httpmock::{Mock, MockServer};
use kupferdraht::keystore::{InMemoryKeyStore, SecureKeyStore};
use kupferdraht::{Client, Kupferdraht};
use tempfile::TempDir;

mod common;

const QUALIFIED: &str = "user-uuid@example.test";
const TEAM: &str = "6ba7b810-9dad-41d1-80b4-00c04fd430c8";
const TEAM_NAME: &str = "Fixture Team";

fn self_body(team: Option<&str>) -> String {
    let team = team.map_or(String::new(), |t| format!(r#","team":"{t}""#));
    format!(
        r#"{{"id":"user-uuid","qualified_id":{{"id":"user-uuid","domain":"example.test"}},"handle":"alice","name":"Alice Example","email":"alice@example.test"{team}}}"#
    )
}

/// The fields a live `GET /v14/teams/{tid}` answers with
/// (`docs/wire/api-notes.md`).
fn team_body() -> String {
    format!(
        r#"{{"binding":true,"creator":"creator-uuid","icon":"default","id":"{TEAM}","name":"{TEAM_NAME}","splash_screen":"default"}}"#
    )
}

/// `GET /v14/teams/{id}` answering `status` with `body`.
fn mount_team<'a>(server: &'a MockServer, id: &str, status: u16, body: &str) -> Mock<'a> {
    let path = format!("/v14/teams/{id}");
    server.mock(|when, then| {
        when.method(GET).path(path);
        then.status(status)
            .header("content-type", "application/json")
            .body(body);
    })
}

/// Any request for a team, to count that none was made.
fn mount_any_team(server: &MockServer) -> Mock<'_> {
    server.mock(|when, then| {
        when.path_includes("/teams/");
        then.status(500);
    })
}

struct Session {
    _home: TempDir,
    app: Kupferdraht,
    client: Arc<Client>,
}

async fn login(server: &MockServer, team: Option<&str>) -> Session {
    common::login_mocks::mount_login_with_self(server, &self_body(team));
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

#[tokio::test]
async fn a_team_member_gets_the_id_and_the_name() {
    let server = MockServer::start();
    let team = server.mock(|when, then| {
        when.method(GET)
            .path(format!("/v14/teams/{TEAM}"))
            .header("authorization", "Bearer rebound.jwt.token");
        then.status(200)
            .header("content-type", "application/json")
            .body(team_body());
    });

    let session = login(&server, Some(TEAM)).await;

    let account = session.client.account();
    assert_eq!(account.team_id(), Some(TEAM));
    assert_eq!(account.team_name(), Some(TEAM_NAME));
    team.assert_calls(1);
}

#[tokio::test]
async fn an_sso_login_gets_them_too() {
    let server = MockServer::start();
    common::login_mocks::mount_device_registration(&server);
    server.mock(|when, then| {
        when.method(GET).path("/api-version");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"supported":[14],"development":[]}"#);
    });
    // After the registration's `/access`, which asks for a `client_id`: the
    // first mock that matches answers, so this one takes the SSO cookie's.
    server.mock(|when, then| {
        when.method(POST).path("/v14/access").header("cookie", "zuid=test");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"access_token":"initial.jwt.token","token_type":"Bearer","expires_in":900,"user":"user-uuid"}"#);
    });
    server.mock(|when, then| {
        when.method(GET).path("/v14/self");
        then.status(200)
            .header("content-type", "application/json")
            .body(self_body(Some(TEAM)));
    });
    let team = mount_team(&server, TEAM, 200, &team_body());
    let home = TempDir::new().expect("tempdir");
    let keystore: Arc<dyn SecureKeyStore> = Arc::new(InMemoryKeyStore::new());
    let app = Kupferdraht::open(home.path().to_path_buf(), keystore)
        .expect("open kupferdraht")
        .permit_http();

    let client = app
        .finalize_sso(common::server_config(&server.base_url()), "test", None)
        .await
        .expect("finalize_sso")
        .register_device()
        .await
        .expect("register device");

    assert_eq!(client.account().team_id(), Some(TEAM));
    assert_eq!(client.account().team_name(), Some(TEAM_NAME));
    team.assert_calls(1);
}

#[tokio::test]
async fn a_personal_account_makes_no_team_request() {
    let server = MockServer::start();
    let any_team = mount_any_team(&server);

    let session = login(&server, None).await;

    let account = session.client.account();
    assert_eq!(account.team_id(), None);
    assert_eq!(account.team_name(), None);
    any_team.assert_calls(0);
}

#[tokio::test]
async fn an_empty_team_is_no_team() {
    let server = MockServer::start();
    let any_team = mount_any_team(&server);

    let session = login(&server, Some("")).await;

    let account = session.client.account();
    assert_eq!(account.team_id(), None);
    assert_eq!(account.team_name(), None);
    any_team.assert_calls(0);
}

/// `403`, `404`, `5xx` and an answer that does not decode: a warning, and the
/// login stands with the id alone. A transport failure takes the same path;
/// a mock cannot refuse one request of many.
#[tokio::test]
async fn a_failing_team_fetch_leaves_the_name_empty_and_the_login_standing() {
    let failures: [(u16, &str); 4] = [
        (
            403,
            r#"{"code":403,"label":"no-team-member","message":"no"}"#,
        ),
        (
            404,
            r#"{"code":404,"label":"no-team","message":"Team not found"}"#,
        ),
        (500, r#"{"code":500,"label":"server-error","message":"no"}"#),
        (200, "not json"),
    ];
    for (status, body) in failures {
        let server = MockServer::start();
        let team = mount_team(&server, TEAM, status, body);

        let session = login(&server, Some(TEAM)).await;

        let account = session.client.account();
        assert_eq!(
            account.team_id(),
            Some(TEAM),
            "status {status}: the id stays"
        );
        assert_eq!(account.team_name(), None, "status {status}: no name");
        team.assert_calls(1);
    }
}

/// `a/b`, not `../..`: a URL parser folds dots away, and the request would
/// slip past the mock that counts it.
#[tokio::test]
async fn a_team_id_that_is_no_path_segment_goes_out_nowhere() {
    let server = MockServer::start();
    let any_team = mount_any_team(&server);

    let session = login(&server, Some("a/b")).await;

    assert_eq!(session.client.account().team_name(), None);
    any_team.assert_calls(0);
}

#[tokio::test]
async fn an_uppercase_team_id_is_stored_lowercase() {
    let server = MockServer::start();
    let team = mount_team(&server, TEAM, 200, &team_body());

    let session = login(&server, Some(&TEAM.to_ascii_uppercase())).await;

    assert_eq!(session.client.account().team_id(), Some(TEAM));
    team.assert_calls(1);
    let store = session.client.store().expect("login attached a store");
    let row = store
        .load_self_user(QUALIFIED)
        .await
        .expect("load_self_user")
        .expect("self_user row present");
    assert_eq!(row.team_id.as_deref(), Some(TEAM));
}

#[tokio::test]
async fn a_resume_has_the_values_of_the_last_login() {
    let server = MockServer::start();
    let team = mount_team(&server, TEAM, 200, &team_body());
    let session = login(&server, Some(TEAM)).await;
    drop(session.client);

    let resumed = session.app.resume(QUALIFIED).await.expect("resume");

    assert_eq!(resumed.account().team_id(), Some(TEAM));
    assert_eq!(resumed.account().team_name(), Some(TEAM_NAME));
    team.assert_calls(1);
}

/// `remember_self_profile` writes the row whole.
#[tokio::test]
async fn a_profile_write_keeps_the_team() {
    let server = MockServer::start();
    mount_team(&server, TEAM, 200, &team_body());
    let session = login(&server, Some(TEAM)).await;
    server.mock(|when, then| {
        when.method(PUT).path("/v14/self");
        then.status(200);
    });

    session
        .client
        .set_display_name("Alice Two")
        .await
        .expect("set_display_name");

    let store = session.client.store().expect("login attached a store");
    let row = store
        .load_self_user(QUALIFIED)
        .await
        .expect("load_self_user")
        .expect("self_user row present");
    assert_eq!(row.name, "Alice Two");
    assert_eq!(row.team_id.as_deref(), Some(TEAM));
    assert_eq!(row.team_name.as_deref(), Some(TEAM_NAME));
}

/// The migrations `(version, path)` in version order.
fn migrations() -> Vec<(u32, PathBuf)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/storage/migrations");
    let mut files: Vec<(u32, PathBuf)> = std::fs::read_dir(dir)
        .expect("read migrations")
        .map(|entry| {
            let path = entry.expect("dir entry").path();
            let name = path.file_name().expect("name").to_string_lossy();
            let version = name
                .strip_prefix('V')
                .and_then(|rest| rest.split("__").next())
                .and_then(|v| v.parse().ok())
                .unwrap_or_else(|| panic!("{name} is no migration"));
            (version, path)
        })
        .collect();
    files.sort();
    files
}

#[test]
fn v33_adds_the_team_columns_and_a_v32_row_reads_null() {
    let files = migrations();
    let v33 = files
        .iter()
        .find(|(version, _)| *version == 33)
        .expect("V33 exists");
    let conn = rusqlite::Connection::open_in_memory().expect("memory db");
    for (_, path) in files.iter().filter(|(version, _)| *version <= 32) {
        conn.execute_batch(&std::fs::read_to_string(path).expect("read"))
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    }
    conn.execute(
        "INSERT INTO self_user (id, name) VALUES (?1, 'Alice')",
        [QUALIFIED],
    )
    .expect("a V32 row");

    conn.execute_batch(&std::fs::read_to_string(&v33.1).expect("read"))
        .expect("V33 applies over V32");

    let (id, name): (Option<String>, Option<String>) = conn
        .query_row(
            "SELECT team_id, team_name FROM self_user WHERE id = ?1",
            [QUALIFIED],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("the row");
    assert_eq!((id, name), (None, None));
}

fn workspace_file(rel: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// `step_94_tab_proxy_parity` holds the wasm class and the tab mirror to each
/// other; this holds that both name the two getters.
#[test]
fn the_browser_surface_carries_both_getters() {
    let wasm = workspace_file("kupferdraht-core/src/wasm/manager.rs");
    let tabs = "bindings/web/kupferdraht-tabs";
    let protocol = workspace_file(&format!("{tabs}/protocol.js"));
    let proxy = workspace_file(&format!("{tabs}/proxy.js"));
    let types = workspace_file(&format!("{tabs}/index.d.ts"));
    for getter in ["teamId", "teamName"] {
        assert!(
            wasm.contains(&format!("js_name = {getter})")),
            "the wasm Client has {getter}"
        );
        assert!(
            protocol.contains(&format!(r#"["Client", "{getter}"]"#)),
            "protocol.js GETTERS has {getter}"
        );
        assert!(
            proxy.contains(&format!("get {getter}()")),
            "proxy.js defines {getter}"
        );
        assert!(
            types.contains(&format!("readonly {getter}: string | undefined;")),
            "index.d.ts declares {getter}"
        );
    }
}
