// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 81 — httpmock 0.7 → 0.8 (TDD red).
//!
//! The Step-76 lockfile-pin pattern: the upgraded dev-dep tree must
//! shed the discontinued `async-std` chain (RUSTSEC-2025-0052,
//! suppressed since 2026-05-22 — open.md §48 → Step 81), and
//! `deny.toml` must drop the ignore. Everything else about this step
//! is preservation: the full suite against the migrated mocks IS the
//! test.

use std::path::Path;

fn workspace_file(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// Every resolved version of `package` in `Cargo.lock` (exact-name
/// match).
fn lock_versions_of(lock: &str, package: &str) -> Vec<String> {
    let needle = format!("name = \"{package}\"");
    let mut versions = Vec::new();
    let mut lines = lock.lines();
    while let Some(line) = lines.next() {
        if line.trim() == needle
            && let Some(version_line) = lines.next()
            && let Some(v) = version_line
                .trim()
                .strip_prefix("version = \"")
                .and_then(|rest| rest.strip_suffix('"'))
        {
            versions.push(v.to_string());
        }
    }
    versions
}

#[test]
fn lockfile_httpmock_is_0_8() {
    let lock = workspace_file("Cargo.lock");
    let versions = lock_versions_of(&lock, "httpmock");
    assert_eq!(versions.len(), 1, "one httpmock, got {versions:?}");
    assert!(
        versions[0].starts_with("0.8."),
        "httpmock must be on the 0.8 line (drops async-std), got {}",
        versions[0]
    );
}

#[test]
fn lockfile_async_std_is_gone() {
    let lock = workspace_file("Cargo.lock");
    assert!(
        lock_versions_of(&lock, "async-std").is_empty(),
        "async-std must leave the dependency graph with httpmock 0.8 \
         (RUSTSEC-2025-0052, open.md §48)"
    );
    // Corrected expectation (the §5.22 class, verified in the
    // lockfile): httpmock 0.8 still uses async-object-pool — as 0.2,
    // the line that no longer sits on async-std.
    let pool = lock_versions_of(&lock, "async-object-pool");
    assert!(
        pool.iter().all(|v| !v.starts_with("0.1.")),
        "async-object-pool must be off the 0.1 (async-std) line, got {pool:?}"
    );
}

#[test]
fn deny_toml_async_std_ignore_is_removed() {
    let deny = workspace_file("deny.toml");
    assert!(
        !deny.contains("id = \"RUSTSEC-2025-0052\""),
        "deny.toml must drop the RUSTSEC-2025-0052 ignore once \
         async-std leaves the tree"
    );
}
