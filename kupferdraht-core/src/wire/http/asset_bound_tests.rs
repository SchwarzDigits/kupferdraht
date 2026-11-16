// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 160 — the asset download bounded by the sender's size.
//!
//! Natively the stream to the cache stops past `max_bytes` and leaves no
//! file behind. The browser's in-memory download, compiled here for tests,
//! stops the same way and refuses a bound over its ceiling before any
//! request leaves.

use httpmock::Method::GET;
use httpmock::{Mock, MockServer};
use tempfile::TempDir;

use super::HttpClient;
use super::assets::{AssetDownload, MAX_BUFFERED_ASSET_BYTES};
use crate::error::KupferdrahtError;

/// The bound of these tests, in bytes.
const BOUND: u64 = 96;

fn http(base_url: &str) -> HttpClient {
    HttpClient::new(base_url, true).expect("HttpClient::new (permit_http=true)")
}

fn bounded(max_bytes: u64) -> AssetDownload<'static> {
    AssetDownload {
        asset_domain: "wire.com",
        asset_id: "key-1",
        asset_token: Some("private-tok"),
        max_bytes,
    }
}

/// The asset answered with `len` bytes.
fn serve(server: &MockServer, len: u64) -> Mock<'_> {
    let body = vec![0xc3; usize::try_from(len).expect("a test length")];
    server.mock(|when, then| {
        when.method(GET).path("/v14/assets/wire.com/key-1");
        then.status(200)
            .header("content-type", "application/octet-stream")
            .body(body);
    })
}

#[tokio::test]
async fn a_download_of_the_bound_is_written() {
    let server = MockServer::start();
    serve(&server, BOUND);
    let dir = TempDir::new().expect("tempdir");
    let sink = dir.path().join("asset.enc");

    let written = http(&server.base_url())
        .download_asset(14, "tok", bounded(BOUND), &sink)
        .await
        .expect("a download of the bound");

    assert_eq!(written, BOUND);
    assert_eq!(
        std::fs::read(&sink).expect("the cached blob").len() as u64,
        BOUND
    );
}

#[tokio::test]
async fn a_download_past_the_bound_is_refused_and_leaves_no_file() {
    let server = MockServer::start();
    serve(&server, BOUND + 1);
    let dir = TempDir::new().expect("tempdir");
    let sink = dir.path().join("asset.enc");

    let err = http(&server.base_url())
        .download_asset(14, "tok", bounded(BOUND), &sink)
        .await
        .expect_err("one byte past the bound");

    assert!(
        matches!(err, KupferdrahtError::IntegrityCheck(_)),
        "more than the sender declared: {err:?}"
    );
    assert!(!sink.exists(), "no partial blob stays in the cache");
}

#[tokio::test]
async fn a_buffered_download_of_the_bound_is_read() {
    let server = MockServer::start();
    serve(&server, BOUND);

    let blob = http(&server.base_url())
        .download_asset_bytes(14, "tok", bounded(BOUND))
        .await
        .expect("a buffered download of the bound");

    assert_eq!(blob.len() as u64, BOUND);
}

#[tokio::test]
async fn a_buffered_download_past_the_bound_is_refused() {
    let server = MockServer::start();
    serve(&server, BOUND + 1);

    let err = http(&server.base_url())
        .download_asset_bytes(14, "tok", bounded(BOUND))
        .await
        .map(|blob| blob.len())
        .expect_err("one byte past the bound");

    assert!(
        matches!(err, KupferdrahtError::IntegrityCheck(_)),
        "more than the sender declared: {err:?}"
    );
}

#[tokio::test]
async fn a_buffered_download_over_the_ceiling_is_refused_before_any_request() {
    let server = MockServer::start();
    let asset = serve(&server, 16);

    let err = http(&server.base_url())
        .download_asset_bytes(14, "tok", bounded(MAX_BUFFERED_ASSET_BYTES + 1))
        .await
        .map(|blob| blob.len())
        .expect_err("a declared size over the ceiling");

    assert!(
        matches!(err, KupferdrahtError::InvalidInput(_)),
        "too large to hold in memory: {err:?}"
    );
    asset.assert_calls(0);
}

#[tokio::test]
async fn a_buffered_download_at_the_ceiling_is_requested() {
    let server = MockServer::start();
    let asset = serve(&server, 16);

    let blob = http(&server.base_url())
        .download_asset_bytes(14, "tok", bounded(MAX_BUFFERED_ASSET_BYTES))
        .await
        .expect("a declared size at the ceiling");

    assert_eq!(blob.len(), 16);
    asset.assert_calls(1);
}
