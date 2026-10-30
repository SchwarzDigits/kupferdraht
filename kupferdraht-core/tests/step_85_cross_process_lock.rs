// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 85 — the cross-process lock around the two critical sections
//! (token refresh + persist, first-time database-key creation) and the
//! `busy_timeout` on every native open path (open.md §69 residual).
//!
//! TDD (red): written before the implementation. What they pin:
//!
//! - the acceptance criterion — two processes, two homes, two users on
//!   one machine work unchanged (the lock is per home / per account,
//!   never machine-wide);
//! - first-key creation is atomic under the account lock: exactly one
//!   key, the loser fails fast with `AccountLocked` and succeeds on
//!   retry with the winner's key;
//! - the refusal is cross-**process** (a child process holds the lock)
//!   and names the holder's pid;
//! - a second client on the same home adopts the rotation the first one
//!   persisted instead of rebinding a dead cookie;
//! - the resume-time refresh goes through the same lock;
//! - a second connection waits out a writer instead of failing
//!   `database is locked`, and the store reads back
//!   `busy_timeout = 5000` / `synchronous = FULL`.
//!
//! Cross-process harness (plan D6): the test binary re-launches itself
//! (`current_exe()` + `--exact <child test>`); a stdin-gated keystore
//! keeps the child inside the critical section until the parent says go.

use std::io::{BufRead as _, BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use httpmock::Method::{GET, POST, PUT};
use httpmock::MockServer;
use kupferdraht::keystore::{FileKeyStore, InMemoryKeyStore, SecureKeyStore};
use kupferdraht::storage::db_key::generate_db_key;
use kupferdraht::storage::{SessionRecord, SqliteStore, Store, open_default_store};
use kupferdraht::{Client, ClientBuilder, Kupferdraht, KupferdrahtError};
use tempfile::TempDir;

mod common;

use common::keystore::GatedKeyStore;

/// The account every single-account scenario below logs in as (`/self`
/// answers `user-uuid@example.test`).
const ACCOUNT: &str = "user-uuid@example.test";
/// The advisory lock file's name — one per home, one per account directory.
const LOCK_FILE: &str = "kupferdraht.lock";

// ── mocks ───────────────────────────────────────────────────────────────

/// The full login flow for `user` (`<user>@example.test`). The login
/// `/access` rebind **rotates** the cookie to `after-login-<user>` and
/// hands out `rebound-<user>` as the post-login access token, so a later
/// refresh mock can match on its own cookie.
fn mount_login_flow(server: &MockServer, user: &str) {
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
            .body(format!(
                r#"{{"access_token":"initial.jwt.token","token_type":"Bearer","expires_in":900,"user":"{user}"}}"#
            ));
    });
    server.mock(|when, then| {
        when.method(GET).path("/v14/self");
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(
                r#"{{"id":"{user}","qualified_id":{{"id":"{user}","domain":"example.test"}},"handle":"{user}","name":"{user}","email":"{user}@example.test"}}"#
            ));
    });
    server.mock(|when, then| {
        when.method(POST).path("/v14/clients");
        then.status(201)
            .header("content-type", "application/json")
            .body(r#"{"id":"new-client-id","type":"permanent"}"#);
    });
    server.mock(|when, then| {
        when.method(PUT).path("/v14/clients/new-client-id");
        then.status(200);
    });
    server.mock(|when, then| {
        when.method(POST)
            .path("/v14/access")
            .query_param("client_id", "new-client-id")
            .header("cookie", "zuid=test");
        then.status(200)
            .header("content-type", "application/json")
            .header(
                "set-cookie",
                format!("zuid=after-login-{user}; HttpOnly; Secure; Path=/access"),
            )
            .body(format!(
                r#"{{"access_token":"rebound-{user}","token_type":"Bearer","expires_in":900,"user":"{user}"}}"#
            ));
    });
    server.mock(|when, then| {
        when.method(POST)
            .path("/v14/mls/key-packages/self/new-client-id");
        then.status(201);
    });
    server.mock(|when, then| {
        when.method(GET).path("/v14/mls/public-keys");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"removal":{"ed25519":"ZWQ=","ecdsa_secp256r1_sha256":"cDI1Ng==","ecdsa_secp384r1_sha384":"cDM4NA==","ecdsa_secp521r1_sha512":"cDUyMQ=="}}"#);
    });
}

/// The refresh trio for `user`: `GET /clients` with the post-login token
/// → 401; `/access` with the post-login cookie → `refreshed-<user>` +
/// the rotated cookie `after-refresh-<user>`; `GET /clients` with the
/// new token → 200. Returns the `/access` mock for call counting.
fn mount_refresh_trio<'a>(server: &'a MockServer, user: &str) -> httpmock::Mock<'a> {
    server.mock(|when, then| {
        when.method(GET)
            .path("/v14/clients")
            .header("authorization", format!("Bearer rebound-{user}"));
        then.status(401)
            .header("content-type", "application/json")
            .body(r#"{"code":401,"label":"invalid-token","message":"x"}"#);
    });
    let refresh = server.mock(|when, then| {
        when.method(POST)
            .path("/v14/access")
            .query_param("client_id", "new-client-id")
            .header("cookie", format!("zuid=after-login-{user}"));
        then.status(200)
            .header("content-type", "application/json")
            .header(
                "set-cookie",
                format!("zuid=after-refresh-{user}; HttpOnly; Secure; Path=/access"),
            )
            .body(format!(
                r#"{{"access_token":"refreshed-{user}","token_type":"Bearer","expires_in":900,"user":"{user}"}}"#
            ));
    });
    server.mock(|when, then| {
        when.method(GET)
            .path("/v14/clients")
            .header("authorization", format!("Bearer refreshed-{user}"));
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"[{"id":"abcd1234","type":"permanent","time":"2026-04-29T10:00:00Z"}]"#);
    });
    refresh
}

// ── fixtures ────────────────────────────────────────────────────────────

fn memory_keystore() -> Arc<dyn SecureKeyStore> {
    Arc::new(InMemoryKeyStore::new())
}

/// Log `user` in through the device manager (the CLI's path: the session,
/// device, server config and registry row are persisted so a later
/// `resume` works without Wire).
async fn login_via_manager(app: &Kupferdraht, server: &MockServer, user: &str) -> Arc<Client> {
    app.login(
        common::server_config(&server.base_url()),
        format!("{user}@example.test"),
        "pw".into(),
        None,
        None,
    )
    .await
    .expect("login")
    .register_device()
    .await
    .expect("register device")
}

/// A raw `ClientBuilder::resume` — bypasses the manager's one-live-client
/// cache (Step 39) so a test can hold two independent `Client`s on one
/// home, which is what two processes look like from the store's side.
async fn resume_via_builder(
    home: &Path,
    keystore: &Arc<dyn SecureKeyStore>,
    account: &str,
) -> Result<Arc<Client>, KupferdrahtError> {
    ClientBuilder::new()
        .permit_http()
        .base_dir(home.to_path_buf())
        .keystore(keystore.clone())
        .account(account)
        .resume()
        .await
}

fn account_lock_path(home: &Path, account: &str) -> PathBuf {
    home.join("accounts").join(account).join(LOCK_FILE)
}

// ── cross-process harness ───────────────────────────────────────────────

const CHILD_HOME_ENV: &str = "KUPFERDRAHT_STEP85_CHILD_HOME";
const CHILD_KEYSTORE_ENV: &str = "KUPFERDRAHT_STEP85_CHILD_KEYSTORE";
const CHILD_TEST: &str = "child_holds_account_lock_until_stdin_says_go";

/// **Child-process entry** for the cross-process scenarios (plan D6) —
/// a no-op when run as an ordinary test. Spawned by [`LockHolder::spawn`]
/// with the home and a file-keystore directory in the environment, it
/// opens the account's store with a keystore that has no key yet, so the
/// open takes the account lock; the gated re-read under the lock prints
/// `LOCK-HELD` and blocks on stdin, which keeps the child inside the
/// critical section until the parent writes a line. (Gated on the read,
/// not the write: for a home whose database exists, ADR-0029 refuses
/// without writing a key — Step 96.)
#[test]
fn child_holds_account_lock_until_stdin_says_go() {
    let Ok(home) = std::env::var(CHILD_HOME_ENV) else {
        return;
    };
    let keystore_dir = std::env::var(CHILD_KEYSTORE_ENV).expect("child keystore dir");
    let gate: Box<dyn FnOnce() + Send> = Box::new(|| {
        println!("LOCK-HELD");
        std::io::stdout().flush().expect("flush");
        let mut line = String::new();
        std::io::stdin()
            .lock()
            .read_line(&mut line)
            .expect("read the go line");
    });
    let keystore: Arc<dyn SecureKeyStore> = Arc::new(GatedKeyStore::on_second_read(
        FileKeyStore::new(PathBuf::from(keystore_dir)),
        gate,
    ));
    // The outcome is the parent's business (a scenario may hand the child
    // a home whose database its fresh key cannot open); the lock hold is
    // what the parent observes.
    let _ = open_default_store(Path::new(&home), &keystore, ACCOUNT);
    println!("CHILD-DONE");
}

/// The spawned child holding the account lock; killed on drop so a
/// failing parent assertion never leaves it behind.
struct LockHolder {
    child: Child,
    stdin: Option<ChildStdin>,
}

impl LockHolder {
    fn spawn(home: &Path, keystore_dir: &Path) -> Self {
        let mut child = Command::new(std::env::current_exe().expect("current exe"))
            .args([CHILD_TEST, "--exact", "--nocapture", "--test-threads=1"])
            .env(CHILD_HOME_ENV, home)
            .env(CHILD_KEYSTORE_ENV, keystore_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn the lock-holder child");
        let stdout = child.stdout.take().expect("child stdout");
        let stdin = child.stdin.take().expect("child stdin");
        let mut lines = BufReader::new(stdout).lines();
        // libtest prefixes the test's own output with `test <name> ... `
        // on the same line when it runs single-threaded, so match the
        // marker anywhere in the line.
        loop {
            match lines.next() {
                Some(Ok(line)) if line.contains("LOCK-HELD") => break,
                Some(Ok(_)) => {}
                other => panic!("child exited before holding the lock: {other:?}"),
            }
        }
        Self {
            child,
            stdin: Some(stdin),
        }
    }

    fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Let the child finish its critical section and wait for it to exit.
    fn release(mut self) {
        let mut stdin = self.stdin.take().expect("stdin still attached");
        stdin.write_all(b"go\n").expect("write go");
        drop(stdin);
        let status = self.child.wait().expect("wait for child");
        // The child's own outcome is scenario-specific (see the entry's
        // doc); only its termination matters here.
        let _ = status;
    }
}

impl Drop for LockHolder {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// ── the acceptance criterion: two homes, two users, one machine ─────────

/// Two consumers on one machine — say kupferdraht-tui and kupferdraht-cli — with
/// their own homes and keystores: fully concurrent logins, resumes and
/// token refreshes must work unchanged, and each home carries its own
/// lock files (nothing shared, nothing machine-wide).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_homes_two_users_on_one_machine_work_unchanged() {
    let server_a = MockServer::start();
    let server_b = MockServer::start();
    mount_login_flow(&server_a, "user-a");
    mount_login_flow(&server_b, "user-b");
    let home_a = TempDir::new().expect("home a");
    let home_b = TempDir::new().expect("home b");
    let app_a = Kupferdraht::open(home_a.path().to_path_buf(), memory_keystore())
        .expect("open a")
        .permit_http();
    let app_b = Kupferdraht::open(home_b.path().to_path_buf(), memory_keystore())
        .expect("open b")
        .permit_http();

    let (client_a, client_b) = tokio::join!(
        login_via_manager(&app_a, &server_a, "user-a"),
        login_via_manager(&app_b, &server_b, "user-b")
    );
    let refresh_a = mount_refresh_trio(&server_a, "user-a");
    let refresh_b = mount_refresh_trio(&server_b, "user-b");
    let (devices_a, devices_b) = tokio::join!(client_a.devices(), client_b.devices());
    devices_a.expect("home a refreshes and retries unhindered");
    devices_b.expect("home b refreshes and retries unhindered");
    refresh_a.assert_calls(1);
    refresh_b.assert_calls(1);

    for (home, user) in [(&home_a, "user-a"), (&home_b, "user-b")] {
        assert!(
            home.path().join(LOCK_FILE).is_file(),
            "each home carries its own home-level lock file"
        );
        assert!(
            account_lock_path(home.path(), &format!("{user}@example.test")).is_file(),
            "each account directory carries its own lock file"
        );
    }
}

/// The desktop reality: CLI + TUI share the OS keyring. Two homes, two
/// different users, ONE keystore — the first logins run one after the
/// other (that is when the shared `global-db` key is created), then
/// resumes and refreshes run concurrently. Different accounts on one
/// keystore are unaffected by the lock.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_homes_on_one_shared_keystore_do_not_conflict() {
    let server_a = MockServer::start();
    let server_b = MockServer::start();
    mount_login_flow(&server_a, "user-a");
    mount_login_flow(&server_b, "user-b");
    let home_a = TempDir::new().expect("home a");
    let home_b = TempDir::new().expect("home b");
    let keystore = memory_keystore();
    let app_a = Kupferdraht::open(home_a.path().to_path_buf(), keystore.clone())
        .expect("open a")
        .permit_http();
    let app_b = Kupferdraht::open(home_b.path().to_path_buf(), keystore.clone())
        .expect("open b")
        .permit_http();
    drop(login_via_manager(&app_a, &server_a, "user-a").await);
    drop(login_via_manager(&app_b, &server_b, "user-b").await);

    let (resumed_a, resumed_b) = tokio::join!(
        resume_via_builder(home_a.path(), &keystore, "user-a@example.test"),
        resume_via_builder(home_b.path(), &keystore, "user-b@example.test")
    );
    let client_a = resumed_a.expect("resume a");
    let client_b = resumed_b.expect("resume b");
    let refresh_a = mount_refresh_trio(&server_a, "user-a");
    let refresh_b = mount_refresh_trio(&server_b, "user-b");
    let (devices_a, devices_b) = tokio::join!(client_a.devices(), client_b.devices());
    devices_a.expect("home a refreshes unhindered by home b");
    devices_b.expect("home b refreshes unhindered by home a");
    refresh_a.assert_calls(1);
    refresh_b.assert_calls(1);
    assert!(
        account_lock_path(home_a.path(), "user-a@example.test").is_file()
            && account_lock_path(home_b.path(), "user-b@example.test").is_file(),
        "each home keeps its own account lock file"
    );
}

// ── first-key creation is atomic under the account lock ─────────────────

/// S18: two opens of a fresh account race the first key creation. The
/// first caller is parked inside `set_secret` (i.e. under the lock); the
/// second must fail fast with `AccountLocked`, not generate a second key.
/// Once the first finishes, a retry finds its key — one key ever.
#[test]
fn first_key_creation_is_serialised_under_the_account_lock() {
    let home = TempDir::new().expect("home");
    let (entered_tx, entered_rx) = mpsc::channel::<()>();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let gate: Box<dyn FnOnce() + Send> = Box::new(move || {
        entered_tx.send(()).expect("announce");
        release_rx.recv().expect("wait for release");
    });
    let gated = Arc::new(GatedKeyStore::new(InMemoryKeyStore::new(), gate));
    let keystore: Arc<dyn SecureKeyStore> = gated.clone();

    let winner_home = home.path().to_path_buf();
    let winner_keystore = keystore.clone();
    let winner = std::thread::spawn(move || {
        open_default_store(&winner_home, &winner_keystore, ACCOUNT).map(|_| ())
    });
    entered_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("the winner reaches set_secret under the lock");

    let err = open_default_store(home.path(), &keystore, ACCOUNT)
        .err()
        .expect("the loser must fail fast while the winner holds the lock");
    assert!(
        matches!(&err, KupferdrahtError::AccountLocked(msg) if msg.contains(ACCOUNT)),
        "expected AccountLocked naming the account, got {err:?}"
    );

    release_tx.send(()).expect("release the winner");
    winner
        .join()
        .expect("winner thread")
        .expect("the winner opens the store");
    let _retry = open_default_store(home.path(), &keystore, ACCOUNT)
        .expect("the retry finds the winner's key without a lock");
    assert_eq!(gated.sets(), 1, "exactly one key was ever generated");
}

/// The same refusal across a real process boundary: a child process holds
/// the account lock; this process's open fails typed and names the
/// child's pid; once the child is done, the open succeeds with the key
/// the child wrote into the shared (file) keystore.
#[test]
fn an_open_while_another_process_holds_the_lock_fails_typed_naming_the_holder() {
    let home = TempDir::new().expect("home");
    let keystore_dir = TempDir::new().expect("keystore dir");
    let holder = LockHolder::spawn(home.path(), keystore_dir.path());
    let keystore: Arc<dyn SecureKeyStore> = Arc::new(FileKeyStore::new(keystore_dir.path()));

    let err = open_default_store(home.path(), &keystore, ACCOUNT)
        .err()
        .expect("must be refused while the child process holds the account lock");
    let pid = holder.pid().to_string();
    assert!(
        matches!(&err, KupferdrahtError::AccountLocked(msg) if msg.contains(ACCOUNT) && msg.contains(&pid)),
        "expected AccountLocked naming account + holder pid {pid}, got {err:?}"
    );

    holder.release();
    let _store = open_default_store(home.path(), &keystore, ACCOUNT)
        .expect("free once the holder finished; its key is in the shared keystore");
}

// ── token refresh: adopt the other process's rotation ───────────────────

/// Two resumed `Client`s on one home (= two processes from the store's
/// side). The first refreshes and persists the rotated cookie; the second
/// gets its own 401 and must **adopt** the persisted rotation instead of
/// rebinding with the cookie Wire already invalidated — `/access` is hit
/// exactly once.
#[tokio::test]
async fn second_client_adopts_the_persisted_rotation_instead_of_rebinding() {
    let server = MockServer::start();
    mount_login_flow(&server, "user-uuid");
    let home = TempDir::new().expect("home");
    let keystore = memory_keystore();
    let app = Kupferdraht::open(home.path().to_path_buf(), keystore.clone())
        .expect("open")
        .permit_http();
    drop(login_via_manager(&app, &server, "user-uuid").await);

    let first = resume_via_builder(home.path(), &keystore, ACCOUNT)
        .await
        .expect("first client");
    let second = resume_via_builder(home.path(), &keystore, ACCOUNT)
        .await
        .expect("second client");
    // The persisted issued-at must be strictly newer than the second
    // client's snapshot for the adoption to be observable.
    tokio::time::sleep(Duration::from_millis(5)).await;
    let refresh = mount_refresh_trio(&server, "user-uuid");

    first.devices().await.expect("first client refreshes");
    refresh.assert_calls(1);
    second
        .devices()
        .await
        .expect("second client adopts the persisted rotation and retries");
    refresh.assert_calls(1);
    assert_eq!(
        second.access_token().as_deref(),
        Some("refreshed-user-uuid"),
        "the second client runs on the token the first one persisted"
    );
    assert_eq!(
        second.refresh_token().as_deref(),
        Some("after-refresh-user-uuid"),
        "…and on the rotated cookie"
    );
}

/// The resume-time refresh (`access_token_issued_at_ms` aged past the
/// TTL) is the same critical section: while another process holds the
/// account lock it fails typed and leaves `/access` untouched; afterwards
/// the resume refreshes normally.
#[tokio::test]
async fn resume_refresh_while_another_process_holds_the_lock_fails_typed() {
    let server = MockServer::start();
    mount_login_flow(&server, "user-uuid");
    let home = TempDir::new().expect("home");
    let keystore = memory_keystore();
    let app = Kupferdraht::open(home.path().to_path_buf(), keystore.clone())
        .expect("open")
        .permit_http();
    let client = login_via_manager(&app, &server, "user-uuid").await;
    let store = client.store().expect("store");
    let mut session: SessionRecord = store
        .load_session(ACCOUNT)
        .await
        .expect("load")
        .expect("session persisted by login");
    session.access_token_issued_at_ms -= 20 * 60 * 1000;
    store.save_session(&session).await.expect("age the token");
    drop(client);
    let refresh = mount_refresh_trio(&server, "user-uuid");

    let holder = LockHolder::spawn(home.path(), TempDir::new().expect("child keystore").path());
    let err = resume_via_builder(home.path(), &keystore, ACCOUNT)
        .await
        .expect_err("resume must not rebind while another process holds the lock");
    assert!(
        matches!(&err, KupferdrahtError::AccountLocked(_)),
        "expected AccountLocked, got {err:?}"
    );
    refresh.assert_calls(0);
    holder.release();

    let client = resume_via_builder(home.path(), &keystore, ACCOUNT)
        .await
        .expect("resume refreshes once the lock is free");
    refresh.assert_calls(1);
    assert_eq!(
        client.access_token().as_deref(),
        Some("refreshed-user-uuid")
    );
}

// ── busy_timeout on the open paths (open.md §69) ────────────────────────

fn session_record() -> SessionRecord {
    SessionRecord {
        user_id: ACCOUNT.into(),
        device_id: "new-client-id".into(),
        access_token: "t".into(),
        refresh_token: "z".into(),
        cookie_label: None,
        api_version: 14,
        access_token_issued_at_ms: 1,
    }
}

/// A second connection on the same database waits out a writer instead
/// of failing `database is locked` at once — what two processes on one
/// home need from every write.
#[tokio::test]
async fn a_write_waits_for_a_concurrent_writer_instead_of_failing_busy() {
    let dir = TempDir::new().expect("dir");
    let path = dir.path().join("kupferdraht.db");
    let key = generate_db_key();
    let writer_store = SqliteStore::open(&path, &key).expect("writer store");
    let waiting_store = SqliteStore::open(&path, &key).expect("waiting store");
    let (begun_tx, begun_rx) = mpsc::channel::<()>();
    let writer = std::thread::spawn(move || {
        let conn = writer_store.lock().expect("lock");
        conn.execute_batch("BEGIN IMMEDIATE").expect("begin");
        begun_tx.send(()).expect("announce");
        std::thread::sleep(Duration::from_millis(300));
        conn.execute_batch("COMMIT").expect("commit");
    });
    begun_rx.recv().expect("writer holds the write lock");

    let started = Instant::now();
    waiting_store
        .save_session(&session_record())
        .await
        .expect("the write waits (busy_timeout) instead of failing `database is locked`");
    assert!(
        started.elapsed() >= Duration::from_millis(200),
        "the write actually waited for the writer, not raced it"
    );
    writer.join().expect("writer thread");
}

/// Read back off the live connection (LEARNINGS §5.28): `busy_timeout`
/// is 5000 ms and `synchronous` stays FULL (2) on the account store.
#[test]
fn account_store_connection_carries_busy_timeout_and_full_synchronous() {
    let dir = TempDir::new().expect("dir");
    let store =
        SqliteStore::open(&dir.path().join("kupferdraht.db"), &generate_db_key()).expect("open");
    let conn = store.lock().expect("lock");
    let busy: i64 = conn
        .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
        .expect("busy_timeout");
    assert_eq!(busy, 5000, "busy_timeout in effect on kupferdraht.db");
    let synchronous: i64 = conn
        .query_row("PRAGMA synchronous", [], |row| row.get(0))
        .expect("synchronous");
    assert_eq!(synchronous, 2, "synchronous = FULL on kupferdraht.db");
}
