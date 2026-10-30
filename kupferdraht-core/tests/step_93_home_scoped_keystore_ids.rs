// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 93 — home-scoped keystore ids and the per-home login cookie label
//! (ADR-0025). Closes open.md gap-B.33 (label eviction between two homes
//! of one user on one keystore, observations §83) and gap-B.34 (two homes
//! racing the first-ever creation of one keystore-wide id).
//!
//! TDD (red): written before the implementation. What they pin:
//!
//! - the home id: `base/home.id`, 32 lowercase hex, created once, stable
//!   across re-opens; the label is `kupferdraht-<home-id>`;
//! - two homes on one keystore never share a keystore entry — distinct
//!   ids, labels and database keys, no keystore-wide id written any more;
//! - the §83 scenario with a mock that keeps one cookie per label: a
//!   refresh on one home leaves the other home's session usable;
//! - gap-B.34: two homes creating the same account's first key at the
//!   same instant both succeed, each under its own key, no `AccountLocked`;
//! - legacy adoption (H3): a keystore-wide entry is adopted under the
//!   namespaced id only when the database it opens exists, and is never
//!   deleted; a fresh database gets a fresh key; a whole pre-Step-93 home
//!   resumes after adopting all three keys;
//! - the id creation runs under the home lock (a held lock is a typed,
//!   retryable refusal; the retry reads the winner's id) and an existing
//!   id is read lock-free; a malformed `home.id` is a recoverable error
//!   that is never rewritten;
//! - the session row records the label it was logged in with.

use std::fs::{self, File};
use std::path::Path;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use httpmock::Method::{GET, POST, PUT};
use httpmock::{Mock, MockServer};
use kupferdraht::keystore::{InMemoryKeyStore, SecureKeyStore};
use kupferdraht::storage::open_default_store;
use kupferdraht::{Client, Kupferdraht, KupferdrahtError};
use tempfile::TempDir;

mod common;

use common::keystore::GatedKeyStore;

/// The one account every scenario logs in as (`/self` answers
/// `user-uuid@example.test`) — the §83 case is one user, two homes.
const ACCOUNT: &str = "user-uuid@example.test";
const HOME_ID_FILE: &str = "home.id";
const LOCK_FILE: &str = "kupferdraht.lock";

// ── mocks ───────────────────────────────────────────────────────────────

/// The label-independent half of the login flow.
fn mount_shared_login_mocks(server: &MockServer) {
    server.mock(|when, then| {
        when.method(GET).path("/api-version");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"supported":[14],"development":[]}"#);
    });
    server.mock(|when, then| {
        when.method(GET).path("/v14/self");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"id":"user-uuid","qualified_id":{"id":"user-uuid","domain":"example.test"},"handle":"alice","name":"Alice","email":"alice@example.test"}"#);
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

/// Wire's cookie model, one `label` at a time: `POST /login` with this
/// `label` hands out the cookie `login-<tag>`; the login rebind on that
/// cookie rotates it to `after-login-<tag>` and issues the access token
/// `rebound-<tag>`. Returns the `/login` mock for call counting.
fn mount_login_for_label<'a>(server: &'a MockServer, label: &str, tag: &str) -> Mock<'a> {
    let login = server.mock(|when, then| {
        when.method(POST)
            .path("/v14/login")
            .json_body_includes(format!(r#"{{"label":"{label}"}}"#));
        then.status(200)
            .header("content-type", "application/json")
            .header(
                "set-cookie",
                format!("zuid=login-{tag}; HttpOnly; Secure; Path=/access"),
            )
            .body(format!(
                r#"{{"access_token":"initial-{tag}","token_type":"Bearer","expires_in":900,"user":"user-uuid"}}"#
            ));
    });
    server.mock(|when, then| {
        when.method(POST)
            .path("/v14/access")
            .query_param("client_id", "new-client-id")
            .header("cookie", format!("zuid=login-{tag}"));
        then.status(200)
            .header("content-type", "application/json")
            .header(
                "set-cookie",
                format!("zuid=after-login-{tag}; HttpOnly; Secure; Path=/access"),
            )
            .body(format!(
                r#"{{"access_token":"rebound-{tag}","token_type":"Bearer","expires_in":900,"user":"user-uuid"}}"#
            ));
    });
    login
}

/// The refresh trio for the session tagged `tag`: `GET /clients` with its
/// post-login token → 401; `/access` with its own post-login cookie →
/// `refreshed-<tag>` + the rotated cookie; `GET /clients` with the new
/// token → 200. Returns the `/access` mock for call counting.
fn mount_refresh_trio<'a>(server: &'a MockServer, tag: &str) -> Mock<'a> {
    server.mock(|when, then| {
        when.method(GET)
            .path("/v14/clients")
            .header("authorization", format!("Bearer rebound-{tag}"));
        then.status(401)
            .header("content-type", "application/json")
            .body(r#"{"code":401,"label":"invalid-token","message":"x"}"#);
    });
    let refresh = server.mock(|when, then| {
        when.method(POST)
            .path("/v14/access")
            .query_param("client_id", "new-client-id")
            .header("cookie", format!("zuid=after-login-{tag}"));
        then.status(200)
            .header("content-type", "application/json")
            .header(
                "set-cookie",
                format!("zuid=after-refresh-{tag}; HttpOnly; Secure; Path=/access"),
            )
            .body(format!(
                r#"{{"access_token":"refreshed-{tag}","token_type":"Bearer","expires_in":900,"user":"user-uuid"}}"#
            ));
    });
    server.mock(|when, then| {
        when.method(GET)
            .path("/v14/clients")
            .header("authorization", format!("Bearer refreshed-{tag}"));
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

fn open_app(home: &Path, keystore: &Arc<dyn SecureKeyStore>) -> Kupferdraht {
    Kupferdraht::open(home.to_path_buf(), keystore.clone())
        .expect("open the home")
        .permit_http()
}

async fn login_via_manager(app: &Kupferdraht, server: &MockServer) -> Arc<Client> {
    app.login(
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
    .expect("register device")
}

/// The id as persisted: `base/home.id`, trimmed.
fn home_id_on_disk(home: &Path) -> String {
    fs::read_to_string(home.join(HOME_ID_FILE))
        .expect("home.id is readable")
        .trim()
        .to_owned()
}

/// The keystore id of `legacy` (`global-db`, `account-<id>`, …) in the home
/// `home_id` — the ADR-0025 shape.
fn namespaced(home_id: &str, legacy: &str) -> String {
    format!("kupferdraht.{home_id}.{legacy}")
}

fn legacy_ids() -> [String; 3] {
    [
        "global-db".to_owned(),
        format!("account-{ACCOUNT}"),
        format!("account-{ACCOUNT}-mls"),
    ]
}

fn secret(keystore: &Arc<dyn SecureKeyStore>, id: &str) -> Option<Vec<u8>> {
    keystore.get_secret(id.to_owned()).expect("keystore read")
}

fn is_home_id(id: &str) -> bool {
    id.len() == 32
        && id
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

/// Hold `base/kupferdraht.lock` the way another process would (the same kernel
/// primitive `ProcessLock` uses; two open file descriptions contend).
fn hold_home_lock(home: &Path) -> File {
    fs::create_dir_all(home).expect("home dir");
    let file = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(home.join(LOCK_FILE))
        .expect("lock file");
    file.try_lock().expect("the test holds the home lock");
    file
}

// ── the home id ─────────────────────────────────────────────────────────

/// `base/home.id` is created on the first open, is 32 lowercase hex, is
/// what every later open reads back, and is the label's tail.
#[test]
fn home_id_is_created_once_and_stable_across_reopens() {
    let home = TempDir::new().expect("home");
    let keystore = memory_keystore();
    let first = open_app(home.path(), &keystore);
    let id = first.home_id().to_owned();
    assert!(is_home_id(&id), "32 lowercase hex, got {id:?}");
    assert_eq!(first.cookie_label(), format!("kupferdraht-{id}"));
    assert_eq!(
        fs::read_to_string(home.path().join(HOME_ID_FILE)).expect("home.id"),
        format!("{id}\n"),
        "the file holds the id and a newline, nothing else"
    );
    drop(first);
    let second = open_app(home.path(), &keystore);
    assert_eq!(second.home_id(), id, "a re-open reads the same id");
    assert_eq!(home_id_on_disk(home.path()), id);
}

/// A first open while another process holds the home lock is refused
/// typed (no id is generated); once the holder wrote its id and let go,
/// the retry reads exactly that id.
#[test]
fn first_open_under_a_held_home_lock_fails_typed_and_the_retry_reads_the_winners_id() {
    const WINNER: &str = "0123456789abcdef0123456789abcdef";
    let home = TempDir::new().expect("home");
    let keystore = memory_keystore();
    let lock = hold_home_lock(home.path());
    let err = Kupferdraht::open(home.path().to_path_buf(), keystore.clone())
        .err()
        .expect("a fresh home cannot take the home lock while another process holds it");
    assert!(
        matches!(&err, KupferdrahtError::AccountLocked(msg) if msg.contains("home")),
        "expected AccountLocked naming the home, got {err:?}"
    );
    assert!(
        !home.path().join(HOME_ID_FILE).exists(),
        "the loser must not have written an id of its own"
    );
    // The winner persists its id, still under the lock, then releases.
    fs::write(home.path().join(HOME_ID_FILE), format!("{WINNER}\n")).expect("winner writes");
    lock.unlock().expect("release");
    let app = open_app(home.path(), &keystore);
    assert_eq!(app.home_id(), WINNER, "the retry adopts the winner's id");
}

/// An existing id is read without touching the lock — an open must never
/// contend with a process inside the home's critical section.
#[test]
fn an_existing_home_id_is_read_without_taking_the_home_lock() {
    let home = TempDir::new().expect("home");
    let keystore = memory_keystore();
    let id = open_app(home.path(), &keystore).home_id().to_owned();
    let _lock = hold_home_lock(home.path());
    let app = Kupferdraht::open(home.path().to_path_buf(), keystore)
        .expect("an existing id needs no lock");
    assert_eq!(app.home_id(), id);
}

/// A malformed `home.id` is runtime data: a recoverable storage error that
/// names the file, and the file is left exactly as it was — a regenerated
/// id would orphan every key of the home.
#[test]
fn malformed_home_id_is_a_recoverable_storage_error_and_is_not_rewritten() {
    let home = TempDir::new().expect("home");
    let path = home.path().join(HOME_ID_FILE);
    fs::write(&path, "not-a-home-id\n").expect("write garbage");
    let err = Kupferdraht::open(home.path().to_path_buf(), memory_keystore())
        .err()
        .expect("garbage must not open");
    assert!(
        matches!(&err, KupferdrahtError::Storage(msg) if msg.contains(HOME_ID_FILE)),
        "expected Storage naming home.id, got {err:?}"
    );
    assert_eq!(
        fs::read_to_string(&path).expect("read back"),
        "not-a-home-id\n",
        "never auto-repaired"
    );
}

// ── two homes, one keystore ─────────────────────────────────────────────

/// Two homes on ONE keystore (the desktop reality: CLI + TUI on one
/// keychain) get distinct ids and labels, and their registry keys sit
/// under distinct namespaced ids — no keystore-wide `global-db` any more.
#[test]
fn two_homes_on_one_keystore_get_distinct_ids_labels_and_registry_keys() {
    let home_a = TempDir::new().expect("home a");
    let home_b = TempDir::new().expect("home b");
    let keystore = memory_keystore();
    let app_a = open_app(home_a.path(), &keystore);
    let app_b = open_app(home_b.path(), &keystore);
    assert_ne!(app_a.home_id(), app_b.home_id());
    assert_ne!(app_a.cookie_label(), app_b.cookie_label());
    let key_a = secret(&keystore, &namespaced(app_a.home_id(), "global-db"))
        .expect("home a's registry key under its namespaced id");
    let key_b = secret(&keystore, &namespaced(app_b.home_id(), "global-db"))
        .expect("home b's registry key under its namespaced id");
    assert_ne!(key_a, key_b, "each home keys its registry independently");
    assert!(
        secret(&keystore, "global-db").is_none(),
        "the keystore-wide id is never written any more"
    );
}

/// The §83 scenario, mocked with one cookie per label: two homes of the
/// SAME user on one keystore log in with distinct labels, hold distinct
/// keys, record their labels in their session rows, and a token refresh on
/// one home leaves the other home's cookie usable.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_homes_of_one_user_on_one_keystore_keep_both_sessions_alive() {
    let server = MockServer::start();
    mount_shared_login_mocks(&server);
    let home_a = TempDir::new().expect("home a");
    let home_b = TempDir::new().expect("home b");
    let keystore = memory_keystore();
    let app_a = open_app(home_a.path(), &keystore);
    let app_b = open_app(home_b.path(), &keystore);
    let (label_a, label_b) = (app_a.cookie_label(), app_b.cookie_label());
    assert_ne!(label_a, label_b, "one label per home");
    let login_a = mount_login_for_label(&server, &label_a, "a");
    let login_b = mount_login_for_label(&server, &label_b, "b");

    let client_a = login_via_manager(&app_a, &server).await;
    let client_b = login_via_manager(&app_b, &server).await;
    login_a.assert_calls(1);
    login_b.assert_calls(1);

    for legacy in legacy_ids() {
        let key_a = secret(&keystore, &namespaced(app_a.home_id(), &legacy))
            .unwrap_or_else(|| panic!("home a holds {legacy} under its namespaced id"));
        let key_b = secret(&keystore, &namespaced(app_b.home_id(), &legacy))
            .unwrap_or_else(|| panic!("home b holds {legacy} under its namespaced id"));
        assert_ne!(key_a, key_b, "{legacy}: one key per home");
        assert!(
            secret(&keystore, &legacy).is_none(),
            "{legacy}: no keystore-wide entry"
        );
    }
    for (client, label) in [(&client_a, &label_a), (&client_b, &label_b)] {
        let session = client
            .store()
            .expect("store")
            .load_session(ACCOUNT)
            .await
            .expect("load")
            .expect("session persisted");
        assert_eq!(
            session.cookie_label.as_deref(),
            Some(label.as_str()),
            "the session row records the label it was logged in with"
        );
    }

    let refresh_a = mount_refresh_trio(&server, "a");
    let refresh_b = mount_refresh_trio(&server, "b");
    client_a
        .devices()
        .await
        .expect("home a refreshes on its own cookie");
    client_b
        .devices()
        .await
        .expect("home b is still alive after home a's /access");
    refresh_a.assert_calls(1);
    refresh_b.assert_calls(1);
}

/// gap-B.34: home A is parked inside the first-ever creation of the
/// account's key (its `set_secret`); home B creates the same account's
/// first key on the same keystore at that moment — and must succeed at
/// once: a different home is a different lock and a different id.
#[test]
fn first_key_creation_of_one_account_in_two_homes_on_one_keystore_does_not_contend() {
    let home_a = TempDir::new().expect("home a");
    let home_b = TempDir::new().expect("home b");
    let (entered_tx, entered_rx) = mpsc::channel::<()>();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let gate: Box<dyn FnOnce() + Send> = Box::new(move || {
        entered_tx.send(()).expect("announce");
        release_rx.recv().expect("wait for release");
    });
    let gated = Arc::new(GatedKeyStore::new(InMemoryKeyStore::new(), gate));
    let keystore: Arc<dyn SecureKeyStore> = gated.clone();

    let path_a = home_a.path().to_path_buf();
    let keystore_a = keystore.clone();
    let home_a_open =
        std::thread::spawn(move || open_default_store(&path_a, &keystore_a, ACCOUNT).map(|_| ()));
    entered_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("home a reaches set_secret inside its critical section");

    let started = Instant::now();
    open_default_store(home_b.path(), &keystore, ACCOUNT)
        .expect("home b creates its own key while home a is inside its critical section");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "home b neither waited nor was refused"
    );

    release_tx.send(()).expect("release home a");
    home_a_open
        .join()
        .expect("home a thread")
        .expect("home a opens its store");
    assert_eq!(gated.sets(), 2, "two keys were generated — one per home");
    let legacy = format!("account-{ACCOUNT}");
    let key_a = secret(
        &keystore,
        &namespaced(&home_id_on_disk(home_a.path()), &legacy),
    )
    .expect("home a's key");
    let key_b = secret(
        &keystore,
        &namespaced(&home_id_on_disk(home_b.path()), &legacy),
    )
    .expect("home b's key");
    assert_ne!(key_a, key_b, "neither home overwrote the other's key");
}

// ── legacy adoption (H3) ────────────────────────────────────────────────

/// A pre-Step-93 home (no `home.id`) on a keystore that holds the
/// account key under the keystore-wide id: the open adopts that key under
/// the new namespaced id — the existing database opens — and the legacy
/// entry is left untouched.
#[test]
fn legacy_account_key_is_adopted_for_an_existing_database_and_the_legacy_entry_stays() {
    let home = TempDir::new().expect("home");
    let legacy = format!("account-{ACCOUNT}");
    // Create the database the way any current build does, then turn the
    // home + keystore into their pre-Step-93 shape.
    let first = memory_keystore();
    drop(open_default_store(home.path(), &first, ACCOUNT).expect("create the database"));
    let key = secret(&first, &namespaced(&home_id_on_disk(home.path()), &legacy))
        .expect("the key the database was created under");
    fs::remove_file(home.path().join(HOME_ID_FILE)).expect("a pre-Step-93 home has no id");
    let keystore = memory_keystore();
    keystore
        .set_secret(legacy.clone(), key.clone())
        .expect("the legacy keystore holds the key under the keystore-wide id");

    let store =
        open_default_store(home.path(), &keystore, ACCOUNT).expect("the legacy key is adopted");
    drop(store);
    let id = home_id_on_disk(home.path());
    assert_eq!(
        secret(&keystore, &namespaced(&id, &legacy)),
        Some(key.clone()),
        "the namespaced id now holds the same 32 bytes"
    );
    assert_eq!(
        secret(&keystore, &legacy),
        Some(key),
        "the legacy entry is kept — another home may still need it"
    );
}

/// A legacy entry without a database to open is NOT adopted: a fresh
/// database always gets a fresh key (no accidental key sharing between a
/// new home and the home that owns the legacy entry).
#[test]
fn a_legacy_key_without_a_database_is_not_adopted() {
    let home = TempDir::new().expect("home");
    let legacy = format!("account-{ACCOUNT}");
    let stray = vec![7u8; 32];
    let keystore = memory_keystore();
    keystore
        .set_secret(legacy.clone(), stray.clone())
        .expect("a sibling home's key under the keystore-wide id");

    drop(open_default_store(home.path(), &keystore, ACCOUNT).expect("fresh database"));
    let adopted = secret(
        &keystore,
        &namespaced(&home_id_on_disk(home.path()), &legacy),
    )
    .expect("a namespaced key was written");
    assert_ne!(adopted, stray, "a fresh database gets a fresh key");
    assert_eq!(
        secret(&keystore, &legacy),
        Some(stray),
        "the stray entry is untouched"
    );
}

/// Nothing legacy, nothing to adopt: exactly the namespaced ids are
/// written, never the keystore-wide ones.
#[test]
fn a_fresh_home_writes_namespaced_ids_only() {
    let home = TempDir::new().expect("home");
    let keystore = memory_keystore();
    let app = open_app(home.path(), &keystore);
    drop(open_default_store(home.path(), &keystore, ACCOUNT).expect("account store"));
    let id = app.home_id();
    for legacy in ["global-db".to_owned(), format!("account-{ACCOUNT}")] {
        assert!(
            secret(&keystore, &namespaced(id, &legacy)).is_some(),
            "{legacy}: namespaced id written"
        );
        assert!(
            secret(&keystore, &legacy).is_none(),
            "{legacy}: keystore-wide id not written"
        );
    }
}

/// The smoke's adoption scenario end to end: a home that logged in on a
/// pre-Step-93 build (all three keys keystore-wide, no `home.id`) is opened
/// by this build and resumes — registry, account store and `mls.db` are
/// adopted under the new home's ids; the legacy entries stay.
#[tokio::test]
async fn a_pre_step_93_home_resumes_after_adopting_all_three_keys() {
    let server = MockServer::start();
    mount_shared_login_mocks(&server);
    let home = TempDir::new().expect("home");
    let first = memory_keystore();
    let app = open_app(home.path(), &first);
    let _login = mount_login_for_label(&server, &app.cookie_label(), "one");
    drop(login_via_manager(&app, &server).await);
    drop(app);
    let old_id = home_id_on_disk(home.path());
    // The pre-Step-93 shape: keys under the keystore-wide ids, no home id.
    let keystore = memory_keystore();
    for legacy in legacy_ids() {
        let key = secret(&first, &namespaced(&old_id, &legacy)).expect("key of the login");
        keystore.set_secret(legacy, key).expect("legacy keystore");
    }
    fs::remove_file(home.path().join(HOME_ID_FILE)).expect("no id before Step 93");

    let app = open_app(home.path(), &keystore);
    let new_id = app.home_id().to_owned();
    assert_ne!(new_id, old_id, "a home without an id gets a fresh one");
    let client = app
        .resume(ACCOUNT)
        .await
        .expect("resume adopts the account keys");
    assert_eq!(
        client.user_id().map(|u| u.as_str().to_owned()).as_deref(),
        Some(ACCOUNT)
    );
    for legacy in legacy_ids() {
        assert_eq!(
            secret(&keystore, &namespaced(&new_id, &legacy)),
            secret(&keystore, &legacy),
            "{legacy}: adopted under the new id, legacy entry kept"
        );
        assert!(
            secret(&keystore, &legacy).is_some(),
            "{legacy}: still present"
        );
    }
}
