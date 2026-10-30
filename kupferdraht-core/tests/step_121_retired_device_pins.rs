// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 121 — native pins for browser-only wiring (ADR-0040). TDD red.
//!
//! The forgetting itself is unit-tested in the crate
//! (`manager::seed_login_tests`), natively and for real. What cannot be
//! executed natively is the *wiring*: `DeviceRetired` is raised by the
//! browser's storage layer, so these hold the two call sites that must react
//! to it. Both exist because a replaced browser that resets itself on one
//! way in and not on the other is the bug this step fixes.

use std::path::Path;

fn source(rel: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// The registration path forgets the keys when the slot refuses the open.
///
/// It asserts the **match**, not the name: a first version of this test
/// looked for `forget_retired_device` and passed against a `todo!()` stub,
/// because a definition and a doc comment were enough to satisfy it. A guard
/// that a stub satisfies guards nothing.
#[test]
fn a_registration_that_meets_a_retired_device_forgets_its_keys() {
    let text = source("kupferdraht-core/src/manager/pending_login/first_setup.rs");
    assert!(
        text.contains("Err(KupferdrahtError::DeviceRetired(_))"),
        "the registration path does not match on DeviceRetired, so nothing reacts to it"
    );
    assert!(
        text.contains("self.forget_retired_device()"),
        "the registration path meets DeviceRetired without forgetting the account's keys, \
         so every further login meets it again and nothing can clear the browser"
    );
}

/// And the resume path keeps doing it, so the two ways in stay symmetric.
#[test]
fn a_resume_that_meets_a_retired_device_still_forgets_its_keys() {
    let text = source("kupferdraht-core/src/manager/remote_resume.rs");
    assert!(
        text.contains("Err(KupferdrahtError::DeviceRetired(_))") && text.contains("forget_account"),
        "the resume path stopped forgetting a retired device's keys"
    );
}
