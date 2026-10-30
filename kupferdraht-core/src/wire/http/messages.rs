// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Application-message HTTP surface — `POST /v{N}/mls/messages`.
//!
//! Wire's text-send endpoint accepts a single TLS-serialized
//! `MlsMessage(PrivateMessage)` body, content-type `message/mls`.
//! Kupferdraht produces that body via `MlsClient::encrypt_text_message`
//! (Step 4c) over a `GenericMessage` Protobuf payload (Step 4b).
//!
//! Step 4 only writes plain-text application messages; commit /
//! welcome flows live in Step 3 and Step 6 respectively.

use tracing::debug;

use crate::error::KupferdrahtError;

use super::HttpClient;

#[cfg_attr(not(test), allow(dead_code))]
impl HttpClient {
    /// `POST /v{api_version}/mls/messages` — uploads a single
    /// MLS application message (TLS-serialized
    /// `MlsMessage(PrivateMessage)` ciphertext).
    ///
    /// Wire's nginx scopes the call to a specific client device via
    /// the `Z-Client` header it injects from the access token, so
    /// the caller must have gone through `rebind_access_token`
    /// first (LEARNINGS §1.4).
    ///
    /// # Errors
    /// * `BackendRejected` on non-2xx statuses. Wire's MLS error
    ///   labels (`mls-stale-message`, `mls-protocol-error`,
    ///   `mls-client-mismatch`, …) come through verbatim.
    /// * `Network` for transport failures.
    pub async fn send_mls_message(
        &self,
        api_version: u32,
        access_token: &str,
        ciphertext: &[u8],
    ) -> Result<(), KupferdrahtError> {
        debug_assert!(!access_token.is_empty(), "access_token must be non-empty");
        debug_assert!(!ciphertext.is_empty(), "ciphertext must be non-empty");
        let path = format!("/v{api_version}/mls/messages");
        let url = self.url(&path)?;
        debug!(%url, byte_len = ciphertext.len(), "POST mls/messages");
        let req = self
            .client
            .post(url)
            .bearer_auth(access_token)
            .header("content-type", "message/mls")
            .body(ciphertext.to_vec());
        self.send_no_body(req, "send_mls_message").await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::Method::POST;
    use httpmock::{MockServer, prelude::HttpMockRequest};

    fn http(base_url: &str) -> HttpClient {
        HttpClient::new(base_url, true).expect("HttpClient::new (permit_http=true)")
    }

    const APP_CIPHERTEXT: &[u8] = &[0x00u8, 0x01, 0x00, 0x02, 0xCA, 0xFE, 0xBA, 0xBE];

    fn body_equals_ciphertext(req: &HttpMockRequest) -> bool {
        req.body_ref() == APP_CIPHERTEXT
    }

    #[tokio::test]
    async fn send_mls_message_posts_message_mls_body_verbatim() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/v14/mls/messages")
                .header("authorization", "Bearer tok")
                .header("content-type", "message/mls")
                .is_true(body_equals_ciphertext);
            then.status(201)
                .header("content-type", "application/json")
                .body(r#"{"events":[],"time":"2026-04-25T10:00:00.000Z"}"#);
        });
        http(&server.base_url())
            .send_mls_message(14, "tok", APP_CIPHERTEXT)
            .await
            .expect("send_mls_message must succeed on 201");
        mock.assert_calls(1);
    }

    #[tokio::test]
    async fn send_mls_message_surfaces_stale_message() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/v14/mls/messages");
            then.status(409)
                .header("content-type", "application/json")
                .body(r#"{"code":409,"label":"mls-stale-message","message":"epoch race"}"#);
        });
        let err = http(&server.base_url())
            .send_mls_message(14, "tok", APP_CIPHERTEXT)
            .await
            .expect_err("409 must propagate");
        // Step 37: a stale-epoch 409 is typed so the send path can self-heal.
        assert!(
            matches!(err, KupferdrahtError::MlsStaleMessage),
            "expected MlsStaleMessage, got {err:?}"
        );
    }

    #[tokio::test]
    async fn send_mls_message_409_other_label_stays_backend_rejected() {
        // Only `mls-stale-message` is special-cased; a different 409 label must
        // still surface as the opaque BackendRejected. Step 37.
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/v14/mls/messages");
            then.status(409)
                .header("content-type", "application/json")
                .body(r#"{"code":409,"label":"mls-client-mismatch","message":"nope"}"#);
        });
        let err = http(&server.base_url())
            .send_mls_message(14, "tok", APP_CIPHERTEXT)
            .await
            .expect_err("409 must propagate");
        assert!(
            matches!(err, KupferdrahtError::BackendRejected(ref m) if m.contains("mls-client-mismatch")),
            "expected BackendRejected, got {err:?}"
        );
    }

    #[tokio::test]
    async fn send_mls_message_surfaces_protocol_error() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/v14/mls/messages");
            then.status(400)
                .header("content-type", "application/json")
                .body(r#"{"code":400,"label":"mls-protocol-error","message":"bad ct"}"#);
        });
        let err = http(&server.base_url())
            .send_mls_message(14, "tok", APP_CIPHERTEXT)
            .await
            .expect_err("400 must propagate");
        assert!(matches!(err, KupferdrahtError::BackendRejected(_)));
    }
}
