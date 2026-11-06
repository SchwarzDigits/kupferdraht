// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 142 — openmls 0.9.2: the security floor and one fork tag.
//!
//! openmls 0.9.1 and 0.9.2 fix GHSA-gc79-23g3-8g52 (unbounded recursion
//! while decoding nested extensions can abort the process),
//! GHSA-94v4-rg25-x625 (removed leaves reappear after an external commit
//! regrows the tree) and GHSA-p276-g3f3-wwgh (rejected private messages
//! advance the sender ratchet). The RustSec database carries none of
//! them, so `cargo deny` cannot hold the floor: these pins do — in the
//! lockfile, and in the manifest a library consumer resolves against.
//! The three crates from the SchwarzDigits openmls fork come from one
//! tag (ADR-0042); which tag is `step_90_sqlite3mc_engine`'s pin.

use std::path::Path;

/// The first openmls release with all three fixes.
const OPENMLS_FLOOR: (u32, u32, u32) = (0, 9, 2);

/// The crates Kupferdraht takes from the SchwarzDigits openmls fork.
const FORK_CRATES: [&str; 3] = [
    "openmls_sqlite_storage",
    "openmls_rust_crypto",
    "openmls_basic_credential",
];

/// Read a file at the workspace root (one level above the
/// `kupferdraht-core` manifest).
fn workspace_file(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// The `[[package]]` blocks of `package` in `Cargo.lock` (exact-name
/// match — `openmls` must not match `openmls_traits`).
fn lock_blocks(lock: &str, package: &str) -> Vec<String> {
    let needle = format!("name = \"{package}\"");
    let mut blocks = Vec::new();
    let mut lines = lock.lines();
    while let Some(line) = lines.next() {
        if line.trim() == needle {
            let block: Vec<&str> = lines
                .by_ref()
                .take_while(|l| !l.trim().is_empty())
                .collect();
            blocks.push(block.join("\n"));
        }
    }
    blocks
}

/// The value of `key = "…"` in a lockfile block.
fn block_field<'a>(block: &'a str, key: &str) -> Option<&'a str> {
    block.lines().find_map(|line| {
        line.trim()
            .strip_prefix(key)?
            .strip_prefix(" = \"")?
            .strip_suffix('"')
    })
}

/// `major.minor.patch` of a version or a plain requirement (`0.9.2`,
/// `^0.9.2`, `=0.9.2`); a missing part counts as 0.
fn semver_triple(version: &str) -> (u32, u32, u32) {
    let bare = version.trim_start_matches(['^', '=', '~']);
    let mut parts = bare.split('.').map(|p| {
        p.parse::<u32>()
            .unwrap_or_else(|e| panic!("unexpected version shape {version}: {e}"))
    });
    (
        parts.next().expect("major"),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    )
}

/// The version requirement of every `openmls = …` dependency line of a
/// manifest: `openmls = "0.9.2"` or `openmls = { version = "0.9.2", … }`.
fn openmls_requirements(manifest: &str) -> Vec<String> {
    manifest
        .lines()
        .filter_map(|line| line.trim().strip_prefix("openmls = "))
        .map(|rest| {
            let quoted = rest
                .strip_prefix('"')
                .or_else(|| rest.split_once("version = \"").map(|(_, v)| v))
                .unwrap_or_else(|| panic!("openmls dependency without a version: {rest}"));
            quoted.split('"').next().expect("closing quote").to_string()
        })
        .collect()
}

#[test]
fn lockfile_openmls_carries_the_advisory_fixes() {
    let lock = workspace_file("Cargo.lock");
    let blocks = lock_blocks(&lock, "openmls");
    assert_eq!(blocks.len(), 1, "one openmls in the graph");
    let version = block_field(&blocks[0], "version").expect("openmls version");
    assert!(
        semver_triple(version) >= OPENMLS_FLOOR,
        "openmls must be 0.9.2 or later (GHSA-gc79-23g3-8g52, GHSA-94v4-rg25-x625, \
         GHSA-p276-g3f3-wwgh), got {version}"
    );
}

#[test]
fn manifest_requires_the_fixed_openmls_at_every_site() {
    let manifest = workspace_file("kupferdraht-core/Cargo.toml");
    let requirements = openmls_requirements(&manifest);
    assert_eq!(
        requirements.len(),
        2,
        "openmls at the native and the wasm site, got {requirements:?}"
    );
    for requirement in &requirements {
        assert!(
            semver_triple(requirement) >= OPENMLS_FLOOR,
            "every openmls requirement must exclude the affected releases \
             (at least 0.9.2) — a consumer of the library resolves against it; \
             got {requirements:?}"
        );
    }
}

#[test]
fn the_fork_crates_come_from_one_tag() {
    let lock = workspace_file("Cargo.lock");
    let sources: Vec<String> = FORK_CRATES
        .iter()
        .map(|package| {
            let blocks = lock_blocks(&lock, package);
            assert_eq!(blocks.len(), 1, "one {package} in the graph");
            let source = block_field(&blocks[0], "source")
                .unwrap_or_else(|| panic!("{package} has a source"));
            source
                .split_once('#')
                .map_or(source, |(spec, _commit)| spec)
                .to_string()
        })
        .collect();
    assert!(
        sources[0].starts_with("git+https://github.com/SchwarzDigits/openmls?tag="),
        "the fork crates come from the SchwarzDigits openmls fork by tag, got {sources:?}"
    );
    assert!(
        sources.iter().all(|s| *s == sources[0]),
        "the three fork crates must share one tag, got {sources:?}"
    );
}
