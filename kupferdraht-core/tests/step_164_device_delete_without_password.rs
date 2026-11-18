// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 164 — an SSO user deletes a device without a password.
//!
//! Wire re-authenticates a device delete by the account's password, and an
//! account without one (an SSO user's) by nothing (`kalium-readings.md` #31):
//! the backend decides. `Client::delete_device` and
//! `Client::logout(purge_device)` take the password as an option; `None` —
//! and an empty one — sends none, and a backend that wants one answers
//! `missing-auth`. The FFI, the browser binding and the tabs' typings follow.

mod common;

use std::path::Path;

use httpmock::Method::{DELETE, POST};
use httpmock::{Mock, MockServer};
use kupferdraht::KupferdrahtError;

/// Another device of the account; the fixture's own is `new-client-id`.
const OTHER: &str = "abcd1234";
const OWN: &str = "new-client-id";

/// `DELETE /v14/clients/{id}` whose body carries `password` — or, for
/// `None`, no password field at all.
fn mount_delete<'a>(server: &'a MockServer, id: &str, password: Option<&'static str>) -> Mock<'a> {
    server.mock(|when, then| {
        when.method(DELETE)
            .path(format!("/v14/clients/{id}"))
            .is_true(move |req| {
                let Ok(body) = serde_json::from_slice::<serde_json::Value>(req.body_ref()) else {
                    return false;
                };
                body.get("password").and_then(serde_json::Value::as_str) == password
            });
        then.status(200);
    })
}

#[tokio::test]
async fn a_device_is_deleted_without_a_password() {
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    let delete = mount_delete(&server, OTHER, None);
    let (client, _store) = common::run_login(&server).await;

    client
        .delete_device(OTHER, None)
        .await
        .expect("an account without a password deletes without one");

    delete.assert_calls(1);
}

/// What an app or the CLI hands over for "no password" — an empty answer —
/// sends none, rather than an empty password the backend would refuse.
#[tokio::test]
async fn an_empty_password_is_no_password() {
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    let delete = mount_delete(&server, OTHER, None);
    let (client, _store) = common::run_login(&server).await;

    client
        .delete_device(OTHER, Some(""))
        .await
        .expect("an empty password sends none");

    delete.assert_calls(1);
}

#[tokio::test]
async fn a_password_still_goes_along() {
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    let delete = mount_delete(&server, OTHER, Some("pw"));
    let (client, _store) = common::run_login(&server).await;

    client
        .delete_device(OTHER, Some("pw"))
        .await
        .expect("an account with a password deletes with it");

    delete.assert_calls(1);
}

#[tokio::test]
async fn the_current_device_is_still_refused() {
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    let delete = mount_delete(&server, OWN, None);
    let (client, _store) = common::run_login(&server).await;

    let err = client
        .delete_device(OWN, None)
        .await
        .expect_err("the current device goes through logout");

    assert!(
        matches!(err, KupferdrahtError::InvalidInput(_)),
        "got {err:?}"
    );
    delete.assert_calls(0);
}

/// An account with a password, deleting without it: the backend names what
/// is missing, and the caller sees its label.
#[tokio::test]
async fn a_missing_password_is_named_by_the_backend() {
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    server.mock(|when, then| {
        when.method(DELETE).path(format!("/v14/clients/{OTHER}"));
        then.status(403)
            .header("content-type", "application/json")
            .body(r#"{"code":403,"label":"missing-auth","message":"Re-authentication via password required"}"#);
    });
    let (client, _store) = common::run_login(&server).await;

    let err = client
        .delete_device(OTHER, None)
        .await
        .expect_err("the backend wants the password");

    assert!(
        matches!(&err, KupferdrahtError::BackendRejected(msg) if msg.contains("missing-auth")),
        "got {err:?}"
    );
}

#[tokio::test]
async fn a_purge_logout_deletes_the_device_without_a_password() {
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    let delete = mount_delete(&server, OWN, None);
    server.mock(|when, then| {
        when.method(POST).path("/v14/access/logout");
        then.status(200);
    });
    let (client, _store) = common::run_login(&server).await;

    client
        .logout(true, None)
        .await
        .expect("a purge logout without a password");

    delete.assert_calls(1);
}

// ---- the bindings ----

fn workspace_file(rel: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

#[test]
fn the_ffi_takes_the_password_as_an_option() {
    let admin = workspace_file("kupferdraht-core/src/ffi/admin.rs");
    let at = admin
        .find("pub async fn delete_device(")
        .expect("the FFI's delete_device");
    let signature = &admin[at..at + 200];
    assert!(
        signature.contains("password: Option<String>"),
        "FFI delete_device: {signature}"
    );
}

#[test]
fn the_browser_takes_the_password_as_an_option() {
    let manager = workspace_file("kupferdraht-core/src/wasm/manager.rs");
    // `PendingLogin.deleteDevice` takes no password; `Client.deleteDevice` an optional one.
    let signatures: Vec<&str> = manager
        .match_indices("pub async fn delete_device(")
        .map(|(at, _)| &manager[at..at + 160])
        .collect();
    assert!(
        signatures
            .iter()
            .any(|signature| signature.contains("password: Option<String>")),
        "the browser's Client.deleteDevice takes an optional password: {signatures:?}"
    );
    let tabs = workspace_file("bindings/web/kupferdraht-tabs/index.d.ts");
    assert!(
        tabs.contains("deleteDevice(deviceId: string, password?: string | null): Promise<void>;"),
        "the tabs' typings declare the optional password"
    );
}
