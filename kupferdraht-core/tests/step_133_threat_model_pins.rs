// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 133 — THREAT_MODEL 0.7: every claim matches the code (TDD red).
//!
//! Documentation pins. The production-readiness review of 2026-09-25
//! measured six claims of `docs/THREAT_MODEL.md` (and the two sentences
//! README and CLAUDE.md repeat) against the code; phase 1 chose to narrow
//! each of them. These pins hold the narrowed wording: no
//! compiler-enforced zeroization, the auth modes the code lacks only as a
//! marked design sketch, no reproducible builds in the trust model, no
//! local forward secrecy before Step 134, the server's device injection
//! as an assumption, no pointer at the dissolved open.md §71, and a 0.7
//! row in the Versions table.

use std::path::Path;

/// Read a file at the workspace root (one level above the
/// `kupferdraht-core` manifest).
fn workspace_file(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

const THREAT_MODEL: &str = "docs/THREAT_MODEL.md";

/// The documents that state the model's claims to a reader: the model,
/// the README's capability list, the charter, the library guide.
const CLAIM_HOMES: [&str; 4] = ["README.md", "CLAUDE.md", THREAT_MODEL, "docs/library.md"];

/// The text of a `## <heading>` section, up to the next `## ` heading.
fn section<'a>(doc: &'a str, heading: &str) -> &'a str {
    let start = doc
        .find(&format!("\n## {heading}"))
        .unwrap_or_else(|| panic!("section '{heading}' in THREAT_MODEL"));
    let body = &doc[start + 1..];
    let end = body[3..].find("\n## ").map_or(body.len(), |i| i + 3);
    &body[..end]
}

#[test]
fn no_document_claims_compiler_enforced_zeroization() {
    for home in CLAIM_HOMES {
        let doc = workspace_file(home);
        for needle in [
            "compiler-enforced",
            "compiler-checked",
            "Zeroization is thus a compiler guarantee",
        ] {
            assert!(
                !doc.contains(needle),
                "{home} still claims '{needle}': key material and credentials in their \
                 holders are zeroized on drop; tokens, the login body and message \
                 plaintext are not (Step 133)"
            );
        }
    }
}

#[test]
fn threat_model_keeps_the_absent_auth_modes_only_as_a_marked_sketch() {
    let doc = workspace_file(THREAT_MODEL);
    for needle in [
        "Three modes in the API",
        "distinguished at the type level",
        "as interface stubs",
    ] {
        assert!(
            !doc.contains(needle),
            "THREAT_MODEL still presents auth modes the code lacks as types: '{needle}'"
        );
    }
    let auth = section(&doc, "Auth mode");
    assert!(
        auth.to_lowercase().contains("design sketch"),
        "the auth-mode section must mark PasswordProtected / PasswordOnly as a design sketch:\n{auth}"
    );
}

#[test]
fn threat_model_does_not_promise_reproducible_builds() {
    let doc = workspace_file(THREAT_MODEL);
    assert!(
        !doc.contains("code review, reproducible builds and open-source inspection"),
        "the trust model names reproducible builds no build provides"
    );
}

#[test]
fn threat_model_g4_does_not_claim_local_forward_secrecy_before_step_134() {
    let doc = workspace_file(THREAT_MODEL);
    assert!(
        !doc.contains("Forward secrecy applies in the protocol *and* in local storage"),
        "G4 promises local forward secrecy that waits for key rotation and secure_delete (Step 134)"
    );
}

#[test]
fn threat_model_states_device_injection_as_an_assumption() {
    let doc = workspace_file(THREAT_MODEL);
    let assumptions = section(&doc, "Assumptions");
    assert!(
        assumptions.contains("**A7"),
        "the Assumptions section needs A7, the server injects no device:\n{assumptions}"
    );
    let trust = section(&doc, "Trust Model");
    assert!(
        trust.contains("A7"),
        "the Trust Model's content-trust sentence must rest on A7:\n{trust}"
    );
}

#[test]
fn threat_model_has_a_0_7_versions_row() {
    let doc = workspace_file(THREAT_MODEL);
    let versions = section(&doc, "Versions");
    assert!(
        versions.lines().any(|l| l.starts_with("| 0.7 ")),
        "the Versions table needs a 0.7 row for Step 133:\n{versions}"
    );
}

#[test]
fn claim_homes_point_at_no_dissolved_open_md_section() {
    for home in ["README.md", "CLAUDE.md", THREAT_MODEL] {
        let doc = workspace_file(home);
        assert!(
            !doc.contains("§71"),
            "{home} points at open.md §71, which dissolved on 2026-11-01 (the Linux keystore limit is NG-24)"
        );
    }
}
