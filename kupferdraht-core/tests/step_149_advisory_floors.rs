// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 149 — lockfile floors for four repository advisories that no
//! scanner carries.
//!
//! GHSA-7fx9-626j-vqph (zerovec-derive's `#[derive(ULE)]` and zerovec's
//! `impl ULE for [T; N]` validate only a buffer's first element),
//! GHSA-jh2m-fhpj-3vr7 (rustix's xattr getters take a zero-size query's
//! count as initialised bytes), GHSA-3jjw-63vg-v3hh (icu_normalizer's
//! `normalize_utf8` can return invalid UTF-8) and GHSA-6pp4-4cxf-xf88
//! (quinn-udp's `recv_single` fallback; locked, never compiled) are
//! repository advisories in neither RustSec nor GitHub's reviewed
//! database, so neither `cargo deny` nor Dependabot reports them. The
//! crates are transitive: their floor lives in `Cargo.lock`, and these
//! pins hold it.

use std::path::Path;

/// Each crate's first fixed release, with the advisory it fixes.
const FLOORS: [(&str, (u32, u32, u32), &str); 5] = [
    ("zerovec", (0, 11, 8), "GHSA-7fx9-626j-vqph"),
    ("zerovec-derive", (0, 11, 5), "GHSA-7fx9-626j-vqph"),
    ("rustix", (1, 1, 6), "GHSA-jh2m-fhpj-3vr7"),
    ("icu_normalizer", (2, 4, 0), "GHSA-3jjw-63vg-v3hh"),
    ("quinn-udp", (0, 5, 16), "GHSA-6pp4-4cxf-xf88"),
];

/// Read a file at the workspace root (one level above the
/// `kupferdraht-core` manifest).
fn workspace_file(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// The locked versions of `package` (exact-name match — `zerovec` must
/// not match `zerovec-derive`).
fn locked_versions(lock: &str, package: &str) -> Vec<String> {
    let needle = format!("name = \"{package}\"");
    let mut versions = Vec::new();
    let mut lines = lock.lines();
    while let Some(line) = lines.next() {
        if line.trim() == needle {
            let version = lines
                .next()
                .and_then(|l| l.trim().strip_prefix("version = \"")?.strip_suffix('"'))
                .unwrap_or_else(|| panic!("{package}: no version line after its name"));
            versions.push(version.to_owned());
        }
    }
    versions
}

/// `major.minor.patch` of a locked version; a pre-release or build
/// suffix is ignored.
fn semver_triple(version: &str) -> (u32, u32, u32) {
    let core = version.split(['-', '+']).next().unwrap_or(version);
    let mut parts = core.split('.').map(|p| {
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
fn every_floor_names_a_locked_crate() {
    let lock = workspace_file("Cargo.lock");
    let missing: Vec<&str> = FLOORS
        .iter()
        .map(|(name, _, _)| *name)
        .filter(|name| locked_versions(&lock, name).is_empty())
        .collect();
    assert!(
        missing.is_empty(),
        "no longer in Cargo.lock, drop their floors: {missing:?}"
    );
}

#[test]
fn every_locked_version_is_at_or_above_its_advisory_floor() {
    let lock = workspace_file("Cargo.lock");
    let below: Vec<String> = FLOORS
        .iter()
        .flat_map(|(name, floor, advisory)| {
            locked_versions(&lock, name)
                .into_iter()
                .filter(|v| semver_triple(v) < *floor)
                .map(move |v| format!("{name} {v} < {floor:?} ({advisory})"))
        })
        .collect();
    assert!(below.is_empty(), "below the advisory floor: {below:#?}");
}
