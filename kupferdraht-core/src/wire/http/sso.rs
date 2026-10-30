// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Wire SSO (spar) HTTP surface — Step 17β (interactive / SP-driven).
//!
//! Kupferdraht builds the `GET /sso/initiate-login/{code}` redirect URL and
//! validates the SSO code (`HEAD`); the SAML round-trip and
//! `POST /sso/finalize-login` happen on Wire's server. The consumer opens
//! the initiate URL in a browser and later hands the returned `zuid` cookie
//! to the finalize path (17b). See `docs/plan/17-saml-sso.md`.
//!
//! **Open question (live-probe):** `/sso/*` is assumed **unversioned** here
//! (spar service), not `/v14/sso/*` — see plan-doc Open Question #1.

use tracing::debug;

use crate::error::KupferdrahtError;
use crate::wire::dto::SsoSettings;

use super::{HttpClient, reject_from_response};

impl HttpClient {
    /// Build the interactive SSO initiate URL:
    /// `{base}/sso/initiate-login/{code}?success_redirect=…&error_redirect=…`.
    ///
    /// Pure URL construction — the consumer opens the returned URL in a
    /// browser; after the `IdP` round-trip Wire redirects to `success_redirect`
    /// (carrying the `zuid` cookie) or to `error_redirect`.
    ///
    /// # Errors
    /// * `InvalidInput` for an empty `code` or a URL that fails to join.
    pub fn sso_initiate_login_url(
        &self,
        code: &str,
        success_redirect: &str,
        error_redirect: &str,
    ) -> Result<String, KupferdrahtError> {
        if code.is_empty() {
            return Err(KupferdrahtError::InvalidInput(
                "SSO code must be non-empty".into(),
            ));
        }
        let mut url = self.url(&format!("/sso/initiate-login/{code}"))?;
        url.query_pairs_mut()
            .append_pair("success_redirect", success_redirect)
            .append_pair("error_redirect", error_redirect);
        Ok(url.into())
    }

    /// `HEAD /sso/initiate-login/{code}` — does this SSO code resolve to a
    /// configured `IdP`? `Ok(true)` for 200, `Ok(false)` for 404.
    ///
    /// # Errors
    /// * `BackendRejected` for a non-2xx / non-404 status.
    /// * `Network` for transport failures.
    pub async fn sso_code_exists(&self, code: &str) -> Result<bool, KupferdrahtError> {
        debug_assert!(!code.is_empty(), "SSO code must be non-empty");
        let url = self.url(&format!("/sso/initiate-login/{code}"))?;
        debug!(%url, "HEAD sso/initiate-login");
        let resp = self
            .client
            .head(url)
            .send()
            .await
            .map_err(|e| KupferdrahtError::Network(format!("HEAD /sso/initiate-login: {e}")))?;
        let status = resp.status();
        if status.is_success() {
            Ok(true)
        } else if status == reqwest::StatusCode::NOT_FOUND {
            Ok(false)
        } else {
            Err(reject_from_response(resp, "sso_code_exists").await)
        }
    }

    /// `GET /sso/settings` — the backend's **default** SSO code, if any
    /// (Step 17β email discovery). `Ok(Some(code))` when a default `IdP` is
    /// configured, `Ok(None)` when none is. Wire v14 exposes no per-email-
    /// domain discovery, so this team default is all `ssoLogin(email)` can
    /// resolve; multi-`IdP` backends must pass an explicit code.
    ///
    /// # Errors
    /// * `BackendRejected` for a non-2xx status.
    /// * `Network` for transport failures; `Encoding` for a malformed body.
    pub async fn sso_default_code(&self) -> Result<Option<String>, KupferdrahtError> {
        let url = self.url("/sso/settings")?;
        debug!(%url, "GET sso/settings");
        let resp = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| KupferdrahtError::Network(format!("GET /sso/settings: {e}")))?;
        if !resp.status().is_success() {
            return Err(reject_from_response(resp, "sso_default_code").await);
        }
        let settings: SsoSettings = resp
            .json()
            .await
            .map_err(|e| KupferdrahtError::Encoding(format!("/sso/settings body: {e}")))?;
        Ok(settings.default_sso_code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::wire::HttpClient;

    fn http(base_url: &str) -> HttpClient {
        HttpClient::new(base_url, true).expect("HttpClient::new (permit_http=true)")
    }

    // --- initiate URL construction (pure) ---------------------------

    #[test]
    fn sso_initiate_url_builds_unversioned_path_with_code() {
        // Spar's `/sso/initiate-login/{code}` is unversioned (Open Question
        // #1 — to confirm by live-probe); assert the exact path here.
        let c = http("https://wire.example");
        let url = c
            .sso_initiate_login_url(
                "9c8a1f00-idp-code",
                "kupferdraht://sso/done",
                "kupferdraht://sso/error",
            )
            .expect("initiate url must build");
        let parsed = url::Url::parse(&url).expect("returned value must be a valid URL");
        assert_eq!(parsed.path(), "/sso/initiate-login/9c8a1f00-idp-code");
    }

    #[test]
    fn sso_initiate_url_carries_success_and_error_redirects_as_query() {
        let c = http("https://wire.example");
        let url = c
            .sso_initiate_login_url(
                "code-1",
                "kupferdraht://sso/done",
                "kupferdraht://sso/error",
            )
            .expect("initiate url must build");
        let parsed = url::Url::parse(&url).expect("valid URL");
        let pairs: std::collections::HashMap<String, String> =
            parsed.query_pairs().into_owned().collect();
        assert_eq!(
            pairs.get("success_redirect").map(String::as_str),
            Some("kupferdraht://sso/done"),
            "success_redirect must round-trip url-decoded"
        );
        assert_eq!(
            pairs.get("error_redirect").map(String::as_str),
            Some("kupferdraht://sso/error"),
            "error_redirect must round-trip url-decoded"
        );
    }

    #[test]
    fn sso_initiate_url_rejects_empty_code() {
        let c = http("https://wire.example");
        let err = c
            .sso_initiate_login_url("", "kupferdraht://s", "kupferdraht://e")
            .expect_err("empty code must be rejected");
        assert!(
            matches!(err, KupferdrahtError::InvalidInput(_)),
            "got {err:?}"
        );
    }

    // --- HEAD code existence ----------------------------------------

    #[tokio::test]
    async fn sso_code_exists_true_on_head_200() {
        use httpmock::Method::HEAD;
        use httpmock::MockServer;
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(HEAD).path("/sso/initiate-login/valid-code");
            then.status(200);
        });
        let exists = http(&server.base_url())
            .sso_code_exists("valid-code")
            .await
            .expect("HEAD 200 must resolve to Ok(true)");
        assert!(exists);
        mock.assert_calls(1);
    }

    #[tokio::test]
    async fn sso_code_exists_false_on_head_404() {
        use httpmock::Method::HEAD;
        use httpmock::MockServer;
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(HEAD).path("/sso/initiate-login/bogus-code");
            then.status(404);
        });
        let exists = http(&server.base_url())
            .sso_code_exists("bogus-code")
            .await
            .expect("HEAD 404 must resolve to Ok(false)");
        assert!(!exists);
    }

    #[tokio::test]
    async fn sso_code_exists_surfaces_500_as_backend_rejected() {
        use httpmock::Method::HEAD;
        use httpmock::MockServer;
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(HEAD).path("/sso/initiate-login/err-code");
            then.status(500);
        });
        let err = http(&server.base_url())
            .sso_code_exists("err-code")
            .await
            .expect_err("5xx must surface as an error, not Ok");
        assert!(
            matches!(err, KupferdrahtError::BackendRejected(_)),
            "got {err:?}"
        );
    }

    // --- default-code discovery (GET /sso/settings) -----------------

    #[tokio::test]
    async fn sso_default_code_returns_configured_default() {
        use httpmock::Method::GET;
        use httpmock::MockServer;
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/sso/settings");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"default_sso_code":"9c8a1f00-1111-2222-3333-444455556666"}"#);
        });
        let code = http(&server.base_url())
            .sso_default_code()
            .await
            .expect("settings must decode");
        assert_eq!(
            code.as_deref(),
            Some("9c8a1f00-1111-2222-3333-444455556666")
        );
    }

    #[tokio::test]
    async fn sso_default_code_is_none_when_no_default_configured() {
        use httpmock::Method::GET;
        use httpmock::MockServer;
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/sso/settings");
            then.status(200)
                .header("content-type", "application/json")
                .body("{}");
        });
        let code = http(&server.base_url())
            .sso_default_code()
            .await
            .expect("empty settings must decode to None");
        assert!(code.is_none());
    }
}
