// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `Client::logout(purge_device = true)` — the trailing
//! `/access/logout` is best-effort: deleting the device invalidates
//! the session server-side, so Wire rejects the still-held token
//! ("Invalid zauth token") and the logout call 403s through no fault
//! of the caller. Found live 2026-07-19 during the Step 60
//! smoke-artifact cleanup: the propagated error also skipped the
//! CLI's local-state wipe.

mod common;

use httpmock::Method::{DELETE, POST};
use httpmock::MockServer;

#[tokio::test]
async fn purge_logout_tolerates_the_session_dying_with_the_device() {
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    let delete = server.mock(|when, then| {
        when.method(DELETE).path("/v14/clients/new-client-id");
        then.status(200);
    });
    let logout = server.mock(|when, then| {
        when.method(POST).path("/v14/access/logout");
        then.status(403)
            .header("content-type", "application/json")
            .body(r#"{"code":403,"label":"invalid-credentials","message":"Invalid zauth token"}"#);
    });
    let (client, _store) = common::run_login(&server).await;
    client
        .logout(true, Some("pw"))
        .await
        .expect("purge logout succeeds although the session died with the device");
    delete.assert();
    logout.assert();
}

#[tokio::test]
async fn plain_logout_still_surfaces_a_dead_cookie() {
    // purge_device = false: nothing on our side invalidated the
    // session, so a 403 from /access/logout stays a caller-visible
    // error — the best-effort downgrade is purge-only.
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    server.mock(|when, then| {
        when.method(POST).path("/v14/access/logout");
        then.status(403)
            .header("content-type", "application/json")
            .body(r#"{"code":403,"label":"invalid-credentials","message":"Invalid zauth token"}"#);
    });
    let (client, _store) = common::run_login(&server).await;
    client
        .logout(false, None)
        .await
        .expect_err("plain logout must surface the 403");
}

#[tokio::test]
async fn purge_logout_propagates_a_rejected_device_delete() {
    // Wrong password → DELETE /clients 403. That error is real
    // (device still registered) and must propagate; /access/logout
    // must not even be attempted.
    let server = MockServer::start();
    common::mount_full_login_flow(&server);
    server.mock(|when, then| {
        when.method(DELETE).path("/v14/clients/new-client-id");
        then.status(403)
            .header("content-type", "application/json")
            .body(r#"{"code":403,"label":"invalid-credentials","message":"wrong password"}"#);
    });
    let logout = server.mock(|when, then| {
        when.method(POST).path("/v14/access/logout");
        then.status(200);
    });
    let (client, _store) = common::run_login(&server).await;
    client
        .logout(true, Some("wrong"))
        .await
        .expect_err("a rejected device delete must propagate");
    logout.assert_calls(0);
}
