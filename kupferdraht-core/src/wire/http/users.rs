// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! User-directory HTTP surface.
//!
//! `POST /v{N}/list-users` resolves `display_name` + `handle` for a
//! batch of qualified user IDs. Kupferdraht uses it during
//! `Client::sync_metadata` to backfill `RoomMember` fields so a
//! DM's counterpart can be rendered by name without a round-trip
//! on every UI paint.

use tracing::debug;

use crate::error::KupferdrahtError;
use crate::wire::dto::{
    ListUsersByHandlesRequest, ListUsersRequest, ListUsersResponse, QualifiedHandle, QualifiedId,
};

use super::HttpClient;

impl HttpClient {
    /// `POST /v{api_version}/list-users` keyed by qualified
    /// handles instead of qualified ids. Used by Step 6-II-g's
    /// `@<handle>` resolver. Returns the same `ListUsersResponse`
    /// shape as the id flavor; handles Wire couldn't resolve
    /// land in `failed`.
    ///
    /// # Errors
    /// * `BackendRejected` on non-2xx statuses.
    /// * `Network` for transport failures.
    /// * `Encoding` for malformed response bodies.
    pub async fn list_users_by_handles(
        &self,
        api_version: u32,
        access_token: &str,
        qualified_handles: &[QualifiedHandle],
    ) -> Result<ListUsersResponse, KupferdrahtError> {
        debug_assert!(!access_token.is_empty(), "access_token must be non-empty");
        debug_assert!(
            !qualified_handles.is_empty(),
            "qualified_handles must be non-empty — Wire rejects empty queries"
        );
        let path = format!("/v{api_version}/list-users");
        let url = self.url(&path)?;
        let body = ListUsersByHandlesRequest { qualified_handles };
        debug!(%url, count = qualified_handles.len(), "POST list-users (handles)");
        let req = self.client.post(url).bearer_auth(access_token).json(&body);
        self.send_and_decode::<ListUsersResponse>(req, "list_users_by_handles")
            .await
    }

    /// `POST /v{api_version}/list-users` — resolve display-name /
    /// handle for a set of qualified user IDs.
    ///
    /// The Wire endpoint accepts either `qualified_ids` or
    /// `qualified_handles` (but not both); Kupferdraht uses IDs here
    /// and falls back to `list_users_by_handles` for the @-mention
    /// resolver path.
    ///
    /// # Errors
    /// * `BackendRejected` on non-2xx statuses.
    /// * `Network` for transport failures.
    /// * `Encoding` for malformed response bodies.
    pub async fn list_users(
        &self,
        api_version: u32,
        access_token: &str,
        qualified_ids: &[QualifiedId],
    ) -> Result<ListUsersResponse, KupferdrahtError> {
        debug_assert!(!access_token.is_empty(), "access_token must be non-empty");
        debug_assert!(
            !qualified_ids.is_empty(),
            "qualified_ids must be non-empty — Wire rejects empty queries"
        );
        let path = format!("/v{api_version}/list-users");
        let url = self.url(&path)?;
        let body = ListUsersRequest { qualified_ids };
        debug!(%url, count = qualified_ids.len(), "POST list-users");
        let req = self.client.post(url).bearer_auth(access_token).json(&body);
        self.send_and_decode::<ListUsersResponse>(req, "list_users")
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::Method::POST;
    use httpmock::MockServer;

    fn http(base_url: &str) -> HttpClient {
        HttpClient::new(base_url, true).expect("HttpClient::new (permit_http=true)")
    }

    fn qid(id: &str, domain: &str) -> QualifiedId {
        QualifiedId {
            id: id.to_string(),
            domain: domain.to_string(),
        }
    }

    #[tokio::test]
    async fn list_users_sends_qualified_ids_and_parses_profiles() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/v14/list-users")
                .header("authorization", "Bearer tok")
                .is_true(|req| {
                    let bytes = req.body_ref();
                    let Ok(v): Result<serde_json::Value, _> = serde_json::from_slice(bytes) else {
                        return false;
                    };
                    // We always send `qualified_ids` — never `qualified_handles`.
                    v.get("qualified_ids")
                        .and_then(|x| x.as_array())
                        .is_some_and(|a| a.len() == 1)
                        && v.get("qualified_handles").is_none()
                });
            then.status(200)
                .header("content-type", "application/json")
                .body(
                    r#"{"found":[
                         {
                           "qualified_id":{"id":"bob","domain":"example.test"},
                           "name":"Bob Example",
                           "handle":"bobx"
                         }
                       ],
                       "failed":[]}"#,
                );
        });

        let ids = vec![qid("bob", "example.test")];
        let resp = http(&server.base_url())
            .list_users(14, "tok", &ids)
            .await
            .expect("list_users must succeed");
        assert_eq!(resp.found.len(), 1);
        assert_eq!(resp.found[0].name, "Bob Example");
        assert_eq!(resp.found[0].handle.as_deref(), Some("bobx"));
        mock.assert_calls(1);
    }

    #[tokio::test]
    async fn list_users_tolerates_missing_handle() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/v14/list-users");
            then.status(200)
                .header("content-type", "application/json")
                .body(
                    r#"{"found":[
                         {
                           "qualified_id":{"id":"carol","domain":"example.test"},
                           "name":"Carol"
                         }
                       ]}"#,
                );
        });
        let resp = http(&server.base_url())
            .list_users(14, "tok", &[qid("carol", "example.test")])
            .await
            .expect("no-handle case must succeed");
        assert_eq!(resp.found.len(), 1);
        assert!(resp.found[0].handle.is_none());
        assert!(resp.failed.is_empty());
    }

    #[tokio::test]
    async fn list_users_surfaces_server_error() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/v14/list-users");
            then.status(500)
                .header("content-type", "application/json")
                .body(r#"{"code":500,"label":"server-error","message":"boom"}"#);
        });

        let err = http(&server.base_url())
            .list_users(14, "tok", &[qid("bob", "example.test")])
            .await
            .expect_err("5xx must propagate");
        assert!(
            matches!(err, KupferdrahtError::BackendRejected(_)),
            "expected BackendRejected, got {err:?}"
        );
    }
}
