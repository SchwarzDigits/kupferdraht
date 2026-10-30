// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 108 spike — Kupferdraht's browser databases on `sqlite-remote-vfs`
//! (`docs/plan/archive/108-remote-vfs-spike.md`).
//!
//! Needs a running `sqlite-remote-server` and its URL at compile time in
//! `KUPFERDRAHT_REMOTE_VFS_URL` (e.g. `ws://127.0.0.1:18180/v1/ws`); without it
//! every test returns at once. The test page is served from 127.0.0.1, so
//! the server must allow that origin.

#![cfg(target_arch = "wasm32")]

use std::ffi::CString;
use std::path::Path;
use std::sync::Arc;

use kupferdraht::storage::db_key::DbKey;
use kupferdraht::storage::{SqliteStore, Store, TimelineEventRecord};
use rusqlite::{Connection, OpenFlags};
use sqlite_remote_vfs::{
    Algorithm, Cache, Config, Load, RemoteVfs, Server, Signer, Store as VfsStore,
};
use wasm_bindgen_test::{console_log, wasm_bindgen_test, wasm_bindgen_test_configure};

// A commit blocks in `Atomics.wait`, which browsers refuse on the main
// thread — and Kupferdraht's browser storage lives in a dedicated worker anyway.
wasm_bindgen_test_configure!(run_in_dedicated_worker);

const SERVER: Option<&str> = option_env!("KUPFERDRAHT_REMOTE_VFS_URL");

/// An Ed25519 signer from a random seed — one subject per test run.
struct SeedSigner(ed25519_dalek::SigningKey);

impl Signer for SeedSigner {
    fn algorithm(&self) -> Algorithm {
        Algorithm::Ed25519
    }

    fn public_key(&self) -> Vec<u8> {
        self.0.verifying_key().to_bytes().to_vec()
    }

    fn sign(&self, message: &[u8]) -> Vec<u8> {
        use ed25519_dalek::Signer as _;
        self.0.sign(message).to_bytes().to_vec()
    }
}

fn random_signer() -> Arc<dyn Signer> {
    let seed: [u8; 32] = rand::random();
    Arc::new(SeedSigner(ed25519_dalek::SigningKey::from_bytes(&seed)))
}

fn unique(prefix: &str) -> String {
    let n: u32 = rand::random();
    format!("{prefix}-{n:08x}")
}

fn db_key() -> DbKey {
    DbKey::from_keystore_bytes(&[0x5a; 32]).expect("32-byte key")
}

async fn register(url: &str, signer: &Arc<dyn Signer>, takeover: bool) -> RemoteVfs {
    let mut config = Config::new(VfsStore::Server(Server::new(url, signer.clone())));
    config.takeover = takeover;
    config.load = Load::Preload;
    RemoteVfs::register_async(&unique("kupferdraht-remote"), config)
        .await
        .expect("register the remote VFS")
}

/// Make `vfs`'s SQLite3MC wrapper the default, so Kupferdraht's stores — which
/// open with `Connection::open(path)` — land on it.
fn make_default(vfs: &RemoteVfs) {
    let name = CString::new(vfs.name()).expect("vfs name");
    let rc = rusqlite::ffi::sqlite3mc_wrap_vfs(&name, true);
    assert_eq!(
        rc,
        rusqlite::ffi::SQLITE_OK,
        "wrap {} with SQLite3MC",
        vfs.name()
    );
}

fn text_row(n: u32) -> TimelineEventRecord {
    TimelineEventRecord {
        message_id: format!("m-{n:05}"),
        room_id: "room@spike.test".into(),
        sender_user_id: "u@spike.test".into(),
        sender_device_id: "dev".into(),
        kind: "text".into(),
        body: format!("message {n}"),
        sent_at_ms: i64::from(n),
        delivery_status: "received".into(),
        ..TimelineEventRecord::default()
    }
}

/// Q1: one wasm module links the VFS with Kupferdraht's SQLite (sqlite-wasm-rs
/// 0.6.1 + SQLite3MC); a keyed database round-trips through a fresh
/// registration of the same key.
#[wasm_bindgen_test]
async fn a_keyed_database_round_trips_through_the_remote_vfs() {
    let Some(url) = SERVER else { return };
    let signer = random_signer();
    let db = unique("roundtrip") + ".db";
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE;
    {
        let vfs = register(url, &signer, true).await;
        let conn = Connection::open_with_flags_and_vfs(&db, flags, vfs.encrypted_name().as_str())
            .expect("open");
        conn.pragma_update(None, "key", "spike-key").expect("key");
        conn.execute_batch("CREATE TABLE t(x TEXT); INSERT INTO t VALUES ('hello');")
            .expect("write");
        drop(conn);
    }
    let vfs = register(url, &signer, true).await;
    let conn = Connection::open_with_flags_and_vfs(&db, flags, vfs.encrypted_name().as_str())
        .expect("reopen");
    conn.pragma_update(None, "key", "spike-key").expect("key");
    let x: String = conn
        .query_row("SELECT x FROM t", [], |row| row.get(0))
        .expect("read back");
    assert_eq!(x, "hello");
}

/// Q2 + Q3: Kupferdraht's `SqliteStore` runs unchanged on the remote VFS —
/// migrations, pragmas, one transaction per event — and survives a reopen.
/// Prints the commit latency.
#[wasm_bindgen_test]
async fn kupferdraht_store_runs_on_the_remote_vfs() {
    const EVENTS: u32 = 100;
    let Some(url) = SERVER else { return };
    let signer = random_signer();
    let db = unique("account") + "/kupferdraht.db";
    {
        let vfs = register(url, &signer, true).await;
        make_default(&vfs);
        let open_start = js_sys::Date::now();
        let store = SqliteStore::open(Path::new(&db), &db_key()).expect("open store");
        let opened_ms = js_sys::Date::now() - open_start;
        let write_start = js_sys::Date::now();
        for n in 0..EVENTS {
            store
                .insert_timeline_event(&text_row(n))
                .await
                .expect("insert");
        }
        let written_ms = js_sys::Date::now() - write_start;
        let stats = vfs.stats();
        console_log!(
            "spike: open+migrate {opened_ms:.0} ms; {EVENTS} event commits {written_ms:.0} ms \
             = {:.1} ms/commit; vfs commits {} (max {:?}), blocks {}",
            written_ms / f64::from(EVENTS),
            stats.commits,
            stats.max_commit_time,
            stats.committed_blocks
        );
    }
    let vfs = register(url, &signer, true).await;
    make_default(&vfs);
    let reopen_start = js_sys::Date::now();
    let store = SqliteStore::open(Path::new(&db), &db_key()).expect("reopen store");
    let rows = store
        .load_timeline_events("room@spike.test")
        .await
        .expect("load");
    console_log!(
        "spike: reopen + load {} rows {:.0} ms",
        rows.len(),
        js_sys::Date::now() - reopen_start
    );
    assert_eq!(
        rows.len(),
        EVENTS as usize,
        "every committed event came back"
    );
}

/// Q4: a second client with the same key takes the database over; the
/// first one's next write fails, and reopening without takeover reports
/// the database as held elsewhere (`SQLITE_BUSY`).
#[wasm_bindgen_test]
async fn a_takeover_fences_the_older_client() {
    let Some(url) = SERVER else { return };
    let signer = random_signer();
    let db = unique("fence") + ".db";
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE;

    let old_vfs = register(url, &signer, true).await;
    let old = Connection::open_with_flags_and_vfs(&db, flags, old_vfs.encrypted_name().as_str())
        .expect("old opens");
    old.pragma_update(None, "key", "spike-key").expect("key");
    old.execute_batch("CREATE TABLE t(x INTEGER); INSERT INTO t VALUES (1);")
        .expect("old writes");

    let new_vfs = register(url, &signer, true).await;
    let new = Connection::open_with_flags_and_vfs(&db, flags, new_vfs.encrypted_name().as_str())
        .expect("new takes over");
    new.pragma_update(None, "key", "spike-key").expect("key");
    new.execute_batch("INSERT INTO t VALUES (2);")
        .expect("new writes");

    let fenced = old.execute_batch("INSERT INTO t VALUES (3);");
    console_log!("spike: the fenced client's write → {fenced:?}");
    assert!(fenced.is_err(), "the older client can no longer commit");

    drop(old);
    let probe_vfs = register(url, &signer, false).await;
    let probe =
        Connection::open_with_flags_and_vfs(&db, flags, probe_vfs.encrypted_name().as_str())
            .and_then(|conn| {
                conn.pragma_update(None, "key", "spike-key")?;
                conn.query_row("SELECT count(*) FROM t", [], |row| row.get::<_, i64>(0))
            });
    console_log!("spike: reopening without takeover while held → {probe:?}");
    assert!(probe.is_err(), "the database is held by the newer client");
}

// ── drain-pattern measurement (Step 108, second round) ─────────────────

/// How a drain of one page commits.
#[derive(Clone, Copy, Debug)]
enum Pattern {
    /// Today: per notification the MLS state, the event, the cursor — three
    /// transactions.
    Today,
    /// Lever 1: the cursor in the event's transaction — two per
    /// notification.
    CursorInEventTx,
    /// Levers 1 + 2: one MLS transaction and one data transaction for the
    /// whole page.
    Page,
}

const PAGE: u32 = 100;

async fn register_with(url: &str, signer: &Arc<dyn Signer>, cache: Cache) -> RemoteVfs {
    let mut server = Server::new(url, signer.clone());
    server.cache = cache;
    let mut config = Config::new(VfsStore::Server(server));
    config.takeover = true;
    config.load = Load::Preload;
    RemoteVfs::register_async(&unique("kupferdraht-remote"), config)
        .await
        .expect("register the remote VFS")
}

/// `kupferdraht.db` with Kupferdraht's real schema (created through `SqliteStore`),
/// reopened as a plain connection on the same VFS.
fn open_kupferdraht_db(vfs: &RemoteVfs, db: &str) -> Connection {
    make_default(vfs);
    drop(SqliteStore::open(Path::new(db), &db_key()).expect("create the schema"));
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE;
    let conn = Connection::open_with_flags_and_vfs(db, flags, vfs.encrypted_name().as_str())
        .expect("reopen kupferdraht.db");
    apply_kupferdraht_key(&conn);
    conn
}

/// Kupferdraht's own keying (`storage::apply_whole_db_key`): ChaCha20, a 32-byte
/// plaintext header, the raw key of `db_key()`.
fn apply_kupferdraht_key(conn: &Connection) {
    let hex: String = db_key().key().iter().map(|b| format!("{b:02x}")).collect();
    conn.execute_batch(&format!(
        "PRAGMA cipher = 'chacha20'; PRAGMA plaintext_header_size = 32; PRAGMA key = \"x'{hex}'\";"
    ))
    .expect("apply the key");
}

/// A stand-in for `mls.db`: three ~2 KiB records rewritten per message.
fn open_mls_db(vfs: &RemoteVfs, db: &str) -> Connection {
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE;
    let conn = Connection::open_with_flags_and_vfs(db, flags, vfs.encrypted_name().as_str())
        .expect("open mls.db");
    apply_kupferdraht_key(&conn);
    conn.execute_batch("CREATE TABLE IF NOT EXISTS state(k INTEGER PRIMARY KEY, v BLOB);")
        .expect("schema");
    conn
}

fn blob() -> Vec<u8> {
    (0..2048).map(|_| rand::random::<u8>()).collect()
}

fn mls_writes(conn: &Connection) {
    for k in 0..3 {
        conn.execute(
            "INSERT INTO state(k, v) VALUES (?1, ?2) ON CONFLICT(k) DO UPDATE SET v = excluded.v",
            rusqlite::params![k, blob()],
        )
        .expect("mls write");
    }
}

fn event_write(conn: &Connection, n: u32) {
    conn.execute(
        "INSERT INTO timeline_events (message_id, room_id, sender_user_id, sender_device_id,
             kind, body, sent_at_ms, delivery_status, reply_to_id, expires_at_ms, ephemeral_ttl_ms)
         VALUES (?1, 'room@spike.test', 'u@spike.test', 'dev', 'text', ?2, ?3, 'received',
                 NULL, NULL, NULL)",
        rusqlite::params![
            format!("m-{n:05}"),
            format!("message {n}").into_bytes(),
            i64::from(n)
        ],
    )
    .expect("event write");
}

fn cursor_write(conn: &Connection, n: u32) {
    conn.execute(
        "INSERT INTO sync_state (id, sync_marker_uuid, last_event_id, last_connected_at_ms,
             last_key_package_check_at_ms) VALUES (1, NULL, ?1, NULL, NULL)
         ON CONFLICT (id) DO UPDATE SET last_event_id = excluded.last_event_id",
        rusqlite::params![format!("n-{n:05}")],
    )
    .expect("cursor write");
}

fn in_tx(conn: &Connection, body: impl FnOnce(&Connection)) {
    conn.execute_batch("BEGIN IMMEDIATE").expect("begin");
    body(conn);
    conn.execute_batch("COMMIT").expect("commit");
}

fn drain(pattern: Pattern, kupferdraht: &Connection, mls: &Connection) {
    match pattern {
        Pattern::Today => {
            for n in 0..PAGE {
                in_tx(mls, mls_writes);
                in_tx(kupferdraht, |c| event_write(c, n));
                in_tx(kupferdraht, |c| cursor_write(c, n));
            }
        }
        Pattern::CursorInEventTx => {
            for n in 0..PAGE {
                in_tx(mls, mls_writes);
                in_tx(kupferdraht, |c| {
                    event_write(c, n);
                    cursor_write(c, n);
                });
            }
        }
        Pattern::Page => {
            in_tx(mls, |c| {
                for _ in 0..PAGE {
                    mls_writes(c);
                }
            });
            in_tx(kupferdraht, |c| {
                for n in 0..PAGE {
                    event_write(c, n);
                }
                cursor_write(c, PAGE - 1);
            });
        }
    }
}

async fn measure(url: &str, pattern: Pattern, cache: Cache, label: &str) {
    let signer = random_signer();
    let account = unique("account");
    let (kupferdraht_db, mls_db) = (
        format!("{account}/kupferdraht.db"),
        format!("{account}/mls.db"),
    );
    let kupferdraht_vfs = register_with(url, &signer, cache.clone()).await;
    let mls_vfs = register_with(url, &signer, cache.clone()).await;
    let kupferdraht = open_kupferdraht_db(&kupferdraht_vfs, &kupferdraht_db);
    let mls = open_mls_db(&mls_vfs, &mls_db);
    let (s0, m0) = (kupferdraht_vfs.stats(), mls_vfs.stats());

    let start = js_sys::Date::now();
    drain(pattern, &kupferdraht, &mls);
    let ms = js_sys::Date::now() - start;

    let (s1, m1) = (kupferdraht_vfs.stats(), mls_vfs.stats());
    let commits = (s1.commits - s0.commits) + (m1.commits - m0.commits);
    let kib = ((s1.committed_bytes - s0.committed_bytes)
        + (m1.committed_bytes - m0.committed_bytes))
        / 1024;
    console_log!(
        "spike-drain | {pattern:?} | {label} | {ms:.0} ms | {:.1} ms/notification | {commits} commits | {kib} KiB",
        ms / f64::from(PAGE)
    );
    drop((kupferdraht, mls));

    // Reopen: a fresh registration of the same key reads the page back.
    let again = register_with(url, &signer, cache).await;
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE;
    let reopen_start = js_sys::Date::now();
    let conn = Connection::open_with_flags_and_vfs(
        &kupferdraht_db,
        flags,
        again.encrypted_name().as_str(),
    )
    .expect("reopen");
    apply_kupferdraht_key(&conn);
    let rows: i64 = conn
        .query_row("SELECT count(*) FROM timeline_events", [], |row| row.get(0))
        .expect("count");
    let fetches = again.stats().fetched_blocks;
    console_log!(
        "spike-reopen | {pattern:?} | {label} | {:.0} ms | {rows} rows | {fetches} blocks fetched from the server",
        js_sys::Date::now() - reopen_start
    );
    assert_eq!(rows, i64::from(PAGE));
}

/// Second round: the three drain patterns, without and with the VFS's
/// local copy in IndexedDB, and the reopen from it.
#[wasm_bindgen_test]
async fn measure_drain_patterns() {
    let Some(url) = SERVER else { return };
    for pattern in [Pattern::Today, Pattern::CursorInEventTx, Pattern::Page] {
        measure(url, pattern, Cache::None, "no local copy").await;
        measure(url, pattern, Cache::Browser, "local copy").await;
    }
}
