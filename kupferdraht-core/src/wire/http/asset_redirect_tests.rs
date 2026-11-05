// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 141 — `download_asset` follows the backend's one redirect to the
//! asset store (a regression of Step 132, M2). TDD red.
//!
//! The API mock answers `302` (or `303` / `307`) with `Location` on a
//! second mock server — the pre-signed URL of cargohold's asset store. The
//! hop is a plain GET: no bearer token, no `Asset-Token`. Exactly one hop;
//! a permanent redirect and every other 3xx stay rejections. The scheme
//! check (`http` only with `permit_http`) is a unit test of
//! `redirect_target`: these mock servers are plain `http`, so the client
//! here needs `permit_http` anyway.

use std::time::{Duration, Instant};

use httpmock::Method::GET;
use httpmock::{Mock, MockServer};
use tempfile::TempDir;

use super::{HttpClient, HttpTimeouts};
use crate::error::KupferdrahtError;

const ASSET_PATH: &str = "/v14/assets/wire.com/key-1";
const STORE_PATH: &str = "/wire-assets/v3/otr/key-1";
const PAYLOAD: &[u8] = b"ciphertext-from-the-asset-store";

fn http(base_url: &str) -> HttpClient {
    HttpClient::new(base_url, true).expect("HttpClient::new (permit_http=true)")
}

/// The pre-signed URL on `store`: the signature rides in the query.
fn signed_url(store: &MockServer) -> String {
    store.url(format!("{STORE_PATH}?X-Amz-Signature=sig141"))
}

/// The API host redirects the authenticated download to `location`.
fn mount_redirect<'a>(api: &'a MockServer, status: u16, location: &str) -> Mock<'a> {
    api.mock(|when, then| {
        when.method(GET)
            .path(ASSET_PATH)
            .header("authorization", "Bearer tok")
            .header("asset-token", "private-tok");
        then.status(status).header("location", location);
    })
}

/// The asset store serves the bytes — to a request without credentials.
fn mount_store(store: &MockServer) -> Mock<'_> {
    store.mock(|when, then| {
        when.method(GET)
            .path(STORE_PATH)
            .query_param("X-Amz-Signature", "sig141")
            .is_true(|r| {
                let h = r.headers();
                !h.contains_key("authorization")
                    && !h.contains_key("asset-token")
                    && !h.contains_key("cookie")
            });
        then.status(200)
            .header("content-type", "application/octet-stream")
            .body(PAYLOAD);
    })
}

/// Any request at all reaching `server`.
fn mount_any(server: &MockServer) -> Mock<'_> {
    server.mock(|when, then| {
        when.any_request();
        then.status(200).body("must not be fetched");
    })
}

async fn download(client: &HttpClient, sink: &std::path::Path) -> Result<u64, KupferdrahtError> {
    client
        .download_asset(14, "tok", "wire.com", "key-1", Some("private-tok"), sink)
        .await
}

#[tokio::test]
async fn a_302_303_or_307_is_followed_without_credentials() {
    for status in [302u16, 303, 307] {
        let api = MockServer::start();
        let store = MockServer::start();
        let redirect = mount_redirect(&api, status, &signed_url(&store));
        let bytes = mount_store(&store);
        let dir = TempDir::new().expect("tempdir");
        let sink = dir.path().join("downloaded.enc");

        let n = download(&http(&api.base_url()), &sink)
            .await
            .unwrap_or_else(|e| panic!("{status}: the redirect must be followed, got {e:?}"));

        assert_eq!(n, PAYLOAD.len() as u64, "{status}");
        assert_eq!(
            std::fs::read(&sink).expect("read sink"),
            PAYLOAD,
            "{status}"
        );
        redirect.assert_calls(1);
        bytes.assert_calls(1);
    }
}

#[tokio::test]
async fn a_permanent_redirect_is_not_followed() {
    for status in [301u16, 308] {
        let api = MockServer::start();
        let store = MockServer::start();
        mount_redirect(&api, status, &signed_url(&store));
        let target = mount_any(&store);
        let dir = TempDir::new().expect("tempdir");

        let err = download(&http(&api.base_url()), &dir.path().join("x.enc"))
            .await
            .expect_err("a permanent redirect is a rejection");

        assert!(
            matches!(err, KupferdrahtError::BackendRejected(_)),
            "{status}: {err:?}"
        );
        target.assert_calls(0);
    }
}

#[tokio::test]
async fn the_hop_target_redirecting_again_is_refused() {
    let api = MockServer::start();
    let store = MockServer::start();
    let third = MockServer::start();
    mount_redirect(&api, 302, &signed_url(&store));
    let second_hop = store.mock(|when, then| {
        when.method(GET).path(STORE_PATH);
        then.status(302).header("location", third.url("/elsewhere"));
    });
    let beyond = mount_any(&third);
    let dir = TempDir::new().expect("tempdir");

    let err = download(&http(&api.base_url()), &dir.path().join("x.enc"))
        .await
        .expect_err("exactly one hop");

    assert!(
        matches!(err, KupferdrahtError::BackendRejected(_)),
        "{err:?}"
    );
    second_hop.assert_calls(1);
    beyond.assert_calls(0);
}

#[tokio::test]
async fn a_redirect_without_a_usable_location_is_refused_before_any_hop() {
    let store = MockServer::start();
    let userinfo = store
        .url(STORE_PATH)
        .replacen("http://", "http://user:pw@", 1);
    for location in [
        None,
        Some("/relative/key-1"),
        Some("not a url"),
        Some(userinfo.as_str()),
    ] {
        let api = MockServer::start();
        let redirect = api.mock(|when, then| {
            when.method(GET).path(ASSET_PATH);
            match location {
                Some(location) => then.status(302).header("location", location),
                None => then.status(302),
            };
        });
        let elsewhere_on_api = api.mock(|when, then| {
            when.method(GET).path_excludes(ASSET_PATH);
            then.status(200).body("must not be fetched");
        });
        let mut target = mount_any(&store);
        let dir = TempDir::new().expect("tempdir");

        let err = download(&http(&api.base_url()), &dir.path().join("x.enc"))
            .await
            .expect_err("no usable location");

        assert!(
            matches!(err, KupferdrahtError::BackendRejected(_)),
            "{location:?}: {err:?}"
        );
        redirect.assert_calls(1);
        elsewhere_on_api.assert_calls(0);
        target.assert_calls(0);
        target.delete();
    }
}

/// A refusal at the asset store names its host and status — the query is
/// a credential for its lifetime, and S3 echoes the signature in its
/// error body (`SignatureDoesNotMatch`).
#[tokio::test]
async fn a_refusal_of_the_hop_names_the_host_never_the_signature() {
    let api = MockServer::start();
    let store = MockServer::start();
    mount_redirect(&api, 302, &signed_url(&store));
    let refusal = store.mock(|when, then| {
        when.method(GET).path(STORE_PATH);
        then.status(403).body(
            "<Error><Code>SignatureDoesNotMatch</Code>\
             <SignatureProvided>sig141</SignatureProvided></Error>",
        );
    });
    let dir = TempDir::new().expect("tempdir");

    let err = download(&http(&api.base_url()), &dir.path().join("x.enc"))
        .await
        .expect_err("a refused hop fails the download");

    refusal.assert_calls(1);
    let KupferdrahtError::BackendRejected(message) = err else {
        panic!("a refused hop is BackendRejected, got {err:?}");
    };
    assert!(message.contains("403"), "the status: {message}");
    assert!(message.contains(&store.host()), "the host: {message}");
    assert!(
        !message.contains("sig141"),
        "the signature must not reach an error: {message}"
    );
}

/// The transfer idle timeout (Step 98) covers the hop: a store that
/// accepts and never answers fails the download.
#[tokio::test]
async fn a_silent_asset_store_fails_after_the_idle_timeout() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let silent = format!(
        "http://{}{STORE_PATH}",
        listener.local_addr().expect("addr")
    );
    let hold = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept");
        tokio::time::sleep(Duration::from_secs(10)).await;
        drop(stream);
    });
    let api = MockServer::start();
    mount_redirect(&api, 302, &silent);
    let client = HttpClient::with_timeouts(
        &api.base_url(),
        true,
        HttpTimeouts {
            connect: Duration::from_secs(2),
            request: Duration::from_secs(2),
            transfer_idle: Duration::from_millis(300),
        },
    )
    .expect("client");
    let dir = TempDir::new().expect("tempdir");
    let started = Instant::now();

    let err = download(&client, &dir.path().join("x.enc"))
        .await
        .expect_err("a silent store fails the download");

    assert!(
        matches!(&err, KupferdrahtError::Network(m) if m.contains("stalled")),
        "the idle timeout on the hop, got {err:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "bounded by the idle timeout, took {:?}",
        started.elapsed()
    );
    hold.abort();
}
