// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! C1 (#9): `ClientBuilder::resume` requires an explicit account.
//!
//! The old filesystem dir-scan (`discover_resume_account_id`) that
//! auto-picked the single persisted account is gone. The device
//! `GlobalStore` registry (`Kupferdraht::active_account` / `Kupferdraht::resume`)
//! is the one source of "which account to resume". A raw
//! `ClientBuilder::resume()` without `.account(id)` is now refused
//! instead of guessing from disk — even when exactly one account is
//! persisted, so the refusal is about the missing selection, not an
//! empty home.

use std::sync::Arc;

use httpmock::MockServer;
use kupferdraht::keystore::{InMemoryKeyStore, SecureKeyStore};
use kupferdraht::{ClientBuilder, Kupferdraht, KupferdrahtError};
use tempfile::TempDir;

mod common;

#[tokio::test]
async fn resume_without_account_is_refused_even_with_one_persisted_account() {
    let home = TempDir::new().expect("tempdir");
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    let keystore: Arc<dyn SecureKeyStore> = Arc::new(InMemoryKeyStore::new());

    // Seed exactly one persisted account the way the CLI does: through
    // `Kupferdraht::login`, which persists the session + registers the account
    // and stores the per-account key in the keystore.
    let app = Kupferdraht::open(home.path().to_path_buf(), keystore.clone())
        .expect("open kupferdraht")
        .permit_http();
    app.login(
        common::server_config(&server.base_url()),
        "alice@example.test".into(),
        "pw".into(),
        None,
        None,
    )
    .await
    .expect("login")
    .register_device()
    .await
    .expect("register device");

    // A raw resume WITHOUT `.account(id)` against the same home + keystore:
    // the SDK no longer scans the `accounts/` directory to guess the
    // single account — it refuses and points the caller at explicit
    // selection (`Kupferdraht::resume` / `.account(id)`).
    let err = ClientBuilder::new()
        .permit_http()
        .base_dir(home.path().to_path_buf())
        .keystore(keystore)
        .resume()
        .await
        .expect_err("no-arg resume must be refused, not dir-scanned");

    assert!(
        matches!(&err, KupferdrahtError::InvalidInput(msg) if msg.contains("account")),
        "expected an InvalidInput pointing at the missing account, got {err:?}"
    );
}
