// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Every source file Kupferdraht authors opens with two SPDX lines
//! (CLAUDE.md "License Headers"). This test walks the tracked files of
//! the workspace and names every file that lacks them, so a new file
//! cannot land without its notice.
//!
//! Not tagged by design: `vendor/` (third-party), the Gradle wrappers
//! (Apache-2.0, Gradle's), the SHA-pinned SQL migrations
//! (`migration_immutability.rs`), the Protobuf schema (it carries Wire's
//! notice and Kupferdraht's own), docs and configuration files.

#![cfg(not(target_arch = "wasm32"))]

use std::path::{Path, PathBuf};
use std::process::Command;

const COPYRIGHT: &str = "SPDX-FileCopyrightText: 2026 Schwarz Digits KG";
const LICENSE: &str = "SPDX-License-Identifier: GPL-3.0-only";

const TAGGED_EXTENSIONS: &[&str] = &[
    "rs", "kt", "kts", "swift", "js", "ts", "html", "sh", "yml", "yaml",
];
const TAGGED_FILES: &[&str] = &["scripts/smoke.env.example"];
const EXCLUDED_PREFIXES: &[&str] = &[
    "vendor/",
    "docs/",
    "kupferdraht-core/src/storage/migrations/",
];
const EXCLUDED_SUFFIXES: &[&str] = &["/gradlew", "/gradlew.bat"];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("kupferdraht-core sits in the workspace root")
        .to_path_buf()
}

fn tracked_files(root: &Path) -> Vec<String> {
    let out = Command::new("git")
        .arg("ls-files")
        .current_dir(root)
        .output()
        .expect("git ls-files runs");
    assert!(out.status.success(), "git ls-files failed");
    String::from_utf8(out.stdout)
        .expect("utf-8 paths")
        .lines()
        .map(str::to_owned)
        .collect()
}

fn is_tagged(rel: &str) -> bool {
    if EXCLUDED_PREFIXES.iter().any(|p| rel.starts_with(p))
        || EXCLUDED_SUFFIXES.iter().any(|s| rel.ends_with(s))
    {
        return false;
    }
    if TAGGED_FILES.contains(&rel) {
        return true;
    }
    let name = rel.rsplit('/').next().unwrap_or(rel);
    name.rsplit_once('.')
        .is_some_and(|(_, ext)| TAGGED_EXTENSIONS.contains(&ext))
}

/// The lines a file type needs first: a shebang, Swift's tools-version
/// pragma, or an HTML doctype. The notice follows them.
fn notice_window(text: &str) -> String {
    let mut lines = text.lines();
    let mut window = Vec::new();
    if let Some(first) = lines.next() {
        let keep_first = first.starts_with("#!/")
            || first.starts_with("// swift-tools-version")
            || first.to_ascii_lowercase().starts_with("<!doctype");
        if !keep_first {
            window.push(first);
        }
    }
    window.extend(lines.take(3));
    window.join("\n")
}

#[test]
fn every_authored_source_file_carries_the_spdx_notice() {
    let root = workspace_root();
    let mut missing = Vec::new();
    let mut checked = 0usize;
    for rel in tracked_files(&root).into_iter().filter(|r| is_tagged(r)) {
        let text = std::fs::read_to_string(root.join(&rel)).expect("read tracked file");
        checked += 1;
        let window = notice_window(&text);
        if !(window.contains(COPYRIGHT) && window.contains(LICENSE)) {
            missing.push(rel);
        }
    }
    assert!(
        checked > 300,
        "the walk found only {checked} files — rules broken?"
    );
    assert!(
        missing.is_empty(),
        "{} file(s) without the SPDX notice (CLAUDE.md \"License Headers\"):\n  {}",
        missing.len(),
        missing.join("\n  ")
    );
}

#[test]
fn the_exclusions_are_the_documented_ones() {
    assert!(!is_tagged("vendor/sqlite3mc/sqlite3mc_amalgamation.c"));
    assert!(!is_tagged("bindings/kotlin/gradlew"));
    assert!(!is_tagged(
        "kupferdraht-core/src/storage/migrations/V1__init.sql"
    ));
    assert!(!is_tagged("kupferdraht-core/proto/generic_message.proto"));
    assert!(!is_tagged("docs/library.md"));
    assert!(!is_tagged("Cargo.toml"));
    assert!(is_tagged("kupferdraht-core/src/lib.rs"));
    assert!(is_tagged("bindings/swift/Package.swift"));
    assert!(is_tagged("scripts/smoke.env.example"));
    assert!(is_tagged(".github/workflows/ci.yml"));
}
