// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 159 — answers arrive compressed where the server offers it, and no
//! answer is read past 16 MiB of decoded bytes. TDD red.
//!
//! nginz compresses `application/json` of 1 KiB and more when the request
//! asks for it (observations §103). The bound counts decoded bytes, whatever
//! the encoding: a few KiB of gzip inflate past it.

use std::io::Write as _;

use flate2::Compression;
use flate2::write::GzEncoder;
use httpmock::Method::GET;
use httpmock::{Mock, MockServer};

use super::HttpClient;
use super::notifications::FetchedNotifications;
use crate::error::KupferdrahtError;

/// The bound: 16 MiB of decoded bytes.
const BOUND: usize = 16 * 1024 * 1024;

const ONE_PAGE: &str =
    r#"{"notifications":[{"id":"n-1","payload":[{"type":"x"}]}],"has_more":false}"#;

fn http(base_url: &str) -> HttpClient {
    HttpClient::new(base_url, true).expect("HttpClient::new (permit_http=true)")
}

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(bytes).expect("gzip");
    encoder.finish().expect("gzip")
}

/// `ONE_PAGE` padded with trailing whitespace: valid JSON of exactly `len`
/// bytes.
fn page_of(len: usize) -> Vec<u8> {
    assert!(
        len >= ONE_PAGE.len(),
        "a page is at least its one notification"
    );
    let mut page = ONE_PAGE.as_bytes().to_vec();
    page.resize(len, b' ');
    page
}

fn mount_page(server: &MockServer, body: Vec<u8>, gzipped: bool) -> Mock<'_> {
    server.mock(|when, then| {
        when.method(GET).path("/v14/notifications");
        let then = then.status(200).header("content-type", "application/json");
        let then = if gzipped {
            then.header("content-encoding", "gzip")
        } else {
            then
        };
        then.body(body);
    })
}

async fn fetch(server: &MockServer) -> Result<FetchedNotifications, KupferdrahtError> {
    http(&server.base_url())
        .fetch_notifications(14, "tok", "dev-1", None, 100)
        .await
}

fn notification_count(fetched: FetchedNotifications) -> usize {
    match fetched {
        FetchedNotifications::Page(page) => page.notifications.len(),
        FetchedNotifications::SinceNotFound => panic!("a page, not SinceNotFound"),
    }
}

fn is_over_the_bound(err: &KupferdrahtError) -> bool {
    matches!(err, KupferdrahtError::BackendRejected(m) if m.contains(&format!("over {BOUND} bytes")))
}

#[tokio::test]
async fn a_request_asks_for_gzip() {
    let server = MockServer::start();
    let mock = server.mock(|when, then| {
        when.method(GET).path("/v14/notifications").is_true(|req| {
            req.headers()
                .get("accept-encoding")
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| value.split(',').any(|coding| coding.trim() == "gzip"))
        });
        then.status(200)
            .header("content-type", "application/json")
            .body(ONE_PAGE);
    });
    fetch(&server).await.expect("a request asking for gzip");
    mock.assert_calls(1);
}

#[tokio::test]
async fn a_gzip_page_reads_like_the_plain_one() {
    let server = MockServer::start();
    mount_page(&server, gzip(ONE_PAGE.as_bytes()), true);
    let fetched = fetch(&server).await.expect("a gzip page decodes");
    assert_eq!(notification_count(fetched), 1);
}

#[tokio::test]
async fn a_page_of_exactly_the_bound_is_read() {
    let server = MockServer::start();
    mount_page(&server, page_of(BOUND), false);
    let fetched = fetch(&server).await.expect("a page at the bound");
    assert_eq!(notification_count(fetched), 1);
}

#[tokio::test]
async fn a_page_past_the_bound_is_refused() {
    let server = MockServer::start();
    mount_page(&server, page_of(BOUND + 1), false);
    let err = fetch(&server).await.expect_err("a page past the bound");
    assert!(is_over_the_bound(&err), "{err:?}");
    assert!(
        err.to_string().contains("fetch_notifications"),
        "names the call: {err}"
    );
}

#[tokio::test]
async fn a_gzip_page_inflating_past_the_bound_is_refused() {
    let server = MockServer::start();
    let bomb = gzip(&page_of(BOUND + 1));
    assert!(
        bomb.len() < 64 * 1024,
        "a few KiB on the wire: {}",
        bomb.len()
    );
    mount_page(&server, bomb, true);
    let err = fetch(&server).await.expect_err("decoded past the bound");
    assert!(is_over_the_bound(&err), "{err:?}");
}

#[tokio::test]
async fn a_group_info_past_the_bound_is_refused() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET)
            .path("/v14/conversations/example.test/room-uuid/groupinfo");
        then.status(200)
            .header("content-type", "message/mls")
            .body(vec![0u8; BOUND + 1]);
    });
    let err = http(&server.base_url())
        .fetch_group_info(14, "tok", "room-uuid", "example.test")
        .await
        .map(|bytes| bytes.len())
        .expect_err("a group info past the bound");
    assert!(is_over_the_bound(&err), "{err:?}");
}

#[tokio::test]
async fn a_property_value_past_the_bound_is_refused() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/v14/properties/labels");
        then.status(200)
            .header("content-type", "application/json")
            .body(vec![b'a'; BOUND + 1]);
    });
    let err = http(&server.base_url())
        .fetch_property_value(14, "tok", "labels")
        .await
        .map(|value| value.len())
        .expect_err("a property value past the bound");
    assert!(is_over_the_bound(&err), "{err:?}");
}

#[tokio::test]
async fn a_rejection_past_the_bound_still_names_its_status() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/v14/notifications");
        then.status(500).body(vec![b'x'; BOUND + 1]);
    });
    let err = fetch(&server).await.expect_err("a 500");
    assert!(
        matches!(&err, KupferdrahtError::BackendRejected(m) if m.contains("500")),
        "{err:?}"
    );
}
