// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 111a — the browser's databases on sqlite-remote-vfs, local mode
//! (ADR-0035). TDD red. Runs in a **dedicated worker**
//! (`scripts/wasm-test.sh`); the test runner serves the page cross-origin
//! isolated, which the VFS needs.
//!
//! Replaces the OPFS pins of Steps 91, 93 and 96: the engine identity, the
//! wrong key at open, the expiry sweep, `global.db` and `mls.db`, the
//! refusal of a database without its key — now over the VFS, one
//! registration per database, in IndexedDB under the home id. New: the home
//! id is a keystore entry, the databases are named under it, and a database
//! another instance holds is `HeldElsewhere`.
#![cfg(target_arch = "wasm32")]

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use kupferdraht::KupferdrahtError;
use kupferdraht::keystore::{InMemoryKeyStore, IndexedDbKeyStore, SecureKeyStore};
use kupferdraht::storage::{
    BROWSER_HOME_ID_ENTRY, SessionRecord, TimelineEventRecord, browser_database_vfs,
    open_default_global_store, open_default_store, prepare_browser_database,
};
use openmls_sqlite_storage::{Codec, SqliteStorageProvider};
use rusqlite::{Connection, OpenFlags};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use wasm_bindgen_test::{console_log, wasm_bindgen_test, wasm_bindgen_test_configure};

wasm_bindgen_test_configure!(run_in_dedicated_worker);

/// The browser's base directory — the home layout names the databases.
const BASE_DIR: &str = "kupferdraht";
const KEY_A: [u8; 32] = [0x11; 32];
const KEY_B: [u8; 32] = [0x22; 32];
const MARKER_TOKEN: &str = "kupferdraht-step111a-marker";
const PROVENANCE: &str = include_str!("../../vendor/sqlite3mc/PROVENANCE.txt");
static HOME_ID: OnceLock<String> = OnceLock::new();

/// Open the browser's storage once for the worker through the binding —
/// the IndexedDB keystore, the instance lease, the home id — and return
/// the home id.
async fn home_id() -> String {
    if let Some(id) = HOME_ID.get() {
        return id.clone();
    }
    let kupferdraht = kupferdraht::wasm::Kupferdraht::open(JsValue::UNDEFINED, None)
        .await
        .expect("Kupferdraht.open in a cross-origin isolated dedicated worker");
    let id = kupferdraht.home_id();
    HOME_ID.get_or_init(|| id).clone()
}

fn account_db(account: &str) -> PathBuf {
    Path::new(BASE_DIR)
        .join("accounts")
        .join(account)
        .join("kupferdraht.db")
}

/// A keystore holding `key` as `account`'s `kupferdraht.db` key in this home.
async fn keystore_with(account: &str, key: [u8; 32]) -> Arc<dyn SecureKeyStore> {
    let ks = InMemoryKeyStore::new();
    ks.set_secret(
        format!("kupferdraht.{}.account-{account}", home_id().await),
        key.to_vec(),
    )
    .expect("seed the account key");
    Arc::new(ks)
}

/// Open `account`'s store with `key`, its database prepared first.
async fn open_account(
    account: &str,
    key: [u8; 32],
) -> Result<Arc<dyn kupferdraht::storage::Store>, KupferdrahtError> {
    prepare_browser_database(&account_db(account))
        .await
        .expect("prepare the account database");
    open_default_store(
        Path::new(BASE_DIR),
        &keystore_with(account, key).await,
        account,
    )
}

fn pinned_version() -> &'static str {
    PROVENANCE
        .lines()
        .find_map(|l| l.strip_prefix("sqlite3mc-version:"))
        .map(str::trim)
        .expect("PROVENANCE.txt names the SQLite3MC version")
}

fn sample_session(account: &str) -> SessionRecord {
    SessionRecord {
        user_id: account.into(),
        device_id: "dev-browser".into(),
        access_token: MARKER_TOKEN.into(),
        refresh_token: String::new(),
        cookie_label: Some("kupferdraht-step111a".into()),
        api_version: 14,
        access_token_issued_at_ms: 1_700_000_000_000,
    }
}

fn ephemeral_event(message_id: &str, expires_at_ms: Option<i64>) -> TimelineEventRecord {
    TimelineEventRecord {
        message_id: message_id.into(),
        room_id: "room-step111a@example.test".into(),
        sender_user_id: "bob@example.test".into(),
        sender_device_id: "dev-bob".into(),
        kind: "text".into(),
        body: "self-deleting body".into(),
        sent_at_ms: 1_700_000_000_000,
        delivery_status: "acknowledged".into(),
        reply_to_id: None,
        edited_at_ms: None,
        redacted_at_ms: None,
        expires_at_ms,
        ephemeral_ttl_ms: None,
    }
}

/// The names of the origin's IndexedDB databases (`indexedDB.databases()`,
/// called by reflection: web-sys keeps it behind its unstable APIs).
async fn indexeddb_names() -> Vec<String> {
    let factory = js_sys::Reflect::get(&js_sys::global(), &"indexedDB".into()).expect("indexedDB");
    let databases: js_sys::Function = js_sys::Reflect::get(&factory, &"databases".into())
        .expect("indexedDB.databases")
        .unchecked_into();
    let promise: js_sys::Promise = databases
        .call0(&factory)
        .expect("call databases()")
        .unchecked_into();
    let list = JsFuture::from(promise).await.expect("list the databases");
    js_sys::Array::from(&list)
        .iter()
        .filter_map(|info| {
            js_sys::Reflect::get(&info, &"name".into())
                .ok()
                .and_then(|n| n.as_string())
        })
        .collect()
}

// ── the home id and the namespace ───────────────────────────────────────────

/// The home id is the keystore entry `kupferdraht.home-id` — it lives as long
/// as the keys whose ids carry it — and a second open reads it back.
#[wasm_bindgen_test]
async fn the_home_id_is_a_keystore_entry_and_stable() {
    let id = home_id().await;
    assert!(
        id.len() == 32 && id.chars().all(|c| c.is_ascii_hexdigit()),
        "32 hex, got {id:?}"
    );
    let keystore = IndexedDbKeyStore::open()
        .await
        .expect("the IndexedDB keystore");
    let stored = keystore
        .get_secret(BROWSER_HOME_ID_ENTRY.to_owned())
        .expect("read the entry")
        .expect("the home id is stored");
    assert_eq!(String::from_utf8(stored).expect("utf-8"), id);
    let again = kupferdraht::wasm::Kupferdraht::open(JsValue::UNDEFINED, None)
        .await
        .expect("a second open");
    assert_eq!(again.home_id(), id, "stable across opens");
}

/// `Kupferdraht.open` without a `storage` option keeps `global.db` in IndexedDB,
/// in the VFS's local mode, under the home id.
#[wasm_bindgen_test]
async fn the_registry_lives_in_indexeddb_under_the_home_id() {
    let id = home_id().await;
    let names = indexeddb_names().await;
    let expected = format!("sqlite-remote-vfs-local/{id}/{BASE_DIR}/global.db");
    assert!(names.contains(&expected), "{expected} in {names:?}");
}

// ── an account's store over the VFS ─────────────────────────────────────────

/// The browser's engine is the vendored SQLite3MC release, in the chacha20
/// scheme with the 32-byte plaintext header — read off the live connection.
#[wasm_bindgen_test]
async fn store_reports_the_vendored_engine() {
    let store = open_account("engine@example.test", KEY_A)
        .await
        .expect("open over the VFS");
    let info = store
        .storage_engine()
        .await
        .expect("storage_engine")
        .expect("the SQLite store reports its engine");
    assert_eq!(info.engine, "SQLite3 Multiple Ciphers");
    assert_eq!(info.version, pinned_version());
    assert_eq!(info.cipher, "chacha20");
    assert_eq!(info.plaintext_header_size, 32);
}

/// Written, closed, reopened: the key reopens it, the wrong key does not,
/// and the database is an IndexedDB database under the home id.
#[wasm_bindgen_test]
async fn a_store_survives_its_connection_and_refuses_the_wrong_key() {
    let account = "reopen@example.test";
    {
        let store = open_account(account, KEY_A).await.expect("open");
        store
            .save_session(&sample_session(account))
            .await
            .expect("save session");
    }
    assert!(
        open_account(account, KEY_B).await.is_err(),
        "the wrong key must not open the store"
    );
    let store = open_account(account, KEY_A).await.expect("reopen");
    let loaded = store
        .load_session(account)
        .await
        .expect("load")
        .expect("session row present");
    assert_eq!(loaded.access_token, MARKER_TOKEN);
    let id = home_id().await;
    let name = format!(
        "sqlite-remote-vfs-local/{id}/{}",
        account_db(account).display()
    );
    assert!(indexeddb_names().await.contains(&name), "{name}");
}

/// The expiry sweep tombstones a due row and is idempotent (Step 89).
#[wasm_bindgen_test]
async fn ephemeral_sweep_tombstones_due_rows() {
    let store = open_account("sweep@example.test", KEY_A)
        .await
        .expect("open");
    store
        .insert_timeline_event(&ephemeral_event("due", Some(5_000)))
        .await
        .expect("insert due");
    store
        .insert_timeline_event(&ephemeral_event("future", Some(9_000)))
        .await
        .expect("insert future");
    let retired = store.expire_ephemeral_messages(6_000).await.expect("sweep");
    assert_eq!(retired.len(), 1);
    assert_eq!(retired[0].message_id, "due");
    assert!(
        store
            .expire_ephemeral_messages(6_000)
            .await
            .expect("second sweep")
            .is_empty(),
        "idempotent"
    );
}

/// A database another instance holds: the open is refused as
/// `HeldElsewhere`, never a silent second writer.
#[wasm_bindgen_test]
async fn a_database_held_elsewhere_is_held_elsewhere() {
    let account = "held@example.test";
    let path = account_db(account);
    let other = sqlite_remote_vfs::RemoteVfs::register_async(
        "step111a-other-instance",
        sqlite_remote_vfs::Config::local(home_id().await),
    )
    .await
    .expect("a second instance's VFS");
    let _held = Connection::open_with_flags_and_vfs(
        path.to_str().expect("utf-8 path"),
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
        other.encrypted_name().as_str(),
    )
    .expect("the other instance holds the database");

    let refused = open_account(account, KEY_A).await;

    assert!(
        matches!(&refused, Err(KupferdrahtError::HeldElsewhere(_))),
        "{:?}",
        refused.err()
    );
}

// ── Step 96 over the VFS ────────────────────────────────────────────────────

/// An account database that exists without its key is `KeyUnavailable`;
/// nothing is written to the keystore (ADR-0029).
#[wasm_bindgen_test]
async fn an_account_database_without_its_key_is_key_unavailable() {
    let account = "nokey@example.test";
    drop(
        open_account(account, KEY_A)
            .await
            .expect("create the database"),
    );
    let empty: Arc<dyn SecureKeyStore> = Arc::new(InMemoryKeyStore::new());

    let result = open_default_store(Path::new(BASE_DIR), &empty, account);

    assert!(
        matches!(result, Err(KupferdrahtError::KeyUnavailable(_))),
        "{:?}",
        result.err()
    );
    assert_eq!(
        empty
            .get_secret(format!("kupferdraht.{}.account-{account}", home_id().await))
            .expect("read"),
        None,
        "nothing written"
    );
}

/// `global.db` exists (Kupferdraht.open created it); a keystore without its key
/// is refused the same way.
#[wasm_bindgen_test]
async fn the_registry_without_its_key_is_key_unavailable() {
    home_id().await;
    let empty: Arc<dyn SecureKeyStore> = Arc::new(InMemoryKeyStore::new());

    let result = open_default_global_store(Path::new(BASE_DIR), &empty);

    assert!(
        matches!(result, Err(KupferdrahtError::KeyUnavailable(_))),
        "{:?}",
        result.err()
    );
}

// ── mls.db over the VFS ─────────────────────────────────────────────────────

#[derive(Default)]
struct JsonCodec;

impl Codec for JsonCodec {
    type Error = serde_json::Error;

    fn to_vec<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, Self::Error> {
        serde_json::to_vec(value)
    }

    fn from_slice<T: serde::de::DeserializeOwned>(slice: &[u8]) -> Result<T, Self::Error> {
        serde_json::from_slice(slice)
    }
}

fn key_connection(conn: &Connection, key: &[u8; 32]) -> rusqlite::Result<()> {
    let hex: String = key.iter().map(|b| format!("{b:02x}")).collect();
    conn.execute_batch(&format!(
        "PRAGMA cipher = 'chacha20'; PRAGMA plaintext_header_size = 32; PRAGMA key = \"x'{hex}'\";"
    ))?;
    conn.query_row("SELECT count(*) FROM sqlite_master", [], |r| {
        r.get::<_, i64>(0)
    })?;
    Ok(())
}

fn open_raw(path: &Path) -> Connection {
    let vfs = browser_database_vfs(path).expect("the database was prepared");
    Connection::open_with_flags_and_vfs(
        path.to_str().expect("utf-8 path"),
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
        vfs.as_str(),
    )
    .expect("open through the prepared VFS")
}

/// The `mls.db` provider migrates on a VFS database keyed like production,
/// reopens idempotently, and refuses the wrong key.
#[wasm_bindgen_test]
async fn mls_db_migrates_over_the_vfs() {
    home_id().await;
    let path = Path::new(BASE_DIR)
        .join("accounts")
        .join("mls@example.test")
        .join("mls.db");
    prepare_browser_database(&path).await.expect("prepare");
    {
        let conn = open_raw(&path);
        key_connection(&conn, &KEY_A).expect("key a fresh mls.db");
        let mut provider: SqliteStorageProvider<JsonCodec, Connection> =
            SqliteStorageProvider::new(conn);
        provider.run_migrations().expect("openmls migrations");
    }
    {
        let conn = open_raw(&path);
        assert!(
            key_connection(&conn, &KEY_B).is_err(),
            "the wrong key must not open mls.db"
        );
    }
    let conn = open_raw(&path);
    key_connection(&conn, &KEY_A).expect("key a reopens mls.db");
    let tables: i64 = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name LIKE 'openmls_%'",
            [],
            |r| r.get(0),
        )
        .expect("count openmls tables");
    assert!(tables > 0, "the openmls schema persisted");
}

// ── survival across browser sessions ────────────────────────────────────────

/// A run counter in a probe account, not reset at start: headless runs use
/// a fresh profile and see `1`; with a persistent profile a second run
/// reads `2` — the store survives a full browser session.
#[wasm_bindgen_test]
async fn run_counter_persists_with_a_persistent_profile() {
    let store = open_account("reload-probe@example.test", KEY_A)
        .await
        .expect("open probe store");
    let before: i64 = store
        .get_user_property("runs")
        .await
        .expect("read runs")
        .and_then(|p| p.value.parse().ok())
        .unwrap_or(0);
    store
        .upsert_user_property(&kupferdraht::storage::UserPropertyEntry {
            key: "runs".into(),
            value: (before + 1).to_string(),
            last_synced_at_ms: 0,
        })
        .await
        .expect("bump runs");
    console_log!("reload probe: runs before = {before}, now {}", before + 1);
}
