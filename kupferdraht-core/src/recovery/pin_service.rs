// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! The PIN service's client (Step 112, ADR-0035): `POST <service>/v1/evaluate`
//! with a PIN token of the token exchange (`Authorization: Bearer`),
//! `{"keyVersion"?, "refundKey", "blindedElement"}` → `{"keyVersion",
//! "evaluatedElement", "attemptsRemaining", "attemptId"}`, bytes as standard
//! base64 with padding. The service counts every allowed attempt per user
//! and never sees the PIN; since Step 124 `POST <service>/v1/refund`
//! `{"attemptId", "signature"}` gives a successful attempt back.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;

use super::data::invalid;
use super::oprf::ELEMENT_LEN;
use super::receipt::{REFUND_KEY_LEN, SIGNATURE_LEN};
use crate::error::KupferdrahtError;
use crate::storage::remote::token_exchange::http_client;

/// The service's answer.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Evaluation {
    /// The key version the service used — written into new recovery data.
    pub(crate) key_version: u32,
    pub(crate) evaluated: [u8; ELEMENT_LEN],
    /// Attempts left before a limit, after this one.
    pub(crate) attempts_remaining: Option<u32>,
    /// Step 124: the attempt's name for its receipt — valid 15 minutes.
    pub(crate) attempt_id: String,
}

/// The service's answer on the wire.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct EvaluateAnswer {
    key_version: u32,
    evaluated_element: String,
    attempts_remaining: Option<u32>,
    attempt_id: String,
}

/// `POST /v1/refund`'s answer.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RefundAnswer {
    attempts_remaining: u32,
}

/// Have the PIN service evaluate `blinded` for the data of `refund_key` —
/// with `key_version` when opening recovery data, without when sealing new
/// data.
///
/// # Errors
/// `InvalidInput` for a plain-HTTP service without `permit_http`;
/// `Unauthorized` for a refused PIN token (`401`);
/// `InvalidRecoveryData(unsupported)` for a key version the service
/// retired (`410`); `TooManyAttempts` at a limit (`429`, `Retry-After`);
/// `ServiceUnavailable` when unreachable or `503`; `BackendRejected` for
/// any other answer; `Encoding` for a malformed one.
pub(crate) async fn evaluate(
    service_url: &str,
    permit_http: bool,
    pin_token: &str,
    key_version: Option<u32>,
    refund_key: &[u8; REFUND_KEY_LEN],
    blinded: &[u8; ELEMENT_LEN],
) -> Result<Evaluation, KupferdrahtError> {
    let url = service_endpoint(service_url, permit_http, "evaluate")?;
    let mut body = serde_json::json!({
        "refundKey": STANDARD.encode(refund_key),
        "blindedElement": STANDARD.encode(blinded),
    });
    if let Some(version) = key_version {
        body["keyVersion"] = serde_json::json!(version);
    }
    let response = http_client()?
        .post(url)
        .bearer_auth(pin_token)
        .json(&body)
        .send()
        .await
        .map_err(|e| KupferdrahtError::ServiceUnavailable(format!("PIN service: {e}")))?;
    let status = response.status().as_u16();
    if status != 200 {
        return Err(refusal(status, response.headers()));
    }
    let answer: EvaluateAnswer = response
        .json()
        .await
        .map_err(|e| KupferdrahtError::Encoding(format!("PIN service answer: {e}")))?;
    let evaluated = STANDARD
        .decode(&answer.evaluated_element)
        .ok()
        .and_then(|bytes| <[u8; ELEMENT_LEN]>::try_from(bytes).ok())
        .ok_or_else(|| KupferdrahtError::Encoding("PIN service answer: not an element".into()))?;
    Ok(Evaluation {
        key_version: answer.key_version,
        evaluated,
        attempts_remaining: answer.attempts_remaining,
        attempt_id: answer.attempt_id,
    })
}

/// Step 124: give `attempt_id` back with its receipt — the attempts left
/// after it.
///
/// # Errors
/// `InvalidInput` for a plain-HTTP service without `permit_http`;
/// `Unauthorized` for a refused PIN token (`401`); `ServiceUnavailable`
/// when unreachable or `503`; `BackendRejected` for an unknown or expired
/// attempt (`404`), a signature the service does not accept (`403`) or any
/// other answer; `Encoding` for a malformed one.
pub(crate) async fn refund(
    service_url: &str,
    permit_http: bool,
    pin_token: &str,
    attempt_id: &str,
    signature: &[u8; SIGNATURE_LEN],
) -> Result<u32, KupferdrahtError> {
    let url = service_endpoint(service_url, permit_http, "refund")?;
    let body = serde_json::json!({
        "attemptId": attempt_id,
        "signature": STANDARD.encode(signature),
    });
    let response = http_client()?
        .post(url)
        .bearer_auth(pin_token)
        .json(&body)
        .send()
        .await
        .map_err(|e| KupferdrahtError::ServiceUnavailable(format!("PIN service: {e}")))?;
    let status = response.status().as_u16();
    if status != 200 {
        return Err(refusal(status, response.headers()));
    }
    let answer: RefundAnswer = response
        .json()
        .await
        .map_err(|e| KupferdrahtError::Encoding(format!("PIN service receipt answer: {e}")))?;
    Ok(answer.attempts_remaining)
}

/// `<service>/v1/<endpoint>`, HTTPS unless plain HTTP is permitted (dev).
fn service_endpoint(
    service_url: &str,
    permit_http: bool,
    endpoint: &str,
) -> Result<String, KupferdrahtError> {
    let https = service_url.starts_with("https://");
    let http = service_url.starts_with("http://");
    if !(https || (http && permit_http)) {
        return Err(KupferdrahtError::InvalidInput(format!(
            "PIN service {service_url}: HTTPS required (plain HTTP only with permitHttp)"
        )));
    }
    Ok(format!(
        "{}/v1/{endpoint}",
        service_url.trim_end_matches('/')
    ))
}

/// The error an answer other than `200` stands for.
fn refusal(status: u16, headers: &reqwest::header::HeaderMap) -> KupferdrahtError {
    match status {
        401 => KupferdrahtError::Unauthorized,
        410 => invalid("unsupported"),
        429 => KupferdrahtError::TooManyAttempts {
            retry_after_secs: headers
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse().ok()),
        },
        503 => KupferdrahtError::ServiceUnavailable("the PIN service is unavailable".into()),
        _ => KupferdrahtError::BackendRejected(format!("PIN service {status}")),
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use httpmock::Method::POST;
    use httpmock::MockServer;
    use serde_json::json;

    use super::*;

    const BLINDED: [u8; ELEMENT_LEN] = [2; ELEMENT_LEN];
    const EVALUATED: [u8; ELEMENT_LEN] = [3; ELEMENT_LEN];
    const REFUND_KEY: [u8; REFUND_KEY_LEN] = [4; REFUND_KEY_LEN];
    const SIGNATURE: [u8; SIGNATURE_LEN] = [5; SIGNATURE_LEN];
    const ATTEMPT: &str = "AAECAwQFBgcICQoLDA0ODw==";

    fn answer(server: &MockServer, status: u16, body: &str, retry_after: Option<&str>) {
        server.mock(|when, then| {
            when.method(POST).path("/v1/evaluate");
            let then = then
                .status(status)
                .header("content-type", "application/json")
                .body(body);
            if let Some(seconds) = retry_after {
                then.header("retry-after", seconds);
            }
        });
    }

    async fn call(
        server: &MockServer,
        key_version: Option<u32>,
    ) -> Result<Evaluation, KupferdrahtError> {
        evaluate(
            &server.base_url(),
            true,
            "pin.jwt",
            key_version,
            &REFUND_KEY,
            &BLINDED,
        )
        .await
    }

    /// Step 124: the refund key goes along, the attempt's id comes back.
    #[tokio::test]
    async fn the_request_carries_the_token_the_element_the_key_version_and_the_refund_key() {
        let server = MockServer::start();
        let request = server.mock(|when, then| {
            when.method(POST)
                .path("/v1/evaluate")
                .header("authorization", "Bearer pin.jwt")
                .json_body(json!({
                    "keyVersion": 2,
                    "refundKey": STANDARD.encode(REFUND_KEY),
                    "blindedElement": STANDARD.encode(BLINDED)
                }));
            then.status(200)
                .header("content-type", "application/json")
                .body(
                    json!({
                        "keyVersion": 2,
                        "evaluatedElement": STANDARD.encode(EVALUATED),
                        "attemptsRemaining": 4,
                        "attemptId": ATTEMPT
                    })
                    .to_string(),
                );
        });

        let evaluation = call(&server, Some(2)).await.expect("evaluated");

        request.assert();
        assert_eq!(
            evaluation,
            Evaluation {
                key_version: 2,
                evaluated: EVALUATED,
                attempts_remaining: Some(4),
                attempt_id: ATTEMPT.into(),
            }
        );
    }

    /// New recovery data: the service picks its current key version.
    #[tokio::test]
    async fn sealing_sends_no_key_version() {
        let server = MockServer::start();
        let request = server.mock(|when, then| {
            when.method(POST).path("/v1/evaluate").json_body(json!({
                "refundKey": STANDARD.encode(REFUND_KEY),
                "blindedElement": STANDARD.encode(BLINDED)
            }));
            then.status(200)
                .header("content-type", "application/json")
                .body(
                    json!({
                        "keyVersion": 1,
                        "evaluatedElement": STANDARD.encode(EVALUATED),
                        "attemptsRemaining": 11,
                        "attemptId": ATTEMPT
                    })
                    .to_string(),
                );
        });

        let evaluation = call(&server, None).await.expect("evaluated");

        request.assert();
        assert_eq!(evaluation.key_version, 1);
    }

    #[tokio::test]
    async fn a_refused_token_is_unauthorized() {
        let server = MockServer::start();
        answer(&server, 401, r#"{"error":"unauthorized"}"#, None);
        assert!(matches!(
            call(&server, Some(1)).await,
            Err(KupferdrahtError::Unauthorized)
        ));
    }

    #[tokio::test]
    async fn a_retired_key_version_is_unsupported_recovery_data() {
        let server = MockServer::start();
        answer(&server, 410, r#"{"error":"key_version_unavailable"}"#, None);
        match call(&server, Some(1)).await {
            Err(KupferdrahtError::InvalidRecoveryData { reason }) => {
                assert_eq!(reason, "unsupported");
            }
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn the_attempt_limit_is_too_many_attempts_with_the_wait() {
        let server = MockServer::start();
        answer(
            &server,
            429,
            r#"{"error":"too_many_attempts"}"#,
            Some("1800"),
        );
        assert!(matches!(
            call(&server, Some(1)).await,
            Err(KupferdrahtError::TooManyAttempts {
                retry_after_secs: Some(1800)
            })
        ));
    }

    #[tokio::test]
    async fn an_unavailable_service_is_service_unavailable() {
        let server = MockServer::start();
        answer(&server, 503, r#"{"error":"unavailable"}"#, None);
        assert!(matches!(
            call(&server, Some(1)).await,
            Err(KupferdrahtError::ServiceUnavailable(_))
        ));
    }

    #[tokio::test]
    async fn an_unreachable_service_is_service_unavailable() {
        let result = evaluate(
            "http://127.0.0.1:1",
            true,
            "pin.jwt",
            None,
            &REFUND_KEY,
            &BLINDED,
        )
        .await;
        assert!(matches!(
            result,
            Err(KupferdrahtError::ServiceUnavailable(_))
        ));
    }

    #[tokio::test]
    async fn plain_http_is_refused_without_permission() {
        let result = evaluate(
            "http://pin.example",
            false,
            "pin.jwt",
            None,
            &REFUND_KEY,
            &BLINDED,
        )
        .await;
        assert!(matches!(result, Err(KupferdrahtError::InvalidInput(_))));
    }

    #[tokio::test]
    async fn a_malformed_element_is_refused() {
        let server = MockServer::start();
        answer(
            &server,
            200,
            r#"{"keyVersion":1,"evaluatedElement":"AAAA","attemptsRemaining":4,"attemptId":"AAECAwQFBgcICQoLDA0ODw=="}"#,
            None,
        );
        assert!(matches!(
            call(&server, None).await,
            Err(KupferdrahtError::Encoding(_))
        ));
    }

    fn refund_answer(server: &MockServer, status: u16, body: &str) {
        server.mock(|when, then| {
            when.method(POST).path("/v1/refund");
            then.status(status)
                .header("content-type", "application/json")
                .body(body);
        });
    }

    async fn give_back(server: &MockServer) -> Result<u32, KupferdrahtError> {
        refund(&server.base_url(), true, "pin.jwt", ATTEMPT, &SIGNATURE).await
    }

    /// Step 124: the receipt names the attempt and carries the signature,
    /// with the PIN token.
    #[tokio::test]
    async fn a_receipt_carries_the_token_the_attempt_and_the_signature() {
        let server = MockServer::start();
        let request = server.mock(|when, then| {
            when.method(POST)
                .path("/v1/refund")
                .header("authorization", "Bearer pin.jwt")
                .json_body(json!({"attemptId": ATTEMPT, "signature": STANDARD.encode(SIGNATURE)}));
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"attemptsRemaining":5}"#);
        });

        let remaining = give_back(&server).await.expect("refunded");

        request.assert();
        assert_eq!(remaining, 5);
    }

    #[tokio::test]
    async fn a_refused_receipt_is_backend_rejected() {
        for (status, body) in [
            (404, r#"{"error":"unknown_attempt"}"#),
            (403, r#"{"error":"invalid_signature"}"#),
        ] {
            let server = MockServer::start();
            refund_answer(&server, status, body);
            assert!(
                matches!(
                    give_back(&server).await,
                    Err(KupferdrahtError::BackendRejected(_))
                ),
                "{status}"
            );
        }
    }

    #[tokio::test]
    async fn a_receipt_with_a_refused_token_is_unauthorized() {
        let server = MockServer::start();
        refund_answer(&server, 401, r#"{"error":"unauthorized"}"#);
        assert!(matches!(
            give_back(&server).await,
            Err(KupferdrahtError::Unauthorized)
        ));
    }
}
