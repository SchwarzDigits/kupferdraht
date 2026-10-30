// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `CommitBundle` + `GroupInfo` HTTP surface — the two endpoints that
//! Step 3's external-commit join flow drives:
//!
//! * `GET /v{N}/conversations/{cnv_domain}/{cnv}/groupinfo` — fetches
//!   the TLS-serialized `MlsMessage(GroupInfo)` that the local openmls
//!   runtime consumes as the input to an external commit
//!   (RFC 9420 §12.4.3).
//! * `POST /v{N}/mls/commit-bundles` — uploads the raw bundle bytes
//!   produced by `MlsGroup::join_by_external_commit` (commit + welcome
//!   + new `GroupInfo`, RFC 9420 §12.5).
//!
//! Both endpoints speak the `message/mls` MIME type — MLS does not use
//! JSON on the wire. The POST response itself IS JSON
//! (`MLSMessageSendingStatus`), but the request body is raw bytes.
//!
//! Auth on both calls is the rebound access token (`Bearer`). Wire's
//! nginx injects `Z-Client` from the access-token claim, so the caller
//! must have gone through `rebind_access_token` first (LEARNINGS §1.4).

use tracing::debug;

use crate::error::KupferdrahtError;

use super::{HttpClient, reject_from_response};

// Both methods below are exercised by the cycle-3a unit tests at the
// bottom of this module; the core-layer wiring (`Client::join_room`)
// lands in cycle 3c. Until then, `cargo build` sees them as unused —
// suppress only for non-test builds.
#[cfg_attr(not(test), allow(dead_code))]
impl HttpClient {
    /// `GET /v{api_version}/conversations/{room_domain}/{room_id}/groupinfo`
    /// — fetches the TLS-serialized `MlsMessage(GroupInfo)` for an
    /// existing MLS group.
    ///
    /// Returns the raw body bytes unaltered. Parsing into an openmls
    /// `MlsMessageIn` is a Kupferdraht-core concern, not a transport one.
    ///
    /// # Errors
    /// * `BackendRejected` on non-2xx statuses. Wire surfaces
    ///   `400 mls-not-enabled`, `404 mls-missing-group-info`, and
    ///   `404 no-conversation`; all are folded into `BackendRejected`
    ///   with the response body attached for post-hoc diagnosis.
    /// * `Network` for transport failures.
    pub async fn fetch_group_info(
        &self,
        api_version: u32,
        access_token: &str,
        room_id: &str,
        room_domain: &str,
    ) -> Result<Vec<u8>, KupferdrahtError> {
        debug_assert!(!access_token.is_empty(), "access_token must be non-empty");
        debug_assert!(!room_id.is_empty(), "room_id must be non-empty");
        debug_assert!(!room_domain.is_empty(), "room_domain must be non-empty");
        let path = format!("/v{api_version}/conversations/{room_domain}/{room_id}/groupinfo");
        let url = self.url(&path)?;
        debug!(%url, "GET groupinfo");
        let resp = self
            .client
            .get(url)
            .bearer_auth(access_token)
            .header("accept", "message/mls")
            .send()
            .await
            .map_err(|e| KupferdrahtError::Network(format!("GET {path}: {e}")))?;
        if !resp.status().is_success() {
            return Err(reject_from_response(resp, "fetch_group_info").await);
        }
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| KupferdrahtError::Network(format!("fetch_group_info body: {e}")))?;
        Ok(bytes.to_vec())
    }

    /// `POST /v{api_version}/mls/commit-bundles` — uploads a raw MLS
    /// commit bundle (commit + welcome + new `GroupInfo`, as produced
    /// by `MlsGroup::join_by_external_commit` or any subsequent commit
    /// path Kupferdraht exercises).
    ///
    /// The request body is TLS-serialized bytes, content-type
    /// `message/mls`. Wire's 201 response is `MLSMessageSendingStatus`
    /// — Kupferdraht does not consume it in Step 3 (the new group epoch is
    /// authoritative from local MLS state after merging the pending
    /// commit), so this method discards the body and returns `()`.
    ///
    /// # Errors
    /// * `BackendRejected` on non-2xx statuses. Wire surfaces a rich
    ///   taxonomy of 400 / 403 / 404 / 409 / 422 MLS error labels — all
    ///   are folded into `BackendRejected` with the response body
    ///   attached for post-hoc diagnosis.
    /// * `Network` for transport failures.
    pub async fn upload_commit_bundle(
        &self,
        api_version: u32,
        access_token: &str,
        bundle: &[u8],
    ) -> Result<(), KupferdrahtError> {
        debug_assert!(!access_token.is_empty(), "access_token must be non-empty");
        debug_assert!(!bundle.is_empty(), "bundle must be non-empty");
        let path = format!("/v{api_version}/mls/commit-bundles");
        let url = self.url(&path)?;
        debug!(%url, byte_len = bundle.len(), "POST commit-bundles");
        let req = self
            .client
            .post(url)
            .bearer_auth(access_token)
            .header("content-type", "message/mls")
            .body(bundle.to_vec());
        self.send_no_body(req, "upload_commit_bundle").await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::Method::{GET, POST};
    use httpmock::{MockServer, prelude::HttpMockRequest};

    fn http(base_url: &str) -> HttpClient {
        HttpClient::new(base_url, true).expect("HttpClient::new (permit_http=true)")
    }

    // --- fetch_group_info ---------------------------------------------

    /// `MlsMessage` wire prefix: version (0x0001) + wire-format
    /// (0x0004 = `mls_group_info`). The helper must pass the bytes
    /// through untouched — parsing is a core concern, not transport.
    const GROUP_INFO_BYTES: &[u8] = &[0x00u8, 0x01, 0x00, 0x04, 0xDE, 0xAD, 0xBE, 0xEF];

    #[tokio::test]
    async fn fetch_group_info_hits_qualified_path_and_returns_raw_bytes() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(GET)
                .path("/v14/conversations/example.test/room-uuid/groupinfo")
                .header("authorization", "Bearer tok")
                .header("accept", "message/mls");
            then.status(200)
                .header("content-type", "message/mls")
                .body(GROUP_INFO_BYTES);
        });

        let out = http(&server.base_url())
            .fetch_group_info(14, "tok", "room-uuid", "example.test")
            .await
            .expect("fetch_group_info must succeed");
        assert_eq!(out, GROUP_INFO_BYTES);
        mock.assert_calls(1);
    }

    #[tokio::test]
    async fn fetch_group_info_surfaces_missing_group_info() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET)
                .path("/v14/conversations/example.test/room-uuid/groupinfo");
            then.status(404)
                .header("content-type", "application/json")
                .body(r#"{"code":404,"label":"mls-missing-group-info","message":"no gi"}"#);
        });
        let err = http(&server.base_url())
            .fetch_group_info(14, "tok", "room-uuid", "example.test")
            .await
            .expect_err("404 must propagate");
        assert!(
            matches!(err, KupferdrahtError::BackendRejected(ref m) if m.contains("mls-missing-group-info")),
            "expected BackendRejected carrying the label, got {err:?}"
        );
    }

    #[tokio::test]
    async fn fetch_group_info_surfaces_mls_not_enabled() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET)
                .path("/v14/conversations/example.test/room-uuid/groupinfo");
            then.status(400)
                .header("content-type", "application/json")
                .body(r#"{"code":400,"label":"mls-not-enabled","message":"off"}"#);
        });
        let err = http(&server.base_url())
            .fetch_group_info(14, "tok", "room-uuid", "example.test")
            .await
            .expect_err("400 must propagate");
        assert!(matches!(err, KupferdrahtError::BackendRejected(_)));
    }

    // --- upload_commit_bundle -----------------------------------------

    /// Canonical fixture — kept at module scope so the body matcher
    /// stays a non-capturing `fn` pointer (httpmock's `matches` does
    /// not accept capturing closures).
    const COMMIT_BUNDLE: &[u8] = &[0xAAu8, 0xBB, 0xCC, 0xDD];

    fn body_equals_commit_bundle(req: &HttpMockRequest) -> bool {
        req.body_ref() == COMMIT_BUNDLE
    }

    #[tokio::test]
    async fn upload_commit_bundle_posts_message_mls_body_verbatim() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/v14/mls/commit-bundles")
                .header("authorization", "Bearer tok")
                .header("content-type", "message/mls")
                .is_true(body_equals_commit_bundle);
            then.status(201)
                .header("content-type", "application/json")
                .body(r#"{"events":[],"time":"2026-04-24T10:00:00.000Z"}"#);
        });
        http(&server.base_url())
            .upload_commit_bundle(14, "tok", COMMIT_BUNDLE)
            .await
            .expect("upload_commit_bundle must succeed on 201");
        mock.assert_calls(1);
    }

    #[tokio::test]
    async fn upload_commit_bundle_surfaces_protocol_error() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/v14/mls/commit-bundles");
            then.status(400)
                .header("content-type", "application/json")
                .body(r#"{"code":400,"label":"mls-protocol-error","message":"boom"}"#);
        });
        let err = http(&server.base_url())
            .upload_commit_bundle(14, "tok", &[0x01, 0x02, 0x03])
            .await
            .expect_err("400 must propagate");
        assert!(
            matches!(err, KupferdrahtError::BackendRejected(ref m) if m.contains("mls-protocol-error")),
            "expected BackendRejected carrying the label, got {err:?}"
        );
    }

    #[tokio::test]
    async fn upload_commit_bundle_surfaces_identity_mismatch() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/v14/mls/commit-bundles");
            then.status(403)
                .header("content-type", "application/json")
                .body(r#"{"code":403,"label":"mls-identity-mismatch","message":"nope"}"#);
        });
        let err = http(&server.base_url())
            .upload_commit_bundle(14, "tok", &[0x01])
            .await
            .expect_err("403 must propagate");
        assert!(matches!(err, KupferdrahtError::BackendRejected(_)));
    }
}
