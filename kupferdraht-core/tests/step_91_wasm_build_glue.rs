// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 91 — SQLite3MC in the browser (ADR-0022): the native half of the
//! pins. The build glue that makes the wasm target run the same store
//! (one amalgamation, the fork's wasm arm — ADR-0042 —, the CI job), and the
//! cross-target fixture the browser test opens: a `kupferdraht.db` written by
//! the native `SqliteStore` under a known key, closed in rollback-journal
//! mode (the OPFS `sahpool` VFS has no shared-memory WAL).

use std::path::{Path, PathBuf};

use kupferdraht::storage::db_key::DbKey;
use kupferdraht::storage::{SessionRecord, SqliteStore, Store, TimelineEventRecord};

/// The fixture key — fixture material, visible on purpose.
const FIXTURE_KEY: [u8; 32] = [0x11; 32];
const FIXTURE_ACCOUNT: &str = "alice@example.test";
/// A token that must never appear in the file at rest. The committed
/// fixture was generated before the rename to Kupferdraht and carries this
/// exact value (the bytes are the pin, see `generate_step_91_fixture`).
const MARKER_TOKEN: &str = "sodium-step91-plaintext-marker-7a1d44";

fn workspace_path(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(rel)
}

fn workspace_file(rel: &str) -> String {
    let path = workspace_path(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn fixture_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/step-91/native-chacha20.db")
}

fn fixture_key() -> DbKey {
    DbKey::from_keystore_bytes(&FIXTURE_KEY).expect("32-byte fixture key")
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|w| w == needle)
}

/// The `[target.'cfg(target_arch = "wasm32")'.dependencies]` block of a
/// manifest (up to the next section header).
fn wasm_dependency_block(manifest: &str) -> String {
    let start = manifest
        .find("[target.'cfg(target_arch = \"wasm32\")'.dependencies]")
        .expect("manifest has a wasm32 dependencies block");
    let rest = &manifest[start..];
    let end = rest[1..].find("\n[").map_or(rest.len(), |i| i + 1);
    rest[..end].to_string()
}

/// The `[target.'cfg(not(target_arch = "wasm32"))'.dependencies]` block.
fn native_dependency_block(manifest: &str) -> String {
    let start = manifest
        .find("[target.'cfg(not(target_arch = \"wasm32\"))'.dependencies]")
        .expect("manifest has a native-only dependencies block");
    let rest = &manifest[start..];
    let end = rest[1..].find("\n[").map_or(rest.len(), |i| i + 1);
    rest[..end].to_string()
}

// ── one amalgamation on every target ────────────────────────────────────────

/// The wasm engine is `sqlite-wasm-rs` with the `sqlite3mc` feature, and the
/// store dependencies are no longer native-only.
#[test]
fn manifest_runs_the_sqlite_store_on_wasm() {
    let manifest = workspace_file("kupferdraht-core/Cargo.toml");
    let wasm = wasm_dependency_block(&manifest);
    assert!(
        wasm.contains("sqlite-wasm-rs") && wasm.contains("\"sqlite3mc\""),
        "the wasm block must name sqlite-wasm-rs with the sqlite3mc feature:\n{wasm}"
    );
    assert!(
        wasm.contains("sqlite-remote-vfs"),
        "the wasm block must name sqlite-remote-vfs (Step 111, ADR-0035):\n{wasm}"
    );
    assert!(
        wasm.contains("time") && wasm.contains("\"wasm-bindgen\""),
        "refinery's applied_on stamp needs time's wasm-bindgen backend on wasm:\n{wasm}"
    );
    let native = native_dependency_block(&manifest);
    for dep in ["rusqlite", "refinery", "openmls_sqlite_storage"] {
        assert!(
            !native.lines().any(|l| l.trim_start().starts_with(dep)),
            "{dep} must build on every target — not in the native-only block:\n{native}"
        );
    }
    // The per-record AES-GCM path left with the IndexedDB store; AES-GCM is
    // back only to seal recovery data (Step 112, ADR-0035) — never in the
    // storage.
    let storage = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/storage");
    for (file, text) in rust_sources(&storage) {
        assert!(
            !text.contains("aes_gcm"),
            "{file}: the per-record aes-gcm path left with the IndexedDB store"
        );
    }
}

/// Every `.rs` file under `dir`, with its path.
fn rust_sources(dir: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).expect("read dir") {
        let path = entry.expect("entry").path();
        if path.is_dir() {
            out.extend(rust_sources(&path));
        } else if path.extension().is_some_and(|e| e == "rs") {
            let text = std::fs::read_to_string(&path).expect("read source");
            out.push((path.display().to_string(), text));
        }
    }
    out
}

/// `sqlite-wasm-rs` compiles the vendored, PROVENANCE-pinned amalgamation —
/// not its bundled copy (byte-identical today, but the pin is the config).
#[test]
fn cargo_config_points_the_wasm_engine_at_the_vendored_amalgamation() {
    let config = workspace_file(".cargo/config.toml");
    assert!(
        config.contains("SQLITE_WASM_RS_SOURCE_DIR")
            && config.contains("\"vendor/sqlite3mc\"")
            && config.contains("relative = true"),
        "root .cargo/config.toml must set SQLITE_WASM_RS_SOURCE_DIR to vendor/sqlite3mc (relative):\n{config}"
    );
}

/// The lockfile resolves the wasm engine on the line ADR-0022 names, and
/// the storage VFS on its first release (Step 111, ADR-0035).
#[test]
fn lockfile_pins_the_wasm_engine_and_vfs_lines() {
    let lock = workspace_file("Cargo.lock");
    let version_of = |package: &str| -> String {
        let needle = format!("name = \"{package}\"");
        let mut lines = lock.lines().skip_while(|l| l.trim() != needle);
        lines.next().expect(&needle);
        lines
            .next()
            .and_then(|l| l.trim().strip_prefix("version = \""))
            .and_then(|l| l.strip_suffix('"'))
            .unwrap_or_else(|| panic!("version line after {needle}"))
            .to_string()
    };
    assert!(
        version_of("sqlite-wasm-rs").starts_with("0.6."),
        "sqlite-wasm-rs 0.6.x bundles SQLite3MC 2.5.1 and honours SQLITE_WASM_RS_SOURCE_DIR"
    );
    // Step 113: v0.2.0 heals a database in place after an outage; Step 119:
    // v0.3.0 reports a takeover as an event; Step 120: v0.4.0 has the slot;
    // Step 123: v0.5.0 resends a commit the server failed internally.
    assert!(
        version_of("sqlite-remote-vfs").starts_with("0.5."),
        "sqlite-remote-vfs 0.5.x"
    );
}

/// The manifest directory of `package` from `source` as the workspace
/// resolves it (`cargo metadata` on the committed lockfile, offline).
fn resolved_manifest_dir(package: &str, source: &str) -> PathBuf {
    let output = std::process::Command::new(env!("CARGO"))
        .args(["metadata", "--format-version", "1", "--locked", "--offline"])
        .current_dir(workspace_path("."))
        .output()
        .expect("run cargo metadata");
    assert!(
        output.status.success(),
        "cargo metadata: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("cargo metadata is JSON");
    let manifest = metadata["packages"]
        .as_array()
        .expect("packages")
        .iter()
        .find(|p| {
            p["name"] == package && p["source"].as_str().is_some_and(|s| s.starts_with(source))
        })
        .unwrap_or_else(|| panic!("{package} from {source} is not in the resolved graph"))
        ["manifest_path"]
        .as_str()
        .expect("manifest_path")
        .to_owned();
    PathBuf::from(manifest)
        .parent()
        .expect("manifest dir")
        .to_path_buf()
}

/// The fork's wasm arm (ADR-0042): rusqlite 0.37 reaches `sqlite-wasm-rs`
/// through the resolved sys crate, the two deprecated stubs resolve, and
/// the one FFI call has a safe wrapper there — never in kupferdraht-core.
#[test]
fn the_forks_sys_crate_carries_the_wasm_arm() {
    let sys_dir = resolved_manifest_dir(
        "libsqlite3-sys",
        "git+https://github.com/SchwarzDigits/rusqlite?tag=v0.37.0-sqlite3mc-2.5.1",
    );
    let read = |rel: &str| {
        let path = sys_dir.join(rel);
        std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read the fork's {}: {e}", path.display()))
    };
    let manifest = read("Cargo.toml");
    assert!(
        manifest.contains(
            "[target.'cfg(all(target_family = \"wasm\", target_os = \"unknown\"))'.dependencies]"
        ) && manifest.contains("sqlite-wasm-rs"),
        "the fork's sys crate must depend on sqlite-wasm-rs on wasm32-unknown-unknown"
    );
    let arm = read("src/wasm.rs");
    for symbol in [
        "pub use sqlite_wasm_rs::*",
        "pub unsafe fn sqlite3_trace",
        "pub unsafe fn sqlite3_profile",
        "pub fn sqlite3mc_wrap_vfs",
    ] {
        assert!(
            arm.contains(symbol),
            "the fork's libsqlite3-sys/src/wasm.rs lacks `{symbol}`"
        );
    }
    let core_src = workspace_path("kupferdraht-core/src");
    let mut stack = vec![core_src];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("read kupferdraht-core/src") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let src = std::fs::read_to_string(&path).expect("read source");
                assert!(
                    !src.contains("sqlite3mc_vfs_create"),
                    "{}: the raw FFI call belongs in the fork's sys crate, not in kupferdraht-core",
                    path.display()
                );
            }
        }
    }
}

/// The IndexedDB record store and its tests are gone (ADR-0022 deletes them
/// with the per-record helpers and the MLS snapshot).
#[test]
fn indexeddb_record_store_left_the_tree() {
    for gone in [
        "kupferdraht-core/src/storage/indexeddb.rs",
        "kupferdraht-core/tests/indexeddb_global_store.rs",
        "kupferdraht-core/tests/indexeddb_mls.rs",
    ] {
        assert!(
            !workspace_path(gone).exists(),
            "{gone} must be deleted with the IndexedDB backend"
        );
    }
    assert!(
        workspace_path("kupferdraht-core/tests/indexeddb_keystore.rs").exists(),
        "the IndexedDB + WebCrypto keystore stays (ADR-0022 decision 6)"
    );
}

/// W2 (decided 2026-09-24): CI compiles the wasm target and runs the browser
/// pins in headless Firefox.
#[test]
fn ci_builds_and_tests_the_wasm_target() {
    let ci = workspace_file(".github/workflows/ci.yml");
    assert!(
        ci.contains("--target wasm32-unknown-unknown"),
        "ci.yml lacks a wasm32-unknown-unknown compile check"
    );
    assert!(
        ci.contains("scripts/wasm-test.sh") && ci.contains("wasm-bindgen-cli"),
        "ci.yml lacks the headless-Firefox browser test job (scripts/wasm-test.sh)"
    );
    let script = workspace_file("scripts/wasm-test.sh");
    // Step 111a: the storage pins run over sqlite-remote-vfs now.
    for test in [
        "step_111a_local_vfs_storage",
        "step_91_main_thread_refused",
        "indexeddb_keystore",
    ] {
        assert!(
            script.contains(test),
            "scripts/wasm-test.sh does not run {test}"
        );
    }
}

// ── the cross-target fixture ────────────────────────────────────────────────

fn fixture_session() -> SessionRecord {
    SessionRecord {
        user_id: FIXTURE_ACCOUNT.into(),
        device_id: "dev-native".into(),
        access_token: MARKER_TOKEN.into(),
        refresh_token: "native-refresh-cookie".into(),
        cookie_label: Some("kupferdraht-step91".into()),
        api_version: 14,
        access_token_issued_at_ms: 1_700_000_000_000,
    }
}

fn fixture_event(message_id: &str, body: &str, expires_at_ms: Option<i64>) -> TimelineEventRecord {
    TimelineEventRecord {
        message_id: message_id.into(),
        room_id: "room-step91@example.test".into(),
        sender_user_id: "bob@example.test".into(),
        sender_device_id: "dev-bob".into(),
        kind: "text".into(),
        body: body.into(),
        sent_at_ms: 1_700_000_000_000,
        delivery_status: "acknowledged".into(),
        reply_to_id: None,
        edited_at_ms: None,
        redacted_at_ms: None,
        expires_at_ms,
        ephemeral_ttl_ms: None,
    }
}

/// One-time fixture generation (`cargo test -p kupferdraht-core --test
/// step_91_wasm_build_glue generate_step_91_fixture -- --ignored`): the
/// production store writes a session and two timeline rows — one plain,
/// one ephemeral whose timer elapsed long ago (the browser's Step-89 sweep
/// retires it) — then switches to a rollback journal and closes cleanly.
/// Regenerating changes the bytes (random per-page nonces); commit once.
#[tokio::test]
#[ignore = "one-time fixture generation; the committed bytes are the pin"]
async fn generate_step_91_fixture() {
    let path = fixture_path();
    std::fs::create_dir_all(path.parent().expect("fixture dir")).expect("mkdir fixtures");
    let _ = std::fs::remove_file(&path);
    let store = SqliteStore::open(&path, &fixture_key()).expect("open fresh fixture");
    store
        .save_session(&fixture_session())
        .await
        .expect("save session");
    store
        .insert_timeline_event(&fixture_event("m-plain", "plain row", None))
        .await
        .expect("insert plain row");
    store
        .insert_timeline_event(&fixture_event(
            "m-ephemeral",
            "vanishes in the browser",
            Some(1_700_000_060_000),
        ))
        .await
        .expect("insert ephemeral row");
    let mode: String = store
        .lock()
        .expect("lock")
        .query_row("PRAGMA journal_mode = DELETE", [], |r| r.get(0))
        .expect("journal_mode");
    assert_eq!(mode, "delete", "sahpool has no shared-memory WAL");
    drop(store);
    for sidecar in ["-wal", "-shm"] {
        let leftover = PathBuf::from(format!("{}{sidecar}", path.display()));
        assert!(
            !leftover.exists(),
            "clean close must leave no {sidecar} sidecar"
        );
    }
}

/// The committed fixture: readable magic, rollback-journal header bytes,
/// the chacha20 reserve, ciphertext payload — and the native store reads
/// it back under the fixture key.
#[tokio::test]
async fn native_fixture_is_committed_and_readable() {
    let path = fixture_path();
    assert!(
        path.exists(),
        "fixture {} missing — run `cargo test -p kupferdraht-core --test step_91_wasm_build_glue \
         generate_step_91_fixture -- --ignored` once",
        path.display()
    );
    let bytes = std::fs::read(&path).expect("read fixture");
    assert_eq!(&bytes[..16], b"SQLite format 3\0", "plaintext magic");
    assert_eq!(
        (bytes[18], bytes[19]),
        (1, 1),
        "rollback journal (no WAL on sahpool)"
    );
    assert_eq!(bytes[20], 32, "chacha20 reserved bytes per page");
    for needle in [MARKER_TOKEN.as_bytes(), b"CREATE TABLE", b"vanishes"] {
        assert!(
            !contains(&bytes, needle),
            "plaintext {:?} at rest",
            String::from_utf8_lossy(needle)
        );
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let copy = dir.path().join("native-chacha20.db");
    std::fs::copy(&path, &copy).expect("copy fixture");
    let store = SqliteStore::open(&copy, &fixture_key()).expect("open the fixture copy");
    let session = store
        .load_session(FIXTURE_ACCOUNT)
        .await
        .expect("load session")
        .expect("fixture session present");
    assert_eq!(session.access_token, MARKER_TOKEN);
}
