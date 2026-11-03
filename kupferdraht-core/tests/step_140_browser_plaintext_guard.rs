// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 140 — the plaintext guard of sqlite-remote-vfs 0.6.0 in the local
//! mode: a connection on a prepared database that skips `PRAGMA key` —
//! what a build with plain SQLite instead of SQLite3MC, or a lost key,
//! would do — cannot write page 1 in the clear; the keyed open of the same
//! database afterwards works, since the refused transaction rolled back.
//! Runs in a **dedicated worker** (`scripts/wasm-test.sh`), like the Step
//! 111a storage tests.
#![cfg(target_arch = "wasm32")]

use std::path::{Path, PathBuf};

use kupferdraht::storage::{browser_database_vfs, prepare_browser_database};
use rusqlite::{Connection, OpenFlags};
use wasm_bindgen::JsValue;
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

wasm_bindgen_test_configure!(run_in_dedicated_worker);

const BASE_DIR: &str = "kupferdraht";
const KEY: [u8; 32] = [0x33; 32];

fn account_db(account: &str) -> PathBuf {
    Path::new(BASE_DIR)
        .join("accounts")
        .join(account)
        .join("kupferdraht.db")
}

/// Open the browser's storage for the worker through the binding (the
/// IndexedDB keystore, the instance lease, the home id) and prepare
/// `account`'s database in the local mode.
async fn prepared_database(account: &str) -> PathBuf {
    kupferdraht::wasm::Kupferdraht::open(JsValue::UNDEFINED, None)
        .await
        .expect("Kupferdraht.open in a cross-origin isolated dedicated worker");
    let path = account_db(account);
    prepare_browser_database(&path)
        .await
        .expect("prepare the account database");
    path
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

fn key_connection(conn: &Connection) -> rusqlite::Result<()> {
    let hex: String = KEY.iter().map(|b| format!("{b:02x}")).collect();
    conn.execute_batch(&format!(
        "PRAGMA cipher = 'chacha20'; PRAGMA plaintext_header_size = 32; PRAGMA key = \"x'{hex}'\";"
    ))
}

/// A write without a key is refused by the VFS, and the error says why.
#[wasm_bindgen_test]
async fn a_plaintext_write_is_refused_and_the_keyed_open_still_works() {
    let path = prepared_database("plaintext@example.test").await;

    let plain = open_raw(&path);
    let refused = plain.execute_batch("CREATE TABLE t (x INTEGER)");
    let message = match refused {
        Ok(()) => panic!("a plaintext page 1 reached the store"),
        Err(e) => e.to_string(),
    };
    assert!(
        message.contains("plaintext refused"),
        "the error names the guard: {message}"
    );
    drop(plain);

    let keyed = open_raw(&path);
    key_connection(&keyed).expect("the key applies");
    keyed
        .execute_batch("CREATE TABLE t (x INTEGER); INSERT INTO t VALUES (1);")
        .expect("the keyed write succeeds after the refusal");
    let rows: i64 = keyed
        .query_row("SELECT count(*) FROM t", [], |r| r.get(0))
        .expect("the keyed read");
    assert_eq!(rows, 1);
}
