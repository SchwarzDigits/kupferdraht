// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 136 — clippy holds every function of the library to 60 lines
//! (`too_many_lines`, part of the denied `clippy::pedantic`), so none grows
//! past the ceiling unseen; the 40-line budget below it stays a review
//! matter. The two residuals the step was commissioned with are recorded as
//! non-goals with their reasons.

use std::path::Path;

fn workspace_file(rel: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

#[test]
fn clippy_holds_functions_to_sixty_lines() {
    let config = workspace_file("clippy.toml");
    assert!(
        config
            .lines()
            .any(|l| l.trim() == "too-many-lines-threshold = 60"),
        "clippy.toml sets the ceiling: {config}"
    );
    assert!(
        workspace_file("kupferdraht-core/src/lib.rs").contains("#![deny(clippy::pedantic)]"),
        "too_many_lines is a pedantic lint; the library denies pedantic"
    );
}

#[test]
fn the_rules_say_so() {
    let claude = workspace_file("CLAUDE.md");
    assert!(
        claude.contains("`too-many-lines-threshold = 60`"),
        "CLAUDE.md's Design rules name the enforced ceiling"
    );
    let non_goals = workspace_file("docs/non-goals.md");
    for id in ["NG-32", "NG-33"] {
        assert!(non_goals.contains(id), "docs/non-goals.md records {id}");
    }
}
