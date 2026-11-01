// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 78 — pins for the native sign-out and account purge: the public
//! surface (`Kupferdraht::logout`, `Kupferdraht::forget_account`,
//! `PurgeOptions` at the crate root, their FFI mirrors) and the doc homes
//! the step updated. The behaviour itself is tested in the crate
//! (`manager::forget_tests`, `manager::purge_tests`).

use std::path::Path;

fn workspace_file(rel: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// Compile-time pins: the manager surface and its FFI mirror, by name.
#[test]
fn the_manager_surface_and_its_ffi_mirror_exist() {
    let plain = kupferdraht::PurgeOptions::default();
    assert!(
        !plain.purge_device && !plain.purge_local,
        "the default is the plain logout"
    );
    let _logout = kupferdraht::Kupferdraht::logout;
    let _forget = kupferdraht::Kupferdraht::forget_account;
    let _ffi_logout = kupferdraht::ffi::Kupferdraht::logout;
    let _ffi_forget = kupferdraht::ffi::Kupferdraht::forget_account;
}

#[test]
fn the_library_guide_documents_signing_out_and_forgetting() {
    let library = workspace_file("docs/library.md");
    for needle in [
        "### `Kupferdraht::logout(",
        "### `Kupferdraht::forget_account(",
        "### `PurgeOptions {",
        "Signing out and forgetting an account",
    ] {
        assert!(library.contains(needle), "docs/library.md: {needle}");
    }
}

#[test]
fn the_cli_guide_documents_the_purge_flags_and_account_forget() {
    let cli = workspace_file("docs/cli.md");
    for needle in [
        "### `account-forget`",
        "`--purge-local`",
        "The CLI itself deletes no file and no keystore entry.",
    ] {
        assert!(cli.contains(needle), "docs/cli.md: {needle}");
    }
}

#[test]
fn the_other_doc_homes_carry_the_step() {
    let bindings = workspace_file("docs/bindings.md");
    assert!(
        bindings.contains("`forget_account(account_id)`"),
        "docs/bindings.md: the FFI list"
    );
    assert!(
        bindings.contains("Since Step 78 the account is signed out as well"),
        "docs/bindings.md: the browser's logout note"
    );
    let mapping = workspace_file("docs/api-mapping-matrix-rust-sdk.md");
    assert!(
        mapping.contains("`Kupferdraht::forget_account(account_id: &str)"),
        "api-mapping: the forget_account row"
    );
    let architecture = workspace_file("docs/ARCHITECTURE.md");
    assert!(
        architecture.contains("`Kupferdraht::forget_account`"),
        "ARCHITECTURE.md: the purge paragraph"
    );
    let changelog = workspace_file("CHANGELOG.md");
    assert!(
        changelog.contains("Native sign-out and account purge"),
        "CHANGELOG.md: the Unreleased entry"
    );
}
