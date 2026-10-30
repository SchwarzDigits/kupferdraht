// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Conversation-list HTTP surface:
//! `POST /v{N}/conversations/list-ids` (paginated qualified IDs) +
//! `POST /v{N}/conversations/list` (bulk metadata).
//!
//! Both endpoints are Bearer-authenticated + client-scoped (the
//! access token must have been rebound via `/v{N}/access?client_id=`
//! first — the `Z-Client` claim is injected server-side).

use tracing::debug;

use crate::error::KupferdrahtError;
use crate::wire::dto::{
    ConversationAccessData, ConversationCodeInfo, ConversationIdsPage, ConversationIdsRequest,
    ConversationsResponse, CreateConversationCodeRequest, CreateConversationRequest,
    ListConversationsRequest, OwnConversation, QualifiedId,
};

use super::{HttpClient, reject_from_response};

impl HttpClient {
    /// `POST /v{api_version}/conversations/list-ids` — one page of
    /// qualified conversation IDs. `paging_state` is `None` on the
    /// first call; subsequent calls echo the value from the previous
    /// response.
    ///
    /// # Errors
    /// * `BackendRejected` on non-2xx statuses.
    /// * `Network` for transport failures.
    /// * `Encoding` for malformed response bodies.
    pub async fn list_conversation_ids(
        &self,
        api_version: u32,
        access_token: &str,
        paging_state: Option<&str>,
        size: Option<u32>,
    ) -> Result<ConversationIdsPage, KupferdrahtError> {
        debug_assert!(!access_token.is_empty(), "access_token must be non-empty");
        let path = format!("/v{api_version}/conversations/list-ids");
        let url = self.url(&path)?;
        let body = ConversationIdsRequest {
            paging_state: paging_state.map(str::to_string),
            size,
        };
        debug!(%url, has_paging_state = paging_state.is_some(), "POST conversations/list-ids");
        let req = self.client.post(url).bearer_auth(access_token).json(&body);
        self.send_and_decode::<ConversationIdsPage>(req, "list_conversation_ids")
            .await
    }

    /// Wire's schema cap on `POST /conversations/list`: ids per request.
    pub(crate) const LIST_CONVERSATIONS_MAX_IDS: usize = 1000;

    /// `POST /v{api_version}/conversations/list` — bulk metadata for
    /// up to [`Self::LIST_CONVERSATIONS_MAX_IDS`] qualified IDs at a
    /// time (Wire's schema cap; callers chunk). The
    /// response partitions the input into `found` / `not_found` /
    /// `failed` — see [`ConversationsResponse`] docs.
    ///
    /// # Errors
    /// * `BackendRejected` on non-2xx statuses.
    /// * `Network` for transport failures.
    /// * `Encoding` for malformed response bodies.
    pub async fn list_conversations(
        &self,
        api_version: u32,
        access_token: &str,
        qualified_ids: &[QualifiedId],
    ) -> Result<ConversationsResponse, KupferdrahtError> {
        debug_assert!(!access_token.is_empty(), "access_token must be non-empty");
        debug_assert!(
            !qualified_ids.is_empty(),
            "qualified_ids must be non-empty — Wire rejects empty lists"
        );
        debug_assert!(
            qualified_ids.len() <= Self::LIST_CONVERSATIONS_MAX_IDS,
            "Wire caps conversations/list at 1000 ids per request — callers chunk"
        );
        let path = format!("/v{api_version}/conversations/list");
        let url = self.url(&path)?;
        let body = ListConversationsRequest { qualified_ids };
        debug!(%url, count = qualified_ids.len(), "POST conversations/list");
        let req = self.client.post(url).bearer_auth(access_token).json(&body);
        self.send_and_decode::<ConversationsResponse>(req, "list_conversations")
            .await
    }

    /// `POST /v{api_version}/conversations` — register a new MLS
    /// conversation envelope on Wire. The 201 response body has the
    /// same shape as one entry of `conversations/list`'s `found`
    /// bucket — Kupferdraht parses it as [`OwnConversation`] and feeds
    /// it through the same Wire→Room adapter `sync_metadata` uses.
    ///
    /// Wire creates the conversation server-side (assigns a
    /// `qualified_id` UUID and an MLS `group_id`). The caller is
    /// responsible for then **building the local MLS group** with
    /// that `group_id` (`MlsGroup::new_with_group_id`) and — for
    /// non-empty `qualified_users` — uploading the initial commit
    /// bundle that adds the listed members.
    ///
    /// # Errors
    /// * `BackendRejected` on non-2xx statuses. Wire surfaces
    ///   `400 mls-not-enabled`, `403 channels-not-enabled`,
    ///   and the standard team-policy labels; all fold into
    ///   `BackendRejected` with the response body attached for
    ///   post-hoc diagnosis.
    /// * `Network` for transport failures.
    /// * `Encoding` for malformed response bodies.
    pub async fn create_conversation(
        &self,
        api_version: u32,
        access_token: &str,
        request: &CreateConversationRequest<'_>,
    ) -> Result<OwnConversation, KupferdrahtError> {
        debug_assert!(!access_token.is_empty(), "access_token must be non-empty");
        debug_assert!(
            !request.name.is_empty(),
            "conversation name must be non-empty"
        );
        let path = format!("/v{api_version}/conversations");
        let url = self.url(&path)?;
        debug!(
            %url,
            name = request.name,
            invitees = request.qualified_users.len(),
            "POST conversations"
        );
        let req = self
            .client
            .post(url)
            .bearer_auth(access_token)
            .json(request);
        self.send_and_decode::<OwnConversation>(req, "create_conversation")
            .await
    }

    /// `DELETE /v{api_version}/conversations/{cnv_domain}/{cnv}/members/{usr_domain}/{usr}` —
    /// remove a member from a conversation. Used by Step 6e-1 with
    /// `usr` set to the caller's own user id ("self-leave").
    ///
    /// **Wire's MLS leave flow is server-driven**: the leaving client
    /// does *not* build a self-Remove commit (openmls explicitly
    /// rejects that with `CreateCommitError::CannotRemoveSelf` —
    /// observations.md §31). Instead, the client tells the backend via this
    /// REST call, and Wire's external sender produces the MLS Remove
    /// commit on the leaver's behalf. The remaining members receive
    /// the commit via their `/v14/notifications` stream.
    ///
    /// Wire returns:
    /// * `200` — member removed (a `member-leave` event is emitted).
    /// * `204` — no change (member wasn't part of the conversation).
    /// * `403 invalid-op` — caller lacks the role to remove this user.
    /// * `404` — `cnv_domain`, `cnv`, `usr_domain`, or `usr` not found.
    ///
    /// `room_id` and `target_user_id` are both qualified
    /// `<uuid>@<domain>` strings; this function splits them before
    /// encoding into the URL.
    ///
    /// # Errors
    /// * `InvalidInput` if either id is unqualified.
    /// * `BackendRejected` on non-2xx status.
    /// * `Network` for transport failures.
    pub async fn delete_conversation_member(
        &self,
        api_version: u32,
        access_token: &str,
        room_id: &str,
        target_user_id: &str,
    ) -> Result<(), KupferdrahtError> {
        debug_assert!(!access_token.is_empty(), "access_token must be non-empty");
        let (cnv, cnv_domain) = room_id.split_once('@').ok_or_else(|| {
            KupferdrahtError::InvalidInput(format!(
                "delete_conversation_member: room_id `{room_id}` must be qualified <uuid>@<domain>"
            ))
        })?;
        let (usr, usr_domain) = target_user_id.split_once('@').ok_or_else(|| {
            KupferdrahtError::InvalidInput(format!(
                "delete_conversation_member: target_user_id `{target_user_id}` must be qualified \
                 <uuid>@<domain>"
            ))
        })?;
        let path =
            format!("/v{api_version}/conversations/{cnv_domain}/{cnv}/members/{usr_domain}/{usr}");
        let url = self.url(&path)?;
        debug!(%url, "DELETE conversations/{{d}}/{{id}}/members/{{d}}/{{u}}");
        let req = self.client.delete(url).bearer_auth(access_token);
        self.send_no_body(req, "delete_conversation_member").await
    }

    /// `DELETE /v{api_version}/teams/{tid}/conversations/{cid}` — delete
    /// a team conversation for **all** members (a team-admin action,
    /// distinct from leaving). `team_id` and `conv_local_id` are both
    /// the *unqualified* Wire UUIDs. Wire emits a `conversation.delete`
    /// event to the remaining members.
    ///
    /// Wire returns `200` on success and `403` when the caller lacks the
    /// role. A `404` (team or conversation unknown) is treated as
    /// **idempotent success** — deleting an already-gone conversation is
    /// a no-op, so a retry or a double-delete does not surface an error.
    ///
    /// # Errors
    /// * `BackendRejected` on non-2xx status other than `404`.
    /// * `Network` for transport failures.
    pub async fn delete_team_conversation(
        &self,
        api_version: u32,
        access_token: &str,
        team_id: &str,
        conv_local_id: &str,
    ) -> Result<(), KupferdrahtError> {
        debug_assert!(!access_token.is_empty(), "access_token must be non-empty");
        debug_assert!(!team_id.is_empty(), "team_id must be non-empty");
        debug_assert!(!conv_local_id.is_empty(), "conv_local_id must be non-empty");
        let path = format!("/v{api_version}/teams/{team_id}/conversations/{conv_local_id}");
        let url = self.url(&path)?;
        debug!(%url, "DELETE teams/{{tid}}/conversations/{{cid}}");
        let resp = self
            .client
            .delete(url)
            .bearer_auth(access_token)
            .send()
            .await
            .map_err(|e| KupferdrahtError::Network(format!("DELETE {path}: {e}")))?;
        if resp.status().as_u16() == 404 {
            return Ok(());
        }
        if !resp.status().is_success() {
            return Err(reject_from_response(resp, "delete_team_conversation").await);
        }
        Ok(())
    }

    /// `PUT /v{api_version}/conversations/{cnv_domain}/{cnv}/name` —
    /// update a conversation's display name. Wire emits a
    /// `conversation.rename` event to all members; receivers update
    /// their own local copy via the sync stream.
    ///
    /// `room_id` is the qualified `<uuid>@<domain>` form; this
    /// function splits it before encoding into the URL.
    ///
    /// Wire returns `204 No Content` on a real change and `200 OK`
    /// when the name was already the requested value (per swagger).
    /// Both are accepted.
    ///
    /// # Errors
    /// * `InvalidInput` if `room_id` doesn't contain `@`.
    /// * `BackendRejected` on non-2xx statuses (`403 invalid-op` on
    ///   insufficient role, `404 no-conversation` on missing room).
    /// * `Network` for transport failures.
    pub async fn rename_conversation(
        &self,
        api_version: u32,
        access_token: &str,
        room_id: &str,
        new_name: &str,
    ) -> Result<(), KupferdrahtError> {
        debug_assert!(!access_token.is_empty(), "access_token must be non-empty");
        debug_assert!(!new_name.is_empty(), "new_name must be non-empty");
        let (cnv, cnv_domain) = room_id.split_once('@').ok_or_else(|| {
            KupferdrahtError::InvalidInput(format!(
                "rename_conversation: room_id `{room_id}` must be qualified <uuid>@<domain>"
            ))
        })?;
        let path = format!("/v{api_version}/conversations/{cnv_domain}/{cnv}/name");
        let url = self.url(&path)?;
        let body = serde_json::json!({ "name": new_name });
        debug!(%url, "PUT conversations/{{domain}}/{{id}}/name");
        let req = self.client.put(url).bearer_auth(access_token).json(&body);
        self.send_no_body(req, "rename_conversation").await
    }

    /// `POST /v{api_version}/conversations/{cnv}/code` — Step 8 issue.
    ///
    /// Creates (or, if one exists, returns the existing) reusable
    /// guest link for the conversation. Wire's response shape
    /// branches on status:
    ///
    /// * `200 OK` — code already existed, body is
    ///   `ConversationCodeInfo`.
    /// * `201 Created` — code newly created, body is an `Event`
    ///   (member-update notification, not the link material). We
    ///   immediately follow up with `get_conversation_code` to
    ///   fetch the canonical envelope.
    ///
    /// `password` is `None` for an open link or `Some("…")` (≥ 8
    /// chars per Wire's schema) for a password-protected one.
    ///
    /// `room_id` must be the *unqualified* Wire conversation UUID:
    /// the route is `/v14/conversations/{uuid}/code`, not
    /// `/v14/conversations/{domain}/{uuid}/code`. Same shape as
    /// `delete_conversation_member`'s split, but only the local id
    /// is used.
    ///
    /// # Errors
    /// * `BackendRejected` on non-2xx (`403 access-denied` when the
    ///   caller isn't allowed to issue codes, `404 no-conversation`
    ///   on missing room).
    /// * `Network` for transport failures.
    /// * `Encoding` for malformed response bodies.
    pub async fn create_conversation_code(
        &self,
        api_version: u32,
        access_token: &str,
        room_local_id: &str,
        password: Option<&str>,
    ) -> Result<ConversationCodeInfo, KupferdrahtError> {
        debug_assert!(!access_token.is_empty(), "access_token must be non-empty");
        debug_assert!(!room_local_id.is_empty(), "room_local_id must be non-empty");
        let path = format!("/v{api_version}/conversations/{room_local_id}/code");
        let url = self.url(&path)?;
        let body = CreateConversationCodeRequest { password };
        debug!(%url, "POST conversations/{{cnv}}/code");
        let resp = self
            .client
            .post(url)
            .bearer_auth(access_token)
            .json(&body)
            .send()
            .await
            .map_err(|e| KupferdrahtError::Network(format!("POST {path}: {e}")))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(reject_from_response(resp, "create_conversation_code").await);
        }
        if status.as_u16() == 201 {
            // Newly created — Wire's body here is a member-update
            // Event, not the code envelope. Drain + discard, then
            // GET to retrieve the canonical info.
            let _ = resp.bytes().await;
            return self
                .get_conversation_code(api_version, access_token, room_local_id)
                .await?
                .ok_or_else(|| {
                    KupferdrahtError::Encoding(format!(
                        "create_conversation_code: POST {path} returned 201 but the \
                         follow-up GET reported no code on file"
                    ))
                });
        }
        resp.json::<ConversationCodeInfo>()
            .await
            .map_err(|e| KupferdrahtError::Encoding(format!("conversations/code body: {e}")))
    }

    /// `GET /v{api_version}/conversations/{cnv}/code` — Step 8 fetch.
    ///
    /// Returns the current code envelope or `Ok(None)` when Wire
    /// reports `404 no-conversation-code` (the conversation exists
    /// but has no link issued). All other non-2xx surface as
    /// `BackendRejected`.
    ///
    /// # Errors
    /// * `BackendRejected` on non-2xx (other than the no-code 404).
    /// * `Network` for transport failures.
    /// * `Encoding` for malformed response bodies.
    pub async fn get_conversation_code(
        &self,
        api_version: u32,
        access_token: &str,
        room_local_id: &str,
    ) -> Result<Option<ConversationCodeInfo>, KupferdrahtError> {
        debug_assert!(!access_token.is_empty(), "access_token must be non-empty");
        debug_assert!(!room_local_id.is_empty(), "room_local_id must be non-empty");
        let path = format!("/v{api_version}/conversations/{room_local_id}/code");
        let url = self.url(&path)?;
        debug!(%url, "GET conversations/{{cnv}}/code");
        let resp = self
            .client
            .get(url)
            .bearer_auth(access_token)
            .send()
            .await
            .map_err(|e| KupferdrahtError::Network(format!("GET {path}: {e}")))?;
        if resp.status().as_u16() == 404 {
            return Ok(None);
        }
        if !resp.status().is_success() {
            return Err(reject_from_response(resp, "get_conversation_code").await);
        }
        let info = resp
            .json::<ConversationCodeInfo>()
            .await
            .map_err(|e| KupferdrahtError::Encoding(format!("conversations/code body: {e}")))?;
        Ok(Some(info))
    }

    /// `DELETE /v{api_version}/conversations/{cnv}/code` — Step 8
    /// revoke. Idempotent on the Wire side: deleting a non-existent
    /// code returns `404 no-conversation-code`, which Kupferdraht maps
    /// to `Ok(())` so callers don't have to special-case it.
    ///
    /// # Errors
    /// * `BackendRejected` on non-2xx (other than the no-code 404).
    /// * `Network` for transport failures.
    pub async fn delete_conversation_code(
        &self,
        api_version: u32,
        access_token: &str,
        room_local_id: &str,
    ) -> Result<(), KupferdrahtError> {
        debug_assert!(!access_token.is_empty(), "access_token must be non-empty");
        debug_assert!(!room_local_id.is_empty(), "room_local_id must be non-empty");
        let path = format!("/v{api_version}/conversations/{room_local_id}/code");
        let url = self.url(&path)?;
        debug!(%url, "DELETE conversations/{{cnv}}/code");
        let resp = self
            .client
            .delete(url)
            .bearer_auth(access_token)
            .send()
            .await
            .map_err(|e| KupferdrahtError::Network(format!("DELETE {path}: {e}")))?;
        if resp.status().as_u16() == 404 {
            return Ok(());
        }
        if !resp.status().is_success() {
            return Err(reject_from_response(resp, "delete_conversation_code").await);
        }
        Ok(())
    }

    /// `PUT /v{api_version}/conversations/{cnv_domain}/{cnv}/access`
    /// — Step 8b. Replaces the conversation's `access` and
    /// `access_role` arrays. Kupferdraht ships this as the prerequisite
    /// for `POST /code` (the `create_conversation_code` route
    /// rejects with `403 access-denied` on conversations whose
    /// `access` array doesn't include `"code"`).
    ///
    /// `room_id` is the qualified `<uuid>@<domain>` form; we split
    /// internally so the URL gets the two parts Wire's route
    /// expects.
    ///
    /// # Errors
    /// * `InvalidInput` if `room_id` is unqualified or either list is empty.
    /// * `BackendRejected` on `403 access-denied` /
    ///   `403 invalid-op` / `403 action-denied` /
    ///   `404 no-conversation`.
    /// * `Network` for transport failures.
    pub async fn set_conversation_access(
        &self,
        api_version: u32,
        access_token: &str,
        room_id: &str,
        access: &[String],
        access_role: &[String],
    ) -> Result<(), KupferdrahtError> {
        debug_assert!(!access_token.is_empty(), "access_token must be non-empty");
        if access.is_empty() {
            return Err(KupferdrahtError::InvalidInput(
                "set_conversation_access: `access` must be non-empty".into(),
            ));
        }
        if access_role.is_empty() {
            return Err(KupferdrahtError::InvalidInput(
                "set_conversation_access: `access_role` must be non-empty".into(),
            ));
        }
        let (cnv, cnv_domain) = room_id.split_once('@').ok_or_else(|| {
            KupferdrahtError::InvalidInput(format!(
                "set_conversation_access: room_id `{room_id}` must be qualified <uuid>@<domain>"
            ))
        })?;
        let path = format!("/v{api_version}/conversations/{cnv_domain}/{cnv}/access");
        let url = self.url(&path)?;
        let body = ConversationAccessData {
            access,
            access_role,
        };
        debug!(%url, "PUT conversations/{{domain}}/{{cnv}}/access");
        let req = self.client.put(url).bearer_auth(access_token).json(&body);
        self.send_no_body(req, "set_conversation_access").await
    }

    /// `POST /v{api_version}/conversations/{cnv_domain}/{cnv}/typing` —
    /// emit a typing notification (`status` is `"started"` or
    /// `"stopped"`). Wire fans a `conversation.typing` event out to the
    /// other members and returns `200` with no body. Plaintext REST —
    /// typing is **not** an MLS-encrypted message. Step 21.
    ///
    /// `room_id` is the qualified `<uuid>@<domain>` form; this function
    /// splits it before encoding into the URL.
    ///
    /// # Errors
    /// * `InvalidInput` if `room_id` is unqualified.
    /// * `BackendRejected` on non-2xx (`404 no-conversation` on a
    ///   missing room).
    /// * `Network` for transport failures.
    pub async fn send_typing(
        &self,
        api_version: u32,
        access_token: &str,
        room_id: &str,
        status: &str,
    ) -> Result<(), KupferdrahtError> {
        debug_assert!(!access_token.is_empty(), "access_token must be non-empty");
        debug_assert!(
            status == "started" || status == "stopped",
            "typing status must be `started` or `stopped`"
        );
        let (cnv, cnv_domain) = room_id.split_once('@').ok_or_else(|| {
            KupferdrahtError::InvalidInput(format!(
                "send_typing: room_id `{room_id}` must be qualified <uuid>@<domain>"
            ))
        })?;
        let path = format!("/v{api_version}/conversations/{cnv_domain}/{cnv}/typing");
        let url = self.url(&path)?;
        let body = serde_json::json!({ "status": status });
        debug!(%url, status, "POST conversations/{{domain}}/{{id}}/typing");
        let req = self.client.post(url).bearer_auth(access_token).json(&body);
        self.send_no_body(req, "send_typing").await
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
    async fn list_conversation_ids_first_page_omits_paging_state() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/v14/conversations/list-ids")
                .header("authorization", "Bearer tok")
                .is_true(|req| {
                    let bytes = req.body_ref();
                    let Ok(v): Result<serde_json::Value, _> = serde_json::from_slice(bytes) else {
                        return false;
                    };
                    // First page: paging_state is absent (not `null`).
                    v.get("paging_state").is_none()
                });
            then.status(200)
                .header("content-type", "application/json")
                .body(
                    r#"{"qualified_conversations":[
                         {"id":"aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa","domain":"example.test"},
                         {"id":"bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb","domain":"example.test"}
                       ],
                       "has_more":false,
                       "paging_state":"PS=="}"#,
                );
        });

        let page = http(&server.base_url())
            .list_conversation_ids(14, "tok", None, None)
            .await
            .expect("first page must succeed");
        assert_eq!(page.qualified_conversations.len(), 2);
        assert!(!page.has_more);
        assert_eq!(page.paging_state, "PS==");
        mock.assert_calls(1);
    }

    #[tokio::test]
    async fn list_conversation_ids_follow_up_echoes_paging_state() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/v14/conversations/list-ids")
                .is_true(|req| {
                    let bytes = req.body_ref();
                    let Ok(v): Result<serde_json::Value, _> = serde_json::from_slice(bytes) else {
                        return false;
                    };
                    v.get("paging_state") == Some(&serde_json::Value::String("PS==".to_string()))
                        && v.get("size") == Some(&serde_json::Value::from(100))
                });
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"qualified_conversations":[], "has_more":false, "paging_state":"END"}"#);
        });

        let page = http(&server.base_url())
            .list_conversation_ids(14, "tok", Some("PS=="), Some(100))
            .await
            .expect("follow-up must succeed");
        assert_eq!(page.paging_state, "END");
        mock.assert_calls(1);
    }

    #[tokio::test]
    async fn list_conversations_partitions_found_not_found_failed() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST)
                .path("/v14/conversations/list")
                .header("authorization", "Bearer tok")
                .is_true(|req| {
                    let bytes = req.body_ref();
                    let Ok(v): Result<serde_json::Value, _> = serde_json::from_slice(bytes) else {
                        return false;
                    };
                    v.get("qualified_ids")
                        .and_then(|x| x.as_array())
                        .is_some_and(|a| a.len() == 1)
                });
            then.status(200)
                .header("content-type", "application/json")
                .body(
                    r#"{"found":[
                         {
                           "qualified_id":{"id":"aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa","domain":"example.test"},
                           "type":0,
                           "protocol":"mls",
                           "epoch":1,
                           "group_id":"Z3JwLWFh",
                           "name":"Engineering",
                           "members":{
                             "self":{"qualified_id":{"id":"user-uuid","domain":"example.test"}},
                             "others":[]
                           }
                         }
                       ],
                       "not_found":[
                         {"id":"cccccccc-cccc-cccc-cccc-cccccccccccc","domain":"example.test"}
                       ],
                       "failed":[
                         {"id":"dddddddd-dddd-dddd-dddd-dddddddddddd","domain":"other.test"}
                       ]}"#,
                );
        });

        let ids = vec![qid("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa", "example.test")];
        let resp = http(&server.base_url())
            .list_conversations(14, "tok", &ids)
            .await
            .expect("list_conversations must succeed");
        assert_eq!(resp.found.len(), 1);
        assert_eq!(resp.found[0].name.as_deref(), Some("Engineering"));
        assert_eq!(resp.found[0].members.others.len(), 0);
        assert_eq!(resp.not_found.len(), 1);
        assert_eq!(resp.failed.len(), 1);
    }

    #[tokio::test]
    async fn list_conversations_propagates_server_error() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/v14/conversations/list");
            then.status(500)
                .header("content-type", "application/json")
                .body(r#"{"code":500,"label":"server-error","message":"boom"}"#);
        });

        let ids = vec![qid("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa", "example.test")];
        let err = http(&server.base_url())
            .list_conversations(14, "tok", &ids)
            .await
            .expect_err("5xx must propagate");
        assert!(
            matches!(err, KupferdrahtError::BackendRejected(_)),
            "expected BackendRejected, got {err:?}"
        );
    }

    // --- rename_conversation -----------------------------------------

    #[tokio::test]
    async fn rename_conversation_splits_room_id_and_sends_name_body() {
        use httpmock::Method::PUT;
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(PUT)
                .path("/v14/conversations/wire.com/c0ffee03-0000-4000-8000-000000000003/name")
                .header("authorization", "Bearer tok")
                .is_true(|req| {
                    let bytes = req.body_ref();
                    let Ok(v): Result<serde_json::Value, _> = serde_json::from_slice(bytes) else {
                        return false;
                    };
                    v.get("name").and_then(|x| x.as_str()) == Some("Renamed via Kupferdraht")
                });
            then.status(204);
        });
        http(&server.base_url())
            .rename_conversation(
                14,
                "tok",
                "c0ffee03-0000-4000-8000-000000000003@wire.com",
                "Renamed via Kupferdraht",
            )
            .await
            .expect("rename must succeed");
        mock.assert_calls(1);
    }

    #[tokio::test]
    async fn rename_conversation_accepts_200_when_name_unchanged() {
        use httpmock::Method::PUT;
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(PUT)
                .path("/v14/conversations/wire.com/abc/name");
            then.status(200);
        });
        http(&server.base_url())
            .rename_conversation(14, "tok", "abc@wire.com", "Same Name")
            .await
            .expect("200 (name unchanged) must be treated as success");
    }

    // --- delete_conversation_member ----------------------------------

    #[tokio::test]
    async fn delete_conversation_member_self_leave_splits_both_ids() {
        use httpmock::Method::DELETE;
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(DELETE)
                .path("/v14/conversations/wire.com/c0ffee03-0000-4000-8000-000000000003/members/wire.com/a11ce000-0000-4000-8000-000000000001")
                .header("authorization", "Bearer tok");
            then.status(200);
        });
        http(&server.base_url())
            .delete_conversation_member(
                14,
                "tok",
                "c0ffee03-0000-4000-8000-000000000003@wire.com",
                "a11ce000-0000-4000-8000-000000000001@wire.com",
            )
            .await
            .expect("self-leave must succeed");
        mock.assert_calls(1);
    }

    #[tokio::test]
    async fn delete_conversation_member_accepts_204_no_change() {
        use httpmock::Method::DELETE;
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(DELETE)
                .path("/v14/conversations/wire.com/abc/members/wire.com/u");
            then.status(204);
        });
        http(&server.base_url())
            .delete_conversation_member(14, "tok", "abc@wire.com", "u@wire.com")
            .await
            .expect("204 (no change) must be treated as success");
    }

    #[tokio::test]
    async fn delete_conversation_member_surfaces_403_as_backend_rejected() {
        use httpmock::Method::DELETE;
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(DELETE)
                .path("/v14/conversations/wire.com/abc/members/wire.com/u");
            then.status(403)
                .header("content-type", "application/json")
                .body(r#"{"code":403,"label":"invalid-op","message":"x"}"#);
        });
        let err = http(&server.base_url())
            .delete_conversation_member(14, "tok", "abc@wire.com", "u@wire.com")
            .await
            .expect_err("403 must surface as BackendRejected");
        assert!(matches!(err, KupferdrahtError::BackendRejected(_)));
    }

    #[tokio::test]
    async fn delete_conversation_member_rejects_unqualified_room_id() {
        let server = MockServer::start();
        let err = http(&server.base_url())
            .delete_conversation_member(14, "tok", "no-at-sign", "u@wire.com")
            .await
            .expect_err("unqualified room_id must be rejected client-side");
        assert!(matches!(err, KupferdrahtError::InvalidInput(_)));
    }

    #[tokio::test]
    async fn delete_conversation_member_rejects_unqualified_user_id() {
        let server = MockServer::start();
        let err = http(&server.base_url())
            .delete_conversation_member(14, "tok", "abc@wire.com", "no-at-sign")
            .await
            .expect_err("unqualified user_id must be rejected client-side");
        assert!(matches!(err, KupferdrahtError::InvalidInput(_)));
    }

    #[tokio::test]
    async fn rename_conversation_rejects_unqualified_room_id() {
        let server = MockServer::start();
        let err = http(&server.base_url())
            .rename_conversation(14, "tok", "no-at-sign", "X")
            .await
            .expect_err("unqualified room_id must be rejected client-side");
        assert!(
            matches!(err, KupferdrahtError::InvalidInput(_)),
            "expected InvalidInput, got {err:?}"
        );
    }

    // --- send_typing -------------------------------------------------

    #[tokio::test]
    async fn send_typing_splits_room_id_and_sends_status_body() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/v14/conversations/wire.com/c0ffee03-0000-4000-8000-000000000003/typing")
                .header("authorization", "Bearer tok")
                .is_true(|req| {
                    let bytes = req.body_ref();
                    let Ok(v): Result<serde_json::Value, _> = serde_json::from_slice(bytes) else {
                        return false;
                    };
                    v.get("status").and_then(|x| x.as_str()) == Some("started")
                });
            then.status(200);
        });
        http(&server.base_url())
            .send_typing(
                14,
                "tok",
                "c0ffee03-0000-4000-8000-000000000003@wire.com",
                "started",
            )
            .await
            .expect("send_typing must succeed");
        mock.assert_calls(1);
    }

    #[tokio::test]
    async fn send_typing_sends_stopped_status_body() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/v14/conversations/wire.com/abc/typing")
                .is_true(|req| {
                    let bytes = req.body_ref();
                    let Ok(v): Result<serde_json::Value, _> = serde_json::from_slice(bytes) else {
                        return false;
                    };
                    v.get("status").and_then(|x| x.as_str()) == Some("stopped")
                });
            then.status(200);
        });
        http(&server.base_url())
            .send_typing(14, "tok", "abc@wire.com", "stopped")
            .await
            .expect("send_typing stopped must succeed");
        mock.assert_calls(1);
    }

    #[tokio::test]
    async fn send_typing_rejects_unqualified_room_id() {
        let server = MockServer::start();
        let err = http(&server.base_url())
            .send_typing(14, "tok", "no-at-sign", "started")
            .await
            .expect_err("unqualified room_id must be rejected client-side");
        assert!(matches!(err, KupferdrahtError::InvalidInput(_)));
    }

    // --- delete_team_conversation ------------------------------------

    #[tokio::test]
    async fn delete_team_conversation_hits_team_scoped_path() {
        use httpmock::Method::DELETE;
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(DELETE)
                .path("/v14/teams/team-uuid/conversations/conv-uuid")
                .header("authorization", "Bearer tok");
            then.status(200);
        });
        http(&server.base_url())
            .delete_team_conversation(14, "tok", "team-uuid", "conv-uuid")
            .await
            .expect("delete must succeed");
        mock.assert_calls(1);
    }

    #[tokio::test]
    async fn delete_team_conversation_surfaces_403_as_backend_rejected() {
        use httpmock::Method::DELETE;
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(DELETE).path("/v14/teams/t/conversations/c");
            then.status(403)
                .header("content-type", "application/json")
                .body(r#"{"code":403,"label":"operation-denied","message":"x"}"#);
        });
        let err = http(&server.base_url())
            .delete_team_conversation(14, "tok", "t", "c")
            .await
            .expect_err("403 must surface as BackendRejected");
        assert!(matches!(err, KupferdrahtError::BackendRejected(_)));
    }

    #[tokio::test]
    async fn delete_team_conversation_treats_404_as_idempotent_success() {
        use httpmock::Method::DELETE;
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(DELETE).path("/v14/teams/t/conversations/c");
            then.status(404)
                .header("content-type", "application/json")
                .body(r#"{"code":404,"label":"no-conversation","message":"x"}"#);
        });
        http(&server.base_url())
            .delete_team_conversation(14, "tok", "t", "c")
            .await
            .expect("404 (already gone) must be treated as idempotent success");
    }
}
