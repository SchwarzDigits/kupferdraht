// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 128 — `crypto-bigint` 0.5 leaves the dependency graph
//! (RUSTSEC-2026-0387; TDD red).
//!
//! Lockfile and `deny.toml` pins only: the P-256 stack moves to the
//! RustCrypto 0.14 line — `openmls_rust_crypto` and
//! `openmls_basic_credential` from the organisation's openmls fork ahead
//! of an openmls release (ADR-0042's mechanism), `p256` 0.14 and an OPRF
//! client on that line in Kupferdraht — and the advisory's `deny.toml`
//! ignore goes with it. The behaviour pins that must stay green already
//! exist: the RFC 9497 vectors of the PIN service (`recovery/oprf.rs`),
//! the receipt known-answer tests (`recovery/receipt.rs`) and the MLS
//! flows against a real openmls second device (`common::mls::PeerDevice`).

use std::path::Path;

/// Read a file at the workspace root (one level above the
/// `kupferdraht-core` manifest).
fn workspace_file(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// Every resolved version of `package` in `Cargo.lock` (exact-name
/// match — `p256` must not match `p256-oprf`). Duplicates matter: a
/// partial bump leaves the advisory's version beside the fixed one.
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

/// The first `[[package]]` block of `package` in `Cargo.lock`, for
/// source assertions.
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

/// `major.minor.patch` of a release version, for ordering.
fn semver_triple(version: &str) -> (u32, u32, u32) {
    let mut parts = version.split('.').map(|p| {
        p.parse::<u32>()
            .unwrap_or_else(|e| panic!("unexpected version shape {version}: {e}"))
    });
    (
        parts.next().expect("major"),
        parts.next().expect("minor"),
        parts.next().expect("patch"),
    )
}

#[test]
fn lockfile_crypto_bigint_carries_the_advisory_fix() {
    let lock = workspace_file("Cargo.lock");
    let versions = lock_versions_of(&lock, "crypto-bigint");
    assert!(!versions.is_empty(), "crypto-bigint in Cargo.lock");
    for version in &versions {
        assert!(
            semver_triple(version) >= (0, 7, 6),
            "every crypto-bigint must be >= 0.7.6 (RUSTSEC-2026-0387), got {versions:?}"
        );
    }
}

#[test]
fn lockfile_p256_is_on_the_0_14_line() {
    let lock = workspace_file("Cargo.lock");
    let versions = lock_versions_of(&lock, "p256");
    assert_eq!(
        versions.len(),
        1,
        "one p256 in the graph — a second one is the 0.13 line coming back: {versions:?}"
    );
    assert!(
        versions[0].starts_with("0.14."),
        "p256 must be on the 0.14 line (crypto-bigint 0.7), got {}",
        versions[0]
    );
}

#[test]
fn lockfile_hpke_rs_is_on_the_0_8_line() {
    let lock = workspace_file("Cargo.lock");
    for package in ["hpke-rs", "hpke-rs-crypto", "hpke-rs-rust-crypto"] {
        let versions = lock_versions_of(&lock, package);
        assert!(!versions.is_empty(), "{package} in Cargo.lock");
        for version in &versions {
            assert!(
                version.starts_with("0.8."),
                "{package} must be on the 0.8 line (the p256 0.14 HPKE), got {versions:?}"
            );
        }
    }
}

#[test]
fn lockfile_openmls_crypto_crates_come_from_the_fork() {
    let lock = workspace_file("Cargo.lock");
    for package in ["openmls_rust_crypto", "openmls_basic_credential"] {
        let versions = lock_versions_of(&lock, package);
        assert_eq!(
            versions.len(),
            1,
            "one {package} in the graph: {versions:?}"
        );
        let block = lock_block(&lock, package);
        assert!(
            block.contains("source = \"git+https://github.com/SchwarzDigits/openmls?tag="),
            "{package} must resolve to the SchwarzDigits openmls fork by tag \
             (the p256 0.14 line ahead of an openmls release), got:\n{block}"
        );
    }
}

#[test]
fn lockfile_holds_no_voprf_on_the_0_13_line() {
    let lock = workspace_file("Cargo.lock");
    let versions = lock_versions_of(&lock, "voprf");
    assert!(
        versions.is_empty(),
        "voprf (p256 0.13) must be gone from the graph, got {versions:?}"
    );
}

#[test]
fn deny_toml_carries_no_0387_ignore() {
    let deny = workspace_file("deny.toml");
    assert!(
        !deny.contains("RUSTSEC-2026-0387"),
        "deny.toml must not ignore RUSTSEC-2026-0387 once crypto-bigint 0.5 is gone"
    );
}
