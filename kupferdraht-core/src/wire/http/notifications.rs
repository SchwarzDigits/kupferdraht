// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! REST notification-fetch — `GET /v{N}/notifications`.
//!
//! The same event payloads the WebSocket delivers, just pulled.
//! Used by the sync service's REST drain (`sync/drain.rs`) and by
//! `kupferdraht-cli sync-fetch` as a one-shot pull with no long-lived
//! connection.

use tracing::debug;

use crate::error::KupferdrahtError;
use crate::wire::notification_dto::NotificationListResponse;

use super::{HttpClient, WireErrorBody, decode_response};

const OP: &str = "fetch_notifications";

/// Outcome of one `GET /v{N}/notifications` call.
#[derive(Debug)]
pub enum FetchedNotifications {
    /// `200` — one page of the queue.
    Page(NotificationListResponse),
    /// `404` with label `not-found` on a request that sent `since`: the
    /// queue no longer holds that id (backend-spec §11.4).
    SinceNotFound,
}

#[cfg_attr(not(test), allow(dead_code))]
impl HttpClient {
    /// `GET /v{api_version}/notifications?client=<id>&size=<n>[&since=<id>]`
    /// — fetch one page of pending notifications for this device.
    ///
    /// Wire enforces a minimum size of 100 and caps the upper bound
    /// at ~1000. Pages are ordered oldest-first; caller chains
    /// `?since=<last id>` until `has_more` is `false`.
    ///
    /// A `404` with label `not-found` on a request with `since` is
    /// [`FetchedNotifications::SinceNotFound`] — the caller falls back
    /// to a marker-less fetch.
    ///
    /// # Errors
    /// * `BackendRejected` on every other non-2xx status — a `404`
    ///   without that label (a proxy's error page) included.
    /// * `Network` for transport failures.
    /// * `Encoding` for malformed response bodies.
    pub async fn fetch_notifications(
        &self,
        api_version: u32,
        access_token: &str,
        client_id: &str,
        since: Option<&str>,
        size: u32,
    ) -> Result<FetchedNotifications, KupferdrahtError> {
        debug_assert!(!access_token.is_empty(), "access_token must be non-empty");
        debug_assert!(!client_id.is_empty(), "client_id must be non-empty");
        debug_assert!(size >= 100, "Wire enforces minimum size 100");
        let path = format!("/v{api_version}/notifications");
        let mut url = self.url(&path)?;
        {
            let mut q = url.query_pairs_mut();
            q.append_pair("client", client_id);
            q.append_pair("size", &size.to_string());
            if let Some(s) = since {
                q.append_pair("since", s);
            }
        }
        debug!(%url, "GET notifications");
        let req = self.client.get(url).bearer_auth(access_token);
        let resp = self.send_with_auth_retry(req, OP).await?;
        if since.is_some() && resp.status() == reqwest::StatusCode::NOT_FOUND {
            return since_not_found_or_rejected(resp).await;
        }
        decode_response::<NotificationListResponse>(resp, OP)
            .await
            .map(FetchedNotifications::Page)
    }
}

/// Classify a `404` to a request that sent `since`: the label
/// `not-found` means the queue no longer holds that id; any other `404`
/// — a proxy's error page, another label — is rejected. Step 102.
async fn since_not_found_or_rejected(
    resp: reqwest::Response,
) -> Result<FetchedNotifications, KupferdrahtError> {
    let status = resp.status();
    debug_assert_eq!(
        status,
        reqwest::StatusCode::NOT_FOUND,
        "caller routes 404s only"
    );
    let body = resp.text().await.unwrap_or_default();
    let label = serde_json::from_str::<WireErrorBody>(&body)
        .map(|b| b.label)
        .unwrap_or_default();
    if label == "not-found" {
        return Ok(FetchedNotifications::SinceNotFound);
    }
    Err(KupferdrahtError::BackendRejected(format!(
        "{OP} {status}: {body}"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::Method::GET;
    use httpmock::MockServer;

    fn http(base_url: &str) -> HttpClient {
        HttpClient::new(base_url, true).expect("HttpClient::new (permit_http=true)")
    }

    #[tokio::test]
    async fn fetch_notifications_first_page_omits_since() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(GET)
                .path("/v14/notifications")
                .header("authorization", "Bearer tok")
                .query_param("client", "dev-1")
                .query_param("size", "100")
                .is_true(|req| !req.query_params().iter().any(|(k, _)| k == "since"));
            then.status(200)
                .header("content-type", "application/json")
                .body(
                    r#"{"notifications":[{"id":"n-1","payload":[{"type":"x"}]}],"has_more":false}"#,
                );
        });
        let FetchedNotifications::Page(resp) = http(&server.base_url())
            .fetch_notifications(14, "tok", "dev-1", None, 100)
            .await
            .expect("fetch_notifications")
        else {
            panic!("a 200 is a page");
        };
        assert_eq!(resp.notifications.len(), 1);
        assert_eq!(resp.notifications[0].id, "n-1");
        assert!(!resp.has_more);
        mock.assert_calls(1);
    }

    #[tokio::test]
    async fn fetch_notifications_paged_request_includes_since() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(GET)
                .path("/v14/notifications")
                .query_param("since", "n-1");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"notifications":[],"has_more":false}"#);
        });
        http(&server.base_url())
            .fetch_notifications(14, "tok", "dev-1", Some("n-1"), 100)
            .await
            .expect("paged fetch");
        mock.assert_calls(1);
    }

    /// Mount a `404` for `GET /v14/notifications` with the given body.
    fn mount_404(server: &MockServer, content_type: &str, body: &str) {
        server.mock(|when, then| {
            when.method(GET).path("/v14/notifications");
            then.status(404)
                .header("content-type", content_type)
                .body(body);
        });
    }

    async fn fetch(
        server: &MockServer,
        since: Option<&str>,
    ) -> Result<FetchedNotifications, KupferdrahtError> {
        http(&server.base_url())
            .fetch_notifications(14, "tok", "dev-1", since, 100)
            .await
    }

    const NOT_FOUND: &str =
        r#"{"code":404,"label":"not-found","message":"Some notifications not found"}"#;

    #[tokio::test]
    async fn a_404_not_found_on_since_is_since_not_found() {
        let server = MockServer::start();
        mount_404(&server, "application/json;charset=utf-8", NOT_FOUND);
        let fetched = fetch(&server, Some("gone")).await;
        assert!(
            matches!(fetched, Ok(FetchedNotifications::SinceNotFound)),
            "the backend's labelled 404 on `since` is the not-found outcome: {fetched:?}"
        );
    }

    #[tokio::test]
    async fn a_404_error_page_is_rejected() {
        let server = MockServer::start();
        mount_404(
            &server,
            "text/html",
            "<html><body><h1>404 Not Found</h1>not-found</body></html>",
        );
        let fetched = fetch(&server, Some("kept")).await;
        assert!(
            matches!(fetched, Err(KupferdrahtError::BackendRejected(_))),
            "a 404 without the backend's error envelope is an error: {fetched:?}"
        );
    }

    #[tokio::test]
    async fn a_404_with_another_label_is_rejected() {
        let server = MockServer::start();
        mount_404(
            &server,
            "application/json",
            r#"{"code":404,"label":"no-client","message":"Client not found"}"#,
        );
        let fetched = fetch(&server, Some("kept")).await;
        assert!(
            matches!(fetched, Err(KupferdrahtError::BackendRejected(_))),
            "only the label `not-found` means the cursor is gone: {fetched:?}"
        );
    }

    /// Mount a `429` for `GET /v14/notifications`, with `Retry-After`
    /// when given.
    fn mount_429(server: &MockServer, retry_after: Option<&str>) {
        server.mock(|when, then| {
            when.method(GET).path("/v14/notifications");
            let then = then.status(429).header("content-type", "application/json");
            let then = match retry_after {
                Some(value) => then.header("retry-after", value),
                None => then,
            };
            then.body(
                r#"{"code":429,"label":"too-many-requests","message":"Please try again later."}"#,
            );
        });
    }

    // Step 103 — the shared rejection path, exercised through this endpoint.
    #[tokio::test]
    async fn a_429_is_rate_limited_with_its_retry_after() {
        let server = MockServer::start();
        mount_429(&server, Some("7"));
        let fetched = fetch(&server, None).await;
        assert!(
            matches!(
                fetched,
                Err(KupferdrahtError::RateLimited {
                    retry_after_secs: Some(7)
                })
            ),
            "{fetched:?}"
        );
    }

    #[tokio::test]
    async fn a_429_without_retry_after_carries_no_hint() {
        let server = MockServer::start();
        mount_429(&server, None);
        let fetched = fetch(&server, None).await;
        assert!(
            matches!(
                fetched,
                Err(KupferdrahtError::RateLimited {
                    retry_after_secs: None
                })
            ),
            "{fetched:?}"
        );
    }

    #[tokio::test]
    async fn a_429_with_an_http_date_carries_no_hint() {
        let server = MockServer::start();
        mount_429(&server, Some("Wed, 21 Oct 2026 07:28:00 GMT"));
        let fetched = fetch(&server, None).await;
        assert!(
            matches!(
                fetched,
                Err(KupferdrahtError::RateLimited {
                    retry_after_secs: None
                })
            ),
            "{fetched:?}"
        );
    }

    #[tokio::test]
    async fn a_404_not_found_without_since_is_rejected() {
        let server = MockServer::start();
        mount_404(&server, "application/json", NOT_FOUND);
        let fetched = fetch(&server, None).await;
        assert!(
            matches!(fetched, Err(KupferdrahtError::BackendRejected(_))),
            "without `since` there is no cursor to be gone: {fetched:?}"
        );
    }
}
