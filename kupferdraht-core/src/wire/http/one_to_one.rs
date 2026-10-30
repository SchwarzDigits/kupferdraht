// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! 1:1 conversation HTTP surface (Step 14a):
//! `GET /v{N}/one2one-conversations/{usr_domain}/{usr}` — fetch the
//! Wire 1:1 between the caller and the named peer, creating it
//! server-side if it does not yet exist.
//!
//! Wire's 1:1 conversations are first-class server-side (a dedicated
//! endpoint + the `ConvType::DirectMessage` discriminator on the
//! conversation row), unlike Matrix's "normal room with `is_direct`
//! flag" model. The single GET endpoint is the canonical surface:
//! the first invocation against a fresh `(self, peer)` pair causes
//! the server to register the conversation, every subsequent
//! invocation returns the existing one. There is no separate
//! "create" verb on a live Wire v14 backend — the swagger lists a
//! POST `/one2one-conversations` route, but the deployed backend
//! rejects it at nginx with 404 (recorded in
//! `docs/wire/api-notes.md` §"Step 14a"). Kupferdraht therefore models
//! `create_dm` and the fetch surface as the same single GET call.
//!
//! The response wraps the conversation in an `MlsOne2OneConversation`
//! envelope alongside the External Senders `public_keys`. Kupferdraht
//! already holds those keys (Step 1c-4); the wrapper is unwrapped
//! at the boundary so callers see a flat `OwnConversation`.

use serde::Deserialize;
use tracing::debug;

use crate::error::KupferdrahtError;
use crate::wire::dto::{MlsKeysByPurpose, OwnConversation};

use super::{HttpClient, reject_from_response};

/// Wire's response envelope for the MLS 1:1 fetch
/// (`GET /v{N}/one2one-conversations/{domain}/{user}`). The
/// `conversation` field carries the full `OwnConversation` Kupferdraht
/// already consumes via `POST /conversations/list`; `public_keys`
/// echoes the same External Senders bundle login fetches from
/// `GET /v{N}/mls/public-keys`.
#[derive(Debug, Clone, Deserialize)]
struct MlsOne2OneConversation {
    conversation: OwnConversation,
    #[allow(dead_code)] // tolerated + unwrapped, not consumed here
    public_keys: MlsKeysByPurpose,
}

impl HttpClient {
    /// `GET /v{api_version}/one2one-conversations/{usr_domain}/{usr}` —
    /// fetch the Wire 1:1 conversation between the caller and the
    /// peer identified by `(peer_domain, peer_uuid)`. Server-side
    /// idempotent: the first invocation registers the conversation,
    /// later invocations return the existing one. Same call shape
    /// for "create" and "look up".
    ///
    /// Returns the unwrapped `OwnConversation`; the server's
    /// `MlsOne2OneConversation` envelope carries the `public_keys`
    /// alongside it but Kupferdraht already has those from login.
    ///
    /// # Errors
    /// * `BackendRejected` on non-2xx statuses.
    /// * `Network` for transport failures.
    /// * `Encoding` for malformed response bodies.
    pub async fn fetch_one_to_one_conversation(
        &self,
        api_version: u32,
        access_token: &str,
        peer_domain: &str,
        peer_uuid: &str,
    ) -> Result<OwnConversation, KupferdrahtError> {
        debug_assert!(!access_token.is_empty(), "access_token must be non-empty");
        debug_assert!(!peer_domain.is_empty(), "peer_domain must be non-empty");
        debug_assert!(!peer_uuid.is_empty(), "peer_uuid must be non-empty");
        let path = format!("/v{api_version}/one2one-conversations/{peer_domain}/{peer_uuid}");
        let url = self.url(&path)?;
        debug!(%url, "GET one2one-conversations/{{d}}/{{u}}");
        let resp = self
            .client
            .get(url)
            .bearer_auth(access_token)
            .send()
            .await
            .map_err(|e| KupferdrahtError::Network(format!("GET {path}: {e}")))?;
        if !resp.status().is_success() {
            return Err(reject_from_response(resp, "fetch_one_to_one_conversation").await);
        }
        let envelope = resp
            .json::<MlsOne2OneConversation>()
            .await
            .map_err(|e| KupferdrahtError::Encoding(format!("one2one-conversations body: {e}")))?;
        Ok(envelope.conversation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::Method::GET;
    use httpmock::MockServer;

    fn http(base_url: &str) -> HttpClient {
        HttpClient::new(base_url, true).expect("HttpClient::new (permit_http=true)")
    }

    fn one2one_envelope(
        conv_uuid: &str,
        conv_domain: &str,
        self_uuid: &str,
        peer_uuid: &str,
        peer_domain: &str,
    ) -> String {
        format!(
            r#"{{
                "conversation": {{
                  "qualified_id":{{"id":"{conv_uuid}","domain":"{conv_domain}"}},
                  "type":2,
                  "protocol":"mls",
                  "epoch":0,
                  "group_id":"Z3JwLW9uZQ==",
                  "name":null,
                  "members":{{
                    "self":{{"qualified_id":{{"id":"{self_uuid}","domain":"example.test"}}}},
                    "others":[
                      {{"qualified_id":{{"id":"{peer_uuid}","domain":"{peer_domain}"}}}}
                    ]
                  }}
                }},
                "public_keys": {{
                  "removal": {{
                    "ed25519":"ZWQ=",
                    "ecdsa_secp256r1_sha256":"cDI1Ng==",
                    "ecdsa_secp384r1_sha384":"cDM4NA==",
                    "ecdsa_secp521r1_sha512":"cDUyMQ=="
                  }}
                }}
            }}"#
        )
    }

    #[tokio::test]
    async fn fetch_one_to_one_hits_get_with_peer_domain_and_uuid_in_path() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(GET)
                .path("/v14/one2one-conversations/wire.com/peer-uuid")
                .header("authorization", "Bearer tok");
            then.status(200)
                .header("content-type", "application/json")
                .body(one2one_envelope(
                    "dm-uuid",
                    "wire.com",
                    "self-uuid",
                    "peer-uuid",
                    "wire.com",
                ));
        });

        let conv = http(&server.base_url())
            .fetch_one_to_one_conversation(14, "tok", "wire.com", "peer-uuid")
            .await
            .expect("fetch must succeed");
        assert_eq!(conv.qualified_id.id, "dm-uuid");
        assert_eq!(conv.qualified_id.domain, "wire.com");
        assert_eq!(conv.members.others.len(), 1);
        mock.assert_calls(1);
    }

    #[tokio::test]
    async fn fetch_one_to_one_unwraps_envelope_to_flat_conversation() {
        // The Wire response wraps the conversation in
        // `{ conversation, public_keys }`. Kupferdraht's HTTP layer
        // unwraps before handing back to the caller.
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET)
                .path("/v14/one2one-conversations/wire.com/peer-uuid");
            then.status(200)
                .header("content-type", "application/json")
                .body(one2one_envelope(
                    "dm-uuid",
                    "wire.com",
                    "self-uuid",
                    "peer-uuid",
                    "wire.com",
                ));
        });
        let conv = http(&server.base_url())
            .fetch_one_to_one_conversation(14, "tok", "wire.com", "peer-uuid")
            .await
            .expect("fetch must succeed");
        // Caller gets `OwnConversation` directly, not the envelope.
        assert_eq!(conv.qualified_id.to_string(), "dm-uuid@wire.com");
    }

    #[tokio::test]
    async fn fetch_one_to_one_surfaces_5xx_as_backend_rejected() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET)
                .path("/v14/one2one-conversations/wire.com/peer-uuid");
            then.status(500)
                .header("content-type", "application/json")
                .body(r#"{"code":500,"label":"server-error","message":"boom"}"#);
        });

        let err = http(&server.base_url())
            .fetch_one_to_one_conversation(14, "tok", "wire.com", "peer-uuid")
            .await
            .expect_err("5xx must propagate");
        assert!(
            matches!(err, KupferdrahtError::BackendRejected(_)),
            "expected BackendRejected, got {err:?}"
        );
    }
}
