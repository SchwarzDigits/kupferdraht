// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 156: own certificate authorities. The TLS paths are tested where
//! they are built (`wire/http/tls_tests.rs`); this file pins the builders,
//! the FFI and the CLI surface, and the docs.

use std::sync::Arc;

use kupferdraht::keystore::{InMemoryKeyStore, SecureKeyStore};
use kupferdraht::{Kupferdraht, KupferdrahtError};

fn workspace_file(path: &str) -> String {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace");
    std::fs::read_to_string(root.join(path)).expect(path)
}

fn ca_pem() -> String {
    let key = rcgen::KeyPair::generate().expect("key");
    let mut params = rcgen::CertificateParams::new(Vec::<String>::new()).expect("params");
    params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    params.self_signed(&key).expect("ca").pem()
}

fn manager(pem: &str) -> Result<Kupferdraht, KupferdrahtError> {
    let dir = tempfile::tempdir().expect("tempdir");
    let keystore: Arc<dyn SecureKeyStore> = Arc::new(InMemoryKeyStore::new());
    Kupferdraht::builder()
        .base_dir(dir.keep())
        .keystore(keystore)
        .add_root_certificates(pem)
        .build()
}

#[test]
fn the_manager_takes_an_authority() {
    manager(&ca_pem()).expect("a manager trusting an own authority");
}

#[test]
fn the_manager_refuses_pem_without_an_authority() {
    let result = manager("not a certificate");

    assert!(
        matches!(result, Err(KupferdrahtError::InvalidInput(_))),
        "{:?}",
        result.err()
    );
}

#[test]
fn every_surface_has_the_call() {
    assert!(
        workspace_file("kupferdraht-core/src/ffi/manager.rs")
            .contains("pub fn open_with_root_certificates")
    );
    let cli = workspace_file("kupferdraht-cli/src/main.rs");
    assert!(cli.contains("ca_file"), "login --ca-file");
}

#[test]
fn the_docs_say_how() {
    assert!(workspace_file("docs/library.md").contains("add_root_certificates"));
    assert!(workspace_file("docs/bindings.md").contains("openWithRootCertificates"));
    assert!(workspace_file("docs/cli.md").contains("--ca-file"));
    assert!(workspace_file("docs/THREAT_MODEL.md").contains("Step 156"));
    assert!(workspace_file("CHANGELOG.md").contains("`add_root_certificates`"));
    assert!(
        workspace_file("CLAUDE.md").contains("Step 156"),
        "the rustls rule names the TLS configuration Step 156 allows"
    );
    let adrs =
        std::fs::read_dir(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../docs/adrs"))
            .expect("adrs")
            .filter_map(Result::ok)
            .find(|e| {
                std::fs::read_to_string(e.path()).is_ok_and(|t| t.contains("add_root_certificates"))
            })
            .expect("an ADR decides the trust anchors");
    let rule = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../rules")
        .join(adrs.file_name());
    assert!(rule.exists(), "its rules/ file, {}", rule.display());
}
