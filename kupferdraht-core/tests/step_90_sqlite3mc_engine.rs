// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 90 — SQLite3MC engine swap (ADR-0021): the regression pins.
//!
//! The Step-88 harness (`spike/sqlite3mc/`, P1/P2) becomes the swap's
//! pins here: the build glue (lockfile without OpenSSL, `libsqlite3-sys`
//! as the vendored stand-in, plain `bundled`, the amalgamation pinned by
//! `vendor/sqlite3mc/PROVENANCE.txt`), the engine identity
//! (`sqlite3mc_version()` equals the vendored release), the five P2 byte
//! assertions on a production `SqliteStore` file in the `chacha20`
//! scheme, the encrypted-header control (a mis-linked SQLite is a red
//! test, not a plaintext file), the compile-time-default probe, the P1
//! debug-profile timing pin, wrong-key-fails-at-open (the first-read
//! verification, a recoverable `KupferdrahtError::Storage`), and the
//! `StorageEngineInfo` the CLI `status` line prints.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use kupferdraht::KupferdrahtError;
use kupferdraht::storage::db_key::DbKey;
use kupferdraht::storage::{SessionRecord, SqliteStore, StorageEngineInfo, Store};
use rusqlite::Connection;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

/// What every SQLite database file starts with when its header is readable.
const HEADER_MAGIC: &[u8; 16] = b"SQLite format 3\0";
/// Bytes the chacha20 scheme reserves at the end of every page (16-byte
/// nonce + 16-byte Poly1305 tag) — SQLite header byte 20.
const CHACHA20_RESERVED_BYTES: u8 = 32;
/// The plan's P1 bound (debug profile — `cc` compiles the C at `-O0`).
const P1_BUDGET: Duration = Duration::from_secs(1);
/// A token that must never appear in the file at rest.
const MARKER_TOKEN: &str = "kupferdraht-step90-plaintext-marker-3f9c1e";

fn workspace_path(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(rel)
}

fn workspace_file(rel: &str) -> String {
    let path = workspace_path(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// The quoted value of `#define NAME "value"` in the vendored header.
fn header_define(header: &str, name: &str) -> String {
    header
        .lines()
        .find_map(|line| {
            let rest = line.trim().strip_prefix("#define ")?.trim_start();
            let rest = rest.strip_prefix(name)?.trim_start();
            let rest = rest.strip_prefix('"')?;
            rest.split('"').next().map(str::to_string)
        })
        .unwrap_or_else(|| panic!("{name} is not a quoted #define in the vendored header"))
}

/// The value of a `key: value` line in `PROVENANCE.txt`.
fn provenance_value(provenance: &str, key: &str) -> String {
    provenance
        .lines()
        .find_map(|line| {
            line.strip_prefix(key)
                .and_then(|rest| rest.strip_prefix(':'))
                .map(|value| value.trim().to_string())
        })
        .unwrap_or_else(|| panic!("`{key}:` missing in vendor/sqlite3mc/PROVENANCE.txt"))
}

/// `(sqlite3mc version, sqlite version)` as pinned by the provenance record.
fn pinned_versions() -> (String, String) {
    let provenance = workspace_file("vendor/sqlite3mc/PROVENANCE.txt");
    (
        provenance_value(&provenance, "sqlite3mc-version"),
        provenance_value(&provenance, "sqlite-version"),
    )
}

/// A raw 32-byte key whose bytes are visible in a hexdump.
fn db_key(byte: u8) -> DbKey {
    DbKey::from_keystore_bytes(&[byte; 32]).expect("32-byte raw key")
}

fn key_a() -> DbKey {
    db_key(0x11)
}

fn key_b() -> DbKey {
    db_key(0x22)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The production keying batch mirrored for raw connections — SQLite3MC's
/// pragma order (ADR-0021): cipher parameters BEFORE `PRAGMA key`.
fn keying_sql(key: &DbKey, plaintext_header: bool) -> String {
    let header = if plaintext_header {
        "PRAGMA plaintext_header_size = 32; "
    } else {
        ""
    };
    format!(
        "PRAGMA cipher = 'chacha20'; {header}PRAGMA key = \"x'{}'\";",
        hex(key.key())
    )
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|w| w == needle)
}

fn sample_session() -> SessionRecord {
    SessionRecord {
        user_id: "alice@example.test".into(),
        device_id: "dev-1".into(),
        access_token: MARKER_TOKEN.into(),
        refresh_token: "real-refresh-cookie-payload".into(),
        cookie_label: Some("kupferdraht-cli".into()),
        api_version: 14,
        access_token_issued_at_ms: 1_700_000_000_000,
    }
}

fn fresh_path() -> (TempDir, PathBuf) {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("kupferdraht.db");
    (dir, path)
}

fn profile() -> &'static str {
    if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    }
}

// ── build glue ───────────────────────────────────────────────────────────────

/// Vendored OpenSSL leaves with SQLCipher: no `openssl*` crate in the tree.
#[test]
fn lockfile_openssl_left_the_tree() {
    let lock = workspace_file("Cargo.lock");
    for package in ["openssl", "openssl-src", "openssl-sys"] {
        assert!(
            !lock.contains(&format!("name = \"{package}\"\n")),
            "{package} must not be in Cargo.lock — vendored OpenSSL left with SQLCipher (ADR-0021)"
        );
    }
}

/// The `[[package]]` block of `package` in `Cargo.lock` (up to the blank line).
fn lock_block(lock: &str, package: &str) -> String {
    let needle = format!("name = \"{package}\"");
    let mut lines = lock.lines().skip_while(|l| l.trim() != needle);
    let mut block = Vec::new();
    for line in lines.by_ref() {
        if line.trim().is_empty() {
            break;
        }
        block.push(line);
    }
    assert!(!block.is_empty(), "{package} not in Cargo.lock");
    block.join("\n")
}

/// `libsqlite3-sys` resolves to the vendored stand-in (a path source, no
/// registry checksum) through the root `[patch.crates-io]` — a silent
/// fallback to crates.io's vanilla SQLite is a red test.
#[test]
fn lockfile_libsqlite3_sys_is_the_vendored_stand_in() {
    let block = lock_block(&workspace_file("Cargo.lock"), "libsqlite3-sys");
    assert!(
        !block.contains("source ="),
        "libsqlite3-sys must be a path source (vendor/libsqlite3-sys), got:\n{block}"
    );
    assert!(
        block.contains("version = \"0.35.0\""),
        "the stand-in keeps upstream's version so rusqlite 0.37 resolves to it:\n{block}"
    );
    let root = workspace_file("Cargo.toml");
    assert!(
        root.contains("[patch.crates-io]"),
        "root Cargo.toml lacks [patch.crates-io]"
    );
    assert!(
        root.contains("libsqlite3-sys = { path = \"vendor/libsqlite3-sys\" }"),
        "root Cargo.toml must patch libsqlite3-sys to vendor/libsqlite3-sys"
    );
}

/// Both `rusqlite` sites (dependency + dev-dependency) use plain
/// `bundled`; the SQLCipher/OpenSSL feature is gone.
#[test]
fn manifest_links_plain_bundled_sqlite() {
    let manifest = workspace_file("kupferdraht-core/Cargo.toml");
    assert!(
        !manifest.contains("bundled-sqlcipher"),
        "kupferdraht-core/Cargo.toml still names a SQLCipher feature"
    );
    let rusqlite_lines: Vec<&str> = manifest
        .lines()
        .filter(|l| l.trim_start().starts_with("rusqlite"))
        .collect();
    assert_eq!(
        rusqlite_lines.len(),
        2,
        "expected the dependency and the dev-dependency site: {rusqlite_lines:?}"
    );
    for line in rusqlite_lines {
        assert!(
            line.contains("\"bundled\""),
            "rusqlite site without `bundled`: {line}"
        );
    }
}

/// The vendored amalgamation is exactly what `PROVENANCE.txt` pins:
/// version defines in the header, SHA-256 of both files.
#[test]
fn vendored_amalgamation_matches_its_provenance_record() {
    let provenance = workspace_file("vendor/sqlite3mc/PROVENANCE.txt");
    let header = workspace_file("vendor/sqlite3mc/sqlite3mc_amalgamation.h");
    let (mc, sqlite) = pinned_versions();
    assert_eq!(
        header_define(&header, "SQLITE3MC_VERSION_STRING"),
        format!("SQLite3 Multiple Ciphers {mc}")
    );
    assert_eq!(header_define(&header, "SQLITE_VERSION"), sqlite);
    for file in ["sqlite3mc_amalgamation.c", "sqlite3mc_amalgamation.h"] {
        let bytes = std::fs::read(workspace_path(&format!("vendor/sqlite3mc/{file}")))
            .unwrap_or_else(|e| panic!("read vendor/sqlite3mc/{file}: {e}"));
        assert_eq!(
            hex(&Sha256::digest(&bytes)),
            provenance_value(&provenance, &format!("{file} sha256")),
            "vendor/sqlite3mc/{file} differs from its PROVENANCE.txt pin"
        );
    }
}

// ── engine identity ──────────────────────────────────────────────────────────

/// `SELECT sqlite3mc_version()` exists (vanilla SQLite and SQLCipher have
/// no such function) and names the vendored release; `sqlite_version()`
/// is the SQLite the amalgamation embeds.
#[test]
fn engine_identity_is_the_vendored_sqlite3mc() {
    let (_dir, path) = fresh_path();
    let store = SqliteStore::open(&path, &key_a()).expect("open");
    let conn = store.lock().expect("lock");
    let version: String = conn
        .query_row("SELECT sqlite3mc_version()", [], |r| r.get(0))
        .expect("sqlite3mc_version(): the linked engine must be SQLite3MC");
    let (mc, sqlite) = pinned_versions();
    assert!(
        version.starts_with("SQLite3 Multiple Ciphers"),
        "unexpected engine identity: {version:?}"
    );
    assert_eq!(version, format!("SQLite3 Multiple Ciphers {mc}"));
    let sqlite_version: String = conn
        .query_row("SELECT sqlite_version()", [], |r| r.get(0))
        .expect("sqlite_version()");
    assert_eq!(sqlite_version, sqlite);
}

/// The compile-time default cipher is chacha20 — so the explicit
/// `PRAGMA cipher = 'chacha20'` in the production batch pins intent, not
/// a deviation from the build (ADR-0021 Neutral Consequences).
#[test]
fn compile_time_default_cipher_is_chacha20() {
    let (_dir, path) = fresh_path();
    let conn = Connection::open(&path).expect("open");
    let cipher: String = conn
        .query_row("PRAGMA cipher", [], |r| r.get(0))
        .expect("PRAGMA cipher reports the compile-time default");
    assert_eq!(cipher, "chacha20");
}

// ── at rest (P2, on a production SqliteStore file) ──────────────────────────

/// The five P2 assertions on `kupferdraht.db` as `SqliteStore::open` writes it:
/// readable magic; SQLite's own page/WAL metadata in bytes 16..32 (the
/// chacha20 reserve in byte 20); schema SQL and the row are ciphertext;
/// the wrong key fails; the key reopens the file.
#[tokio::test]
async fn at_rest_chacha20_plaintext_header_and_ciphertext_payload() {
    let (_dir, path) = fresh_path();
    let page_size: u32 = {
        let store = SqliteStore::open(&path, &key_a()).expect("open");
        store
            .save_session(&sample_session())
            .await
            .expect("save session");
        let conn = store.lock().expect("lock");
        conn.query_row("PRAGMA page_size", [], |r| r.get(0))
            .expect("page_size")
    };
    let wal = PathBuf::from(format!("{}-wal", path.display()));
    assert!(
        !wal.exists(),
        "close must checkpoint + remove the WAL so the main file is the whole picture"
    );
    let bytes = std::fs::read(&path).expect("read kupferdraht.db");

    // 1 + 2: header readable, SQLite's metadata where SQLite puts it.
    assert_eq!(
        &bytes[..16],
        HEADER_MAGIC,
        "bytes 0..16 must be the SQLite magic"
    );
    assert_eq!(
        u32::from(u16::from_be_bytes([bytes[16], bytes[17]])),
        page_size,
        "bytes 16..18: page size"
    );
    assert_eq!(
        (bytes[18], bytes[19]),
        (2, 2),
        "bytes 18/19: WAL write/read version"
    );
    assert_eq!(
        bytes[20], CHACHA20_RESERVED_BYTES,
        "byte 20: reserved bytes per page for the chacha20 scheme"
    );
    assert_eq!(
        &bytes[21..24],
        &[64, 32, 32],
        "bytes 21..24: payload fractions"
    );

    // 4: page 1 beyond the 32 plaintext bytes (schema SQL) and the row are ciphertext.
    for needle in [MARKER_TOKEN.as_bytes(), b"CREATE TABLE", b"sessions"] {
        assert!(
            !contains(&bytes, needle),
            "plaintext {:?} found at rest",
            String::from_utf8_lossy(needle)
        );
    }

    // 5a: the wrong key fails — at open (first-read verification).
    assert!(
        SqliteStore::open(&path, &key_b()).is_err(),
        "the wrong key must not open the store"
    );

    // 5b: the key reopens the file and reads the row back.
    let store = SqliteStore::open(&path, &key_a()).expect("reopen with the key");
    let loaded = store
        .load_session("alice@example.test")
        .await
        .expect("load")
        .expect("session row present");
    assert_eq!(loaded.access_token, MARKER_TOKEN);
}

/// Control: without `plaintext_header_size` the header is ciphertext too —
/// proves the engine is engaged at all (the §51 "silently does not
/// encrypt" trap; a mis-linked SQLite fails here, not in production).
#[test]
fn header_is_encrypted_without_the_plaintext_header_pragma() {
    let (_dir, path) = fresh_path();
    {
        let conn = Connection::open(&path).expect("open");
        conn.execute_batch(&keying_sql(&key_a(), false))
            .expect("key without the header pragma");
        conn.execute_batch(&format!(
            "CREATE TABLE marker (id INTEGER PRIMARY KEY, body TEXT NOT NULL); \
             INSERT INTO marker (body) VALUES ('{MARKER_TOKEN}');"
        ))
        .expect("write marker");
    }
    let bytes = std::fs::read(&path).expect("read");
    assert_ne!(
        &bytes[..16],
        HEADER_MAGIC,
        "without plaintext_header_size the header must be ciphertext"
    );
    assert!(
        !contains(&bytes, MARKER_TOKEN.as_bytes()),
        "row leaked in plaintext"
    );
    assert!(
        !contains(&bytes, b"CREATE TABLE"),
        "schema leaked in plaintext"
    );
}

// ── keying ──────────────────────────────────────────────────────────────────

/// The wrong key surfaces at `open` as a recoverable `KupferdrahtError::Storage`
/// — never later mid-migration, never as a panic (runtime data is not a
/// code invariant).
#[tokio::test]
async fn wrong_key_fails_at_open_as_a_recoverable_storage_error() {
    let (_dir, path) = fresh_path();
    {
        let store = SqliteStore::open(&path, &key_a()).expect("open");
        store.save_session(&sample_session()).await.expect("save");
    }
    match SqliteStore::open(&path, &key_b()) {
        Err(KupferdrahtError::Storage(message)) => {
            assert!(
                message.contains("db key"),
                "the error names the key verification, got: {message}"
            );
        }
        Err(other) => panic!("expected KupferdrahtError::Storage, got {other:?}"),
        Ok(_) => panic!("the wrong key must not open the store"),
    }
}

/// P1 (CHANGELOG §52 re-examined, Step 88): a raw-key `PRAGMA key`, the
/// first write and a reopen + read each return in < 1 s in the debug
/// profile — the configuration under which SQLite3MC 2.3.5 hung.
#[test]
fn raw_key_pragma_returns_promptly_in_the_debug_profile() {
    let (_dir, path) = fresh_path();
    let key = key_a();

    let t0 = Instant::now();
    let conn = Connection::open(&path).expect("open");
    conn.execute_batch(&keying_sql(&key, true))
        .expect("PRAGMA key");
    let keyed = t0.elapsed();

    let t1 = Instant::now();
    conn.execute_batch(
        "CREATE TABLE marker (id INTEGER PRIMARY KEY, body TEXT NOT NULL); \
         INSERT INTO marker (body) VALUES ('p1');",
    )
    .expect("first write under the key");
    let written = t1.elapsed();
    drop(conn);

    let t2 = Instant::now();
    let conn = Connection::open(&path).expect("reopen");
    conn.execute_batch(&keying_sql(&key, true))
        .expect("PRAGMA key on reopen");
    let body: String = conn
        .query_row("SELECT body FROM marker", [], |r| r.get(0))
        .expect("read back");
    let reopened = t2.elapsed();
    assert_eq!(body, "p1");

    println!(
        "P1 chacha20 [{}]: PRAGMA key {keyed:?} | first write {written:?} | reopen+read {reopened:?}",
        profile()
    );
    assert!(
        keyed < P1_BUDGET,
        "PRAGMA key took {keyed:?} (budget {P1_BUDGET:?})"
    );
    assert!(written < P1_BUDGET, "first write took {written:?}");
    assert!(reopened < P1_BUDGET, "reopen + read took {reopened:?}");
}

// ── StorageEngineInfo (the CLI `status` line) ───────────────────────────────

/// The open store reports engine, cipher scheme and header mode read off
/// its live connection — what `kupferdraht-cli status` prints.
#[tokio::test]
async fn store_reports_the_storage_engine() {
    let (_dir, path) = fresh_path();
    let store = SqliteStore::open(&path, &key_a()).expect("open");
    let info: StorageEngineInfo = store
        .storage_engine()
        .await
        .expect("storage_engine")
        .expect("SqliteStore reports its engine");
    let (mc, _) = pinned_versions();
    assert_eq!(info.engine, "SQLite3 Multiple Ciphers");
    assert_eq!(info.version, mc);
    assert_eq!(info.cipher, "chacha20");
    assert_eq!(info.plaintext_header_size, 32);
}
