// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 140 — the plaintext guard of sqlite-remote-vfs 0.6.0 in the local
//! mode. A connection that skips `PRAGMA key` — what a build with plain
//! SQLite instead of SQLite3MC, or a lost key, would do — cannot write
//! page 1 in the clear: the VFS answers `SQLITE_PERM` (its own message
//! stays inside the VFS), and a keyed open of the same database works
//! afterwards. A database already stored in the clear — written through a
//! VFS without the guard — fails Kupferdraht's open with `Storage`, never
//! with `DeviceRetired`: the VFS answers another device's storage slot
//! with the same code, and `DeviceRetired` makes a login forget the
//! account's keys (Step 121). Runs in a **dedicated worker**
//! (`scripts/wasm-test.sh`), like the Step 111a storage tests.
#![cfg(target_arch = "wasm32")]

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use kupferdraht::KupferdrahtError;
use kupferdraht::keystore::{InMemoryKeyStore, SecureKeyStore};
use kupferdraht::storage::{browser_database_vfs, open_default_store, prepare_browser_database};
use rusqlite::{Connection, OpenFlags};
use sqlite_remote_vfs::{Config, RemoteVfs};
use wasm_bindgen::JsValue;
use wasm_bindgen_test::{console_log, wasm_bindgen_test, wasm_bindgen_test_configure};

wasm_bindgen_test_configure!(run_in_dedicated_worker);

const BASE_DIR: &str = "kupferdraht";
const KEY: [u8; 32] = [0x33; 32];
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

/// A keystore holding `KEY` as `account`'s `kupferdraht.db` key in this home.
async fn keystore_with(account: &str) -> Arc<dyn SecureKeyStore> {
    let ks = InMemoryKeyStore::new();
    ks.set_secret(
        format!("kupferdraht.{}.account-{account}", home_id().await),
        KEY.to_vec(),
    )
    .expect("seed the account key");
    Arc::new(ks)
}

fn open_with(path: &Path, vfs: &str) -> Connection {
    Connection::open_with_flags_and_vfs(
        path.to_str().expect("utf-8 path"),
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
        vfs,
    )
    .expect("open through the VFS")
}

fn key_connection(conn: &Connection) -> rusqlite::Result<()> {
    let hex: String = KEY.iter().map(|b| format!("{b:02x}")).collect();
    conn.execute_batch(&format!(
        "PRAGMA cipher = 'chacha20'; PRAGMA plaintext_header_size = 32; PRAGMA key = \"x'{hex}'\";"
    ))
}

/// A write without a key is refused by the VFS with `SQLITE_PERM`.
#[wasm_bindgen_test]
async fn a_plaintext_write_is_refused_and_the_keyed_open_still_works() {
    home_id().await;
    let path = account_db("plaintext@example.test");
    prepare_browser_database(&path)
        .await
        .expect("prepare the account database");
    let vfs = browser_database_vfs(&path).expect("the database was prepared");

    let plain = open_with(&path, &vfs);
    let code = match plain.execute_batch("CREATE TABLE t (x INTEGER)") {
        Ok(()) => panic!("a plaintext page 1 reached the store"),
        Err(e) => e.sqlite_error_code(),
    };
    assert_eq!(
        code,
        Some(rusqlite::ErrorCode::PermissionDenied),
        "the VFS refuses a plaintext page 1 with SQLITE_PERM"
    );
    drop(plain);

    let keyed = open_with(&path, &vfs);
    key_connection(&keyed).expect("the key applies");
    keyed
        .execute_batch("CREATE TABLE t (x INTEGER); INSERT INTO t VALUES (1);")
        .expect("the keyed write succeeds after the refusal");
    let rows: i64 = keyed
        .query_row("SELECT count(*) FROM t", [], |r| r.get(0))
        .expect("the keyed read");
    assert_eq!(rows, 1);
}

/// A database stored in the clear — what a build without the guard,
/// linked against plain SQLite, would leave — fails Kupferdraht's open as
/// a storage error, not as a retired device.
#[wasm_bindgen_test]
async fn a_database_stored_in_the_clear_is_a_storage_error_not_a_retired_device() {
    let home = home_id().await;
    let account = "stored-plain@example.test";
    let path = account_db(account);
    let unguarded = RemoteVfs::register_async("step140-unguarded", Config::local(home))
        .await
        .expect("register a VFS without the guard");
    {
        let plain = open_with(&path, unguarded.name());
        plain
            .execute_batch("CREATE TABLE t (x INTEGER); INSERT INTO t VALUES (1);")
            .expect("the VFS without the guard stores plaintext");
    }

    prepare_browser_database(&path)
        .await
        .expect("prepare the account database");
    let opened = open_default_store(Path::new(BASE_DIR), &keystore_with(account).await, account);
    match opened {
        Err(KupferdrahtError::Storage(message)) => console_log!("refused: {message}"),
        Err(other) => {
            panic!("a database stored in the clear must be a storage error, got {other:?}")
        }
        Ok(_) => panic!("a database stored in the clear opened"),
    }
}
