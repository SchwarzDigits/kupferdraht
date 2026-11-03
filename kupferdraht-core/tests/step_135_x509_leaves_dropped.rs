// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 135 — X.509 leaves: drop instead of envelope attribution (TDD red).
//!
//! Documentation and source pins. The behaviour itself is pinned beside
//! `attribute_sender` in `sync/sender.rs` (a non-Basic leaf yields `None`
//! and one `warn!`). What these pins hold: no living claim home still
//! states the envelope exception ADR-0027 decision 2 made — the ADR's
//! Decision and Consequences, its rule file, THREAT_MODEL G1, the library
//! guide, the MLS deviations, the architecture overview, ADR-0028's
//! consequence that inherited it — and no source comment or constant
//! keeps the envelope as an attribution source.

use std::path::{Path, PathBuf};

/// Read a file at the workspace root (one level above the
/// `kupferdraht-core` manifest).
fn workspace_file(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

const ADR_0027: &str = "docs/adrs/0027-attribute-inbound-messages-to-mls-leaf-credential.md";
const ADR_0028: &str = "docs/adrs/0028-authorize-edits-and-deletes-by-original-sender.md";
const RULE_0027: &str = "rules/0027-attribute-inbound-messages-to-mls-leaf-credential.md";
const THREAT_MODEL: &str = "docs/THREAT_MODEL.md";

/// Every way the living documents phrased the exception.
const EXCEPTION_PHRASES: [&str; 6] = [
    "keeps the envelope attribution",
    "keep the envelope attribution",
    "still attributed by the envelope",
    "still attributed through the envelope",
    "stay attributed by the unauthenticated envelope",
    "attributed by the envelope (x.509 leaves",
];

/// The text of a `## <heading>` section, up to the next `## ` heading.
fn section<'a>(doc: &'a str, heading: &str) -> &'a str {
    let start = doc
        .find(&format!("\n## {heading}"))
        .unwrap_or_else(|| panic!("section '{heading}'"));
    let body = &doc[start + 1..];
    let end = body[3..].find("\n## ").map_or(body.len(), |i| i + 3);
    &body[..end]
}

fn assert_no_exception_phrase(home: &str, text: &str) {
    let lower = text.to_lowercase();
    for phrase in EXCEPTION_PHRASES {
        assert!(
            !lower.contains(phrase),
            "{home} still says '{phrase}': a message signed by a non-Basic leaf is \
             dropped since Step 135, never attributed through the envelope"
        );
    }
}

#[test]
fn adr_0027_decision_two_drops_non_basic_senders() {
    let adr = workspace_file(ADR_0027);
    let decision = section(&adr, "Decision");
    assert_no_exception_phrase("ADR-0027 Decision", decision);
    assert!(
        decision.to_lowercase().contains("dropped"),
        "ADR-0027's Decision must say that a non-Basic sender's message is dropped"
    );
    assert_no_exception_phrase("ADR-0027 Consequences", section(&adr, "Consequences"));
    assert!(
        adr.contains("Step 135"),
        "ADR-0027's revision history must record the Step 135 amendment"
    );
}

#[test]
fn rule_0027_drops_non_basic_senders() {
    let rule = workspace_file(RULE_0027);
    assert_no_exception_phrase("rules/0027", &rule);
    let x509_line = rule
        .lines()
        .find(|l| l.contains("X.509") && !l.contains("SAN"))
        .expect("rules/0027 has a line about the X.509 sender");
    assert!(
        x509_line.to_lowercase().contains("drop"),
        "the X.509 rule must say drop: {x509_line}"
    );
}

#[test]
fn threat_model_g1_has_no_exception_and_a_new_version_row() {
    let doc = workspace_file(THREAT_MODEL);
    let g1 = doc
        .lines()
        .find(|l| l.starts_with("**G1"))
        .expect("THREAT_MODEL has the G1 paragraph");
    assert_no_exception_phrase("THREAT_MODEL G1", g1);
    assert!(
        !g1.contains("Exception until"),
        "G1 must no longer carry the X.509 exception sentence"
    );
    assert!(
        doc.lines()
            .any(|l| l.starts_with("| 0.8") && l.contains("Step 135")),
        "THREAT_MODEL's Versions table needs a 0.8 row naming Step 135"
    );
    // Later revisions move the header on (0.9 with Step 139): at least 0.8.
    let version: f32 = doc
        .lines()
        .find_map(|l| l.strip_prefix("**Version:** "))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|v| v.parse().ok())
        .expect("THREAT_MODEL's header carries a version");
    assert!(
        version >= 0.8,
        "THREAT_MODEL's header must be at version 0.8 or later, is {version}"
    );
}

#[test]
fn the_other_claim_homes_state_the_drop() {
    for home in [
        "docs/library.md",
        "docs/wire/mls-deviations.md",
        "docs/ARCHITECTURE.md",
    ] {
        assert_no_exception_phrase(home, &workspace_file(home));
    }
    let adr28 = workspace_file(ADR_0028);
    assert_no_exception_phrase("ADR-0028 Consequences", section(&adr28, "Consequences"));
    assert!(
        adr28.contains("Step 135"),
        "ADR-0028's revision history must record that its inherited weakness is gone"
    );
}

/// Every `.rs` file under `dir`, recursively.
fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in
        std::fs::read_dir(dir).unwrap_or_else(|e| panic!("read_dir {}: {e}", dir.display()))
    {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn no_source_keeps_the_envelope_as_an_attribution_source() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&src, &mut files);
    assert!(files.len() > 100, "the walk found the sources");
    for path in files {
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let home = path
            .strip_prefix(&src)
            .expect("under src")
            .display()
            .to_string();
        assert_no_exception_phrase(&home, &text);
        assert!(
            !text.contains("unknown@unknown") && !text.contains("UNKNOWN_SENDER"),
            "{home}: the envelope stand-in sender is gone with the envelope path (Step 135)"
        );
    }
}
