// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Redeeming a guest link: `GET /v{N}/conversations/join` (the
//! conversation the link names, Step 122) + `POST /v{N}/conversations/join`
//! (the redeem, Step 8). Both are unqualified — a link reaches only the
//! own backend's conversations (`open.md` §74).
//!
//! The link's `key` and `code` are secrets: they travel in the lookup's
//! query, so neither the URL nor a transport error (which names it) is
//! logged or returned.

use tracing::debug;

use crate::error::KupferdrahtError;
use crate::wire::dto::{ConversationCoverView, JoinConversationByCode};

use super::{HttpClient, reject_from_response};

impl HttpClient {
    /// `GET /v{api_version}/conversations/join?key=…&code=…` — Step 122:
    /// the conversation a guest link names, asked before the redeem.
    ///
    /// # Errors
    /// * `BackendRejected` on non-2xx (`404 no-conversation-code` for a
    ///   code this backend does not know, `409 guest-links-disabled`, …).
    /// * `Network` for transport failures; `Encoding` for an unreadable
    ///   body.
    pub async fn get_conversation_by_code(
        &self,
        api_version: u32,
        access_token: &str,
        key: &str,
        code: &str,
    ) -> Result<ConversationCoverView, KupferdrahtError> {
        debug_assert!(!access_token.is_empty(), "access_token must be non-empty");
        debug_assert!(!key.is_empty(), "key must be non-empty");
        debug_assert!(!code.is_empty(), "code must be non-empty");
        let path = format!("/v{api_version}/conversations/join");
        let mut url = self.url(&path)?;
        url.query_pairs_mut()
            .append_pair("key", key)
            .append_pair("code", code);
        debug!(%path, "GET conversations/join");
        let resp = self
            .client
            .get(url)
            .bearer_auth(access_token)
            .send()
            .await
            .map_err(|e| KupferdrahtError::Network(format!("GET {path}: {}", e.without_url())))?;
        if !resp.status().is_success() {
            return Err(reject_from_response(resp, "get_conversation_by_code").await);
        }
        crate::answer_bound::read_json::<ConversationCoverView>(resp)
            .await
            .map_err(|e| e.into_error("conversations/join body", KupferdrahtError::Encoding))
    }

    /// `POST /v{api_version}/conversations/join` — Step 8 redeem.
    ///
    /// Joins the caller as a Wire-conversation member; the MLS
    /// external-commit on the resulting group is a separate
    /// operation handled by `Client::join_via_link` once
    /// `sync_metadata` has surfaced the new room. Returns `()` —
    /// the Event payload of a successful redeem only matters to
    /// the `SyncService` consumers, not the join flow itself.
    ///
    /// `password` is `None` for an open link or `Some("…")` for a
    /// password-protected one. `403 invalid-conversation-password`
    /// surfaces as `BackendRejected`.
    ///
    /// # Errors
    /// * `BackendRejected` on non-2xx.
    /// * `Network` for transport failures.
    pub async fn join_conversation_by_code(
        &self,
        api_version: u32,
        access_token: &str,
        key: &str,
        code: &str,
        password: Option<&str>,
    ) -> Result<(), KupferdrahtError> {
        debug_assert!(!access_token.is_empty(), "access_token must be non-empty");
        debug_assert!(!key.is_empty(), "key must be non-empty");
        debug_assert!(!code.is_empty(), "code must be non-empty");
        let path = format!("/v{api_version}/conversations/join");
        let url = self.url(&path)?;
        let body = JoinConversationByCode {
            key,
            code,
            password,
        };
        debug!(%url, "POST conversations/join");
        let req = self.client.post(url).bearer_auth(access_token).json(&body);
        self.send_no_body(req, "join_conversation_by_code").await
    }
}

/// The `key` and `code` of a guest link's URL — the `uri` the backend
/// issues for a conversation code (`…/conversation-join/?key=…&code=…`,
/// Step 163c). No URL, or one whose query lacks either, is `InvalidInput`;
/// the error names neither secret.
pub(crate) fn join_link_key_and_code(link: &str) -> Result<(String, String), KupferdrahtError> {
    let not_a_link = || KupferdrahtError::InvalidInput("join link: not a join link URL".into());
    let url = reqwest::Url::parse(link).map_err(|_| not_a_link())?;
    let value = |name: &str| {
        url.query_pairs()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.into_owned())
            .filter(|value| !value.is_empty())
    };
    match (value("key"), value("code")) {
        (Some(key), Some(code)) => Ok((key, code)),
        _ => Err(not_a_link()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINK: &str = "https://account.example.test/conversation-join/?key=k3y%2Bpart&code=c0de";

    #[test]
    fn a_join_link_names_its_key_and_code() {
        assert_eq!(
            join_link_key_and_code(LINK).expect("a join link"),
            ("k3y+part".to_owned(), "c0de".to_owned())
        );
    }

    #[test]
    fn a_url_without_both_parts_is_no_join_link() {
        for link in [
            "https://account.example.test/conversation-join/?key=k3y",
            "https://account.example.test/conversation-join/?code=c0de",
            "https://account.example.test/conversation-join/?key=&code=c0de",
            "k3y c0de",
            "",
        ] {
            let err = join_link_key_and_code(link).expect_err(link);
            assert!(
                matches!(err, KupferdrahtError::InvalidInput(_)),
                "{link}: {err:?}"
            );
            let shown = err.to_string();
            assert!(
                !shown.contains("k3y") && !shown.contains("c0de"),
                "the error names no secret: {shown}"
            );
        }
    }
}
