// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 148 — the lockfile floor of inout's 0.2 line.
//!
//! GHSA-2943-gp7x-qg39 (in-place buffers of
//! `InOutBufReserved::split_reserved` and `into_padded_blocks` were
//! undefined behaviour through the safe API) affects inout 0.2.0–0.2.5
//! and is in neither RustSec nor GitHub's reviewed database, so neither
//! `cargo deny` nor Dependabot reports it. inout is transitive: its floor
//! lives in `Cargo.lock`. inout 0.1, locked beside it, is not affected.

use std::path::Path;

/// The affected release line and its first fixed release.
const LINE: (u32, u32) = (0, 2);
const FLOOR: (u32, u32, u32) = (0, 2, 6);

/// Read a file at the workspace root (one level above the
/// `kupferdraht-core` manifest).
fn workspace_file(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// The locked versions of `package` (exact-name match).
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

/// The locked inout versions on the affected release line.
fn locked_on_line() -> Vec<(u32, u32, u32)> {
    locked_versions(&workspace_file("Cargo.lock"), "inout")
        .iter()
        .map(|v| semver_triple(v))
        .filter(|(major, minor, _)| (*major, *minor) == LINE)
        .collect()
}

#[test]
fn inout_0_2_is_still_locked() {
    assert!(
        !locked_on_line().is_empty(),
        "no inout 0.2 in Cargo.lock any more: drop this floor"
    );
}

#[test]
fn every_locked_inout_0_2_is_at_or_above_the_fix() {
    let below: Vec<_> = locked_on_line()
        .into_iter()
        .filter(|v| *v < FLOOR)
        .collect();
    assert!(
        below.is_empty(),
        "inout below {FLOOR:?} (GHSA-2943-gp7x-qg39): {below:?}"
    );
}
