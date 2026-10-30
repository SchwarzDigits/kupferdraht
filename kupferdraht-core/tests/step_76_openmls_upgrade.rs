// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 76 — openmls 0.8.1 → 0.9.0 upgrade (TDD red).
//!
//! Lockfile pins: the upgraded tree must carry the advisory fixes the
//! four `deny.toml` ignores were waiting for (open.md §62 — trigger
//! live since openmls 0.9.0, 2026-08-25), and `deny.toml` must no
//! longer carry those ignores. The storage-format preservation pins
//! (an mls.db written by the 0.8.1 stack stays readable) live as unit
//! tests in `encryption/provider.rs` — they need crate-internal
//! access to the keyed-connection path.

use std::path::Path;

/// Read a file at the workspace root (one level above the
/// `kupferdraht-core` manifest).
fn workspace_file(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// Every resolved version of `package` in `Cargo.lock` (exact-name
/// match — `openmls` must not match `openmls_traits`). Duplicates
/// matter: crates without a `links` attribute (e.g. `tls_codec`) can
/// silently resolve to TWO majors after a partial bump.
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

/// Single resolved version of `package`, `None` when absent.
/// Panics on duplicates — callers pinning one version must not
/// silently pick the first of two.
fn lock_version_of(lock: &str, package: &str) -> Option<String> {
    let mut versions = lock_versions_of(lock, package);
    assert!(
        versions.len() <= 1,
        "{package} resolves to multiple versions: {versions:?}"
    );
    versions.pop()
}

#[test]
fn lockfile_openmls_is_0_9() {
    let lock = workspace_file("Cargo.lock");
    let version = lock_version_of(&lock, "openmls").expect("openmls in Cargo.lock");
    assert!(
        version.starts_with("0.9."),
        "openmls must be on the 0.9 line (the deny.toml removal \
         trigger, open.md §62), got {version}"
    );
}

#[test]
fn lockfile_libcrux_sha3_carries_the_advisory_fixes() {
    let lock = workspace_file("Cargo.lock");
    let version = lock_version_of(&lock, "libcrux-sha3").expect("libcrux-sha3 in Cargo.lock");
    let patch: u32 = version
        .strip_prefix("0.0.")
        .and_then(|p| p.parse().ok())
        .unwrap_or_else(|| panic!("unexpected libcrux-sha3 version shape: {version}"));
    assert!(
        patch >= 10,
        "libcrux-sha3 must be >= 0.0.10 (fixes RUSTSEC-2026-0207/0208), got {version}"
    );
}

#[test]
fn lockfile_vulnerable_transitives_are_gone_or_fixed() {
    let lock = workspace_file("Cargo.lock");
    // The wasm timer chain leaves the graph entirely (openmls 0.9
    // uses web-time — RUSTSEC-2024-0384).
    for gone in ["instant", "fluvio-wasm-timer"] {
        assert!(
            lock_version_of(&lock, gone).is_none(),
            "{gone} must leave the dependency graph with the openmls 0.9 bump"
        );
    }
    // libcrux-secrets stays in the tree (transitive under
    // libcrux-sha3 0.0.10) but must be on the fixed line —
    // RUSTSEC-2026-0212 is patched in >= 0.0.6.
    if let Some(version) = lock_version_of(&lock, "libcrux-secrets") {
        let patch: u32 = version
            .strip_prefix("0.0.")
            .and_then(|p| p.parse().ok())
            .unwrap_or_else(|| panic!("unexpected libcrux-secrets version shape: {version}"));
        assert!(
            patch >= 6,
            "libcrux-secrets must be >= 0.0.6 (fixes RUSTSEC-2026-0212), got {version}"
        );
    }
}

/// The format-trap companions must move WITH openmls (plan scope
/// 1 + 3): a partial bump can leave 0.2-format storage, a
/// duplicated `tls_codec`, or the old `rusqlite` behind while
/// `lockfile_openmls_is_0_9` already shows green.
#[test]
fn lockfile_companion_crates_moved_together() {
    let lock = workspace_file("Cargo.lock");
    let expect_single = |package: &str, line: &str| {
        let versions = lock_versions_of(&lock, package);
        assert_eq!(
            versions.len(),
            1,
            "{package} must resolve to exactly one version, got {versions:?}"
        );
        assert!(
            versions[0].starts_with(line),
            "{package} must be on the {line} line, got {}",
            versions[0]
        );
    };
    expect_single("openmls_sqlite_storage", "0.3.");
    expect_single("tls_codec", "0.5.");
    expect_single("rusqlite", "0.37.");
}

/// The 0.8.1 storage-format declaration stays off. Source-verified
/// during the step (observations §73): for Kupferdraht's self-describing
/// `JsonCodec` both feature states read and write identical bytes, and
/// ADR-0041 wiped every installation, so no 0.8.1-format `mls.db`
/// exists that the feature would keep readable. The fixtures that once
/// pinned the round-trip were retired with it. This test pins that the
/// feature does not come back without a decision.
#[test]
fn manifest_leaves_the_081_storage_format_feature_off() {
    let manifest = workspace_file("kupferdraht-core/Cargo.toml");
    let enabled = manifest
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .any(|line| line.contains("0-8-1-storage-format"));
    assert!(
        !enabled,
        "kupferdraht-core/Cargo.toml enables openmls's `0-8-1-storage-format` \
         feature again — no installation holds 0.8.1-format state (ADR-0041); \
         re-enabling it needs a decision and a fixture"
    );
}

#[test]
fn deny_toml_advisory_ignores_are_removed() {
    let deny = workspace_file("deny.toml");
    for id in [
        "RUSTSEC-2026-0207",
        "RUSTSEC-2026-0208",
        "RUSTSEC-2026-0212",
        "RUSTSEC-2024-0384",
    ] {
        // Match the ignore-entry syntax, not the bare id — a later
        // comment mentioning the id historically must not re-redden
        // this test.
        let ignore_entry = format!("id = \"{id}\"");
        assert!(
            !deny.contains(&ignore_entry),
            "deny.toml must drop the {id} ignore once the openmls 0.9 \
             bump removes the vulnerable chain (open.md §62 trigger)"
        );
    }
}
