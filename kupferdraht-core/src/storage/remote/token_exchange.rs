// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! The token exchange's client (Step 111b, ADR-0036): a storage token for
//! the account's storage-login key, bought with the Wire access token.
//!
//! `POST <exchange>/v1/token`, `Authorization: Bearer <Wire access token>`,
//! `{"audience": "storage", "publicKey": "<base64url, no padding>"}` →
//! `{"token": "<JWT>", "expiresIn": <seconds>}`. The address comes from the
//! embedder's configuration — never built in.

use std::time::Duration;

use base64::Engine as _;
use zeroize::Zeroizing;

use crate::error::KupferdrahtError;

/// The answer of `POST /v1/token`.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct TokenAnswer {
    token: String,
    expires_in: u64,
}

/// A storage token and its lifetime.
pub(crate) struct StorageToken {
    pub(crate) token: Zeroizing<String>,
    pub(crate) expires_in: Duration,
}

impl StorageToken {
    /// When to ask for the next one: at half the lifetime.
    pub(crate) fn renew_after(&self) -> Duration {
        self.expires_in / 2
    }
}

/// Buy a storage token for `public_key` with `wire_token`.
///
/// # Errors
/// `InvalidInput` for a plain-HTTP exchange without `permit_http`;
/// `Unauthorized` when the exchange (Wire) refuses the Wire token;
/// `StorageAccessDenied` when the admission refuses the user (`403`);
/// `RateLimited` for `429` (with `Retry-After`); `ServiceUnavailable` when
/// the exchange is unreachable or answers `503`; `BackendRejected` for any
/// other answer.
pub(crate) async fn fetch_storage_token(
    exchange_url: &str,
    permit_http: bool,
    wire_token: &str,
    public_key: &[u8; 32],
) -> Result<StorageToken, KupferdrahtError> {
    let body = serde_json::json!({
        "audience": "storage",
        "publicKey": base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(public_key),
    });
    exchange(exchange_url, permit_http, wire_token, &body).await
}

/// `POST <exchange>/v1/token` with `body`, bought with `wire_token`.
async fn exchange(
    exchange_url: &str,
    permit_http: bool,
    wire_token: &str,
    body: &serde_json::Value,
) -> Result<StorageToken, KupferdrahtError> {
    let url = token_url(exchange_url, permit_http)?;
    let response = http_client()?
        .post(url)
        .bearer_auth(wire_token)
        .json(body)
        .send()
        .await
        .map_err(|e| KupferdrahtError::ServiceUnavailable(format!("token exchange: {e}")))?;
    let status = response.status().as_u16();
    if status == 200 {
        let answer: TokenAnswer = response
            .json()
            .await
            .map_err(|e| KupferdrahtError::Encoding(format!("token exchange answer: {e}")))?;
        debug_assert!(!answer.token.is_empty(), "the exchange issues a token");
        return Ok(StorageToken {
            token: Zeroizing::new(answer.token),
            expires_in: Duration::from_secs(answer.expires_in),
        });
    }
    Err(refusal(status, response).await)
}

/// A PIN token for the PIN service (Step 112, ADR-0035): `{"audience":
/// "pin"}`, bought with `wire_token`, no key bound to it. Short-lived; used
/// at once.
///
/// # Errors
/// As [`fetch_storage_token`].
pub(crate) async fn fetch_pin_token(
    exchange_url: &str,
    permit_http: bool,
    wire_token: &str,
) -> Result<Zeroizing<String>, KupferdrahtError> {
    let body = serde_json::json!({ "audience": "pin" });
    Ok(exchange(exchange_url, permit_http, wire_token, &body)
        .await?
        .token)
}

/// `<exchange>/v1/token`, HTTPS unless plain HTTP is permitted (dev).
fn token_url(exchange_url: &str, permit_http: bool) -> Result<String, KupferdrahtError> {
    let https = exchange_url.starts_with("https://");
    let http = exchange_url.starts_with("http://");
    if !(https || (http && permit_http)) {
        return Err(KupferdrahtError::InvalidInput(format!(
            "token exchange {exchange_url}: HTTPS required (plain HTTP only with permitHttp)"
        )));
    }
    Ok(format!("{}/v1/token", exchange_url.trim_end_matches('/')))
}

pub(crate) fn http_client() -> Result<reqwest::Client, KupferdrahtError> {
    let builder = reqwest::Client::builder();
    #[cfg(not(target_arch = "wasm32"))]
    let builder = builder.timeout(Duration::from_secs(10));
    builder
        .build()
        .map_err(|e| KupferdrahtError::Environment(format!("token exchange client: {e}")))
}

/// The error an answer other than `200` stands for.
async fn refusal(status: u16, response: reqwest::Response) -> KupferdrahtError {
    match status {
        401 => KupferdrahtError::Unauthorized,
        403 => KupferdrahtError::StorageAccessDenied(
            "the token exchange does not admit this user".into(),
        ),
        429 => KupferdrahtError::RateLimited {
            retry_after_secs: response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse().ok()),
        },
        503 => KupferdrahtError::ServiceUnavailable("the token exchange is overloaded".into()),
        _ => {
            let text = response.text().await.unwrap_or_default();
            KupferdrahtError::BackendRejected(format!("token exchange {status}: {text}"))
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use httpmock::Method::POST;
    use httpmock::MockServer;

    use super::*;

    const KEY: [u8; 32] = [7; 32];
    /// base64url of 32 × 0x07, no padding.
    const KEY_B64: &str = "BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc";

    fn answer(server: &MockServer, status: u16, body: &str, retry_after: Option<&str>) {
        server.mock(|when, then| {
            when.method(POST).path("/v1/token");
            let then = then
                .status(status)
                .header("content-type", "application/json")
                .body(body);
            if let Some(seconds) = retry_after {
                then.header("retry-after", seconds);
            }
        });
    }

    #[tokio::test]
    async fn the_request_carries_the_wire_token_the_audience_and_the_key() {
        let server = MockServer::start();
        let exchange = server.mock(|when, then| {
            when.method(POST)
                .path("/v1/token")
                .header("authorization", "Bearer wire-token")
                .json_body(serde_json::json!({ "audience": "storage", "publicKey": KEY_B64 }));
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"token":"storage.jwt","expiresIn":3600}"#);
        });

        let token = fetch_storage_token(&server.base_url(), true, "wire-token", &KEY)
            .await
            .expect("a token");

        exchange.assert();
        assert_eq!(token.token.as_str(), "storage.jwt");
        assert_eq!(token.expires_in, Duration::from_secs(3600));
        assert_eq!(
            token.renew_after(),
            Duration::from_mins(30),
            "at half its lifetime"
        );
    }

    /// Step 112: a PIN token carries only the audience.
    #[tokio::test]
    async fn a_pin_token_asks_for_the_pin_audience() {
        let server = MockServer::start();
        let exchange = server.mock(|when, then| {
            when.method(POST)
                .path("/v1/token")
                .header("authorization", "Bearer wire.token")
                .json_body(serde_json::json!({"audience": "pin"}));
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"token":"pin.jwt","expiresIn":600}"#);
        });

        let token = fetch_pin_token(&server.base_url(), true, "wire.token")
            .await
            .expect("a token");

        exchange.assert();
        assert_eq!(token.as_str(), "pin.jwt");
    }

    #[tokio::test]
    async fn plain_http_is_refused_without_permission() {
        let result = fetch_storage_token("http://exchange.example", false, "t", &KEY).await;
        assert!(
            matches!(result, Err(KupferdrahtError::InvalidInput(_))),
            "{:?}",
            result.err()
        );
    }

    #[tokio::test]
    async fn a_refused_wire_token_is_unauthorized() {
        let server = MockServer::start();
        answer(&server, 401, r#"{"error":"unauthorized"}"#, None);
        let result = fetch_storage_token(&server.base_url(), true, "t", &KEY).await;
        assert!(
            matches!(result, Err(KupferdrahtError::Unauthorized)),
            "{:?}",
            result.err()
        );
    }

    #[tokio::test]
    async fn a_user_the_admission_refuses_has_no_storage_access() {
        let server = MockServer::start();
        answer(&server, 403, r#"{"error":"not_allowed"}"#, None);
        let result = fetch_storage_token(&server.base_url(), true, "t", &KEY).await;
        assert!(
            matches!(result, Err(KupferdrahtError::StorageAccessDenied(_))),
            "{:?}",
            result.err()
        );
    }

    #[tokio::test]
    async fn a_limit_is_rate_limited_with_its_retry_after() {
        let server = MockServer::start();
        answer(&server, 429, r#"{"error":"too_many_requests"}"#, Some("42"));
        let result = fetch_storage_token(&server.base_url(), true, "t", &KEY).await;
        assert!(
            matches!(
                result,
                Err(KupferdrahtError::RateLimited {
                    retry_after_secs: Some(42)
                })
            ),
            "{:?}",
            result.err()
        );
    }

    #[tokio::test]
    async fn an_unavailable_exchange_is_service_unavailable() {
        let server = MockServer::start();
        answer(&server, 503, r#"{"error":"unavailable"}"#, Some("1"));
        let result = fetch_storage_token(&server.base_url(), true, "t", &KEY).await;
        assert!(
            matches!(result, Err(KupferdrahtError::ServiceUnavailable(_))),
            "{:?}",
            result.err()
        );
    }

    #[tokio::test]
    async fn an_unreachable_exchange_is_service_unavailable() {
        // Nothing listens on port 9 of localhost.
        let result = fetch_storage_token("http://127.0.0.1:9", true, "t", &KEY).await;
        assert!(
            matches!(result, Err(KupferdrahtError::ServiceUnavailable(_))),
            "{:?}",
            result.err()
        );
    }
}
