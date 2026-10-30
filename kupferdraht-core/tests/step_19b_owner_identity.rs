// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 19b — the owner identity is unified to the qualified user id.
//!
//! Proves the bare/qualified seam is closed on the owner side: after login,
//! `Client::user_id()` is the qualified `<localpart>@<domain>`, and the
//! persisted owner rows (`session` / `self_user`) store that same qualified
//! id — not the bare uuid plus a separate (nullable) domain column.
//!
//! Uses `Kupferdraht::login` (not the lower-level `ClientBuilder::login`) because
//! the device manager is what persists the session / self_user rows.

use std::sync::Arc;

use httpmock::MockServer;
use kupferdraht::Kupferdraht;
use kupferdraht::keystore::{InMemoryKeyStore, SecureKeyStore};
use tempfile::TempDir;

mod common;

const QUALIFIED: &str = "user-uuid@example.test";

#[tokio::test]
async fn owner_rows_store_the_qualified_user_id() {
    let home = TempDir::new().expect("tempdir");
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    let keystore: Arc<dyn SecureKeyStore> = Arc::new(InMemoryKeyStore::new());

    let app = Kupferdraht::open(home.path().to_path_buf(), keystore)
        .expect("open kupferdraht")
        .permit_http();
    let client = app
        .login(
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

    // In memory: the client's identity is the qualified id, with the
    // localpart / domain reachable as structural slices.
    let user_id = client.user_id().expect("logged-in client has a user id");
    assert_eq!(user_id.as_str(), QUALIFIED);
    assert_eq!(user_id.localpart(), "user-uuid");
    assert_eq!(user_id.domain(), "example.test");

    // At rest: the owner rows are keyed by the qualified id — there is no
    // bare uuid + separate domain column any more.
    let store = client.store().expect("login attached a store");
    let session = store
        .load_session(QUALIFIED)
        .await
        .expect("load_session")
        .expect("session row present under the qualified id");
    assert_eq!(session.user_id, QUALIFIED);

    let self_user = store
        .load_self_user(QUALIFIED)
        .await
        .expect("load_self_user")
        .expect("self_user row present under the qualified id");
    assert_eq!(self_user.id, QUALIFIED);
}
