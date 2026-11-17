// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 134 — what Kupferdraht creates on disk is its owner's only: a fresh
//! home, its accounts and their databases are created 0700 / 0600; a file
//! that already exists keeps its mode. `secure_delete` is pinned per open
//! path in the unit tests; the docs say what is and is not provided.

use std::path::Path;
use std::sync::Arc;

use kupferdraht::keystore::{InMemoryKeyStore, SecureKeyStore};
use kupferdraht::storage::{open_default_global_store, open_default_store};

const ACCOUNT: &str = "alice@example.test";

#[cfg(unix)]
fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::metadata(path)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
        .permissions()
        .mode()
        & 0o777
}

fn keystore() -> Arc<dyn SecureKeyStore> {
    Arc::new(InMemoryKeyStore::new())
}

#[cfg(unix)]
#[test]
fn a_fresh_home_its_account_and_their_databases_are_private() {
    let dir = tempfile::tempdir().expect("tempdir");
    let base = dir.path().join("home");
    let keystore = keystore();
    let _global = open_default_global_store(&base, &keystore).expect("global store");
    let _store = open_default_store(&base, &keystore, ACCOUNT).expect("account store");
    let account = base.join("accounts").join(ACCOUNT);
    for d in [&base, &base.join("accounts"), &account] {
        assert_eq!(mode(d), 0o700, "{} is private", d.display());
    }
    for f in [base.join("global.db"), account.join("kupferdraht.db")] {
        assert_eq!(mode(&f), 0o600, "{} is private", f.display());
    }
    let wal = account.join("kupferdraht.db-wal");
    if wal.exists() {
        assert_eq!(
            mode(&wal),
            0o600,
            "SQLite gives the WAL the database's mode"
        );
    }
}

#[cfg(unix)]
#[test]
fn an_existing_database_keeps_its_mode() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = tempfile::tempdir().expect("tempdir");
    let base = dir.path().join("home");
    let keystore = keystore();
    drop(open_default_global_store(&base, &keystore).expect("first open"));
    let global = base.join("global.db");
    std::fs::set_permissions(&global, std::fs::Permissions::from_mode(0o644)).expect("chmod");
    drop(open_default_global_store(&base, &keystore).expect("second open"));
    assert_eq!(mode(&global), 0o644, "no chmod sweep over an existing file");
}

#[test]
fn the_docs_say_what_is_provided() {
    let read = |rel: &str| {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(rel);
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
    };
    let threat_model = read("docs/THREAT_MODEL.md");
    assert!(
        threat_model.contains("secure_delete"),
        "G4 names secure_delete"
    );
    assert!(
        threat_model.contains("Step 161"),
        "G4 names the parked key rotation"
    );
    assert!(read("docs/ARCHITECTURE.md").contains("secure_delete"));
    assert!(read("CHANGELOG.md").contains("`secure_delete`"));
}
