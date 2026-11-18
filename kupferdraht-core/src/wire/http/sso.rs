// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Wire SSO (spar) HTTP surface — Step 17β (interactive / SP-driven), its
//! start URL reshaped in Step 162.
//!
//! Kupferdraht builds the `GET /sso/initiate-login/{code}` URL and validates
//! the SSO code (`HEAD`); the SAML round-trip and `POST /sso/finalize-login`
//! happen on Wire's server. How the outcome comes back is `wire::sso`.
//! `/sso/*` is unversioned (spar), live-confirmed (`docs/observations.md`
//! §107).

use tracing::debug;

use crate::error::KupferdrahtError;
use crate::wire::dto::SsoSettings;
use crate::wire::sso::RedirectPair;

use super::{HttpClient, reject_from_response};

impl HttpClient {
    /// Build the interactive SSO initiate URL:
    /// `{base}/sso/initiate-login/{code}?success_redirect=…&error_redirect=…&label=…`,
    /// the redirects only when `redirects` names them.
    ///
    /// Pure URL construction — the consumer opens the returned URL in a
    /// browser. With `redirects` Wire sends the browser to one of them after
    /// the `IdP`; without, its page reports to the window that opened it.
    /// `label` is the session cookie's label (ADR-0025).
    ///
    /// # Errors
    /// * `InvalidInput` for an empty `code` or a URL that fails to join.
    pub fn sso_initiate_login_url(
        &self,
        code: &str,
        redirects: Option<&RedirectPair>,
        label: &str,
    ) -> Result<String, KupferdrahtError> {
        if code.is_empty() {
            return Err(KupferdrahtError::InvalidInput(
                "SSO code must be non-empty".into(),
            ));
        }
        debug_assert!(!label.is_empty(), "a cookie label is never empty");
        let code = Self::path_segment(code, "sso_initiate_login_url code")?;
        let mut url = self.url(&format!("/sso/initiate-login/{code}"))?;
        {
            let mut query = url.query_pairs_mut();
            if let Some(redirects) = redirects {
                query
                    .append_pair("success_redirect", &redirects.on_success)
                    .append_pair("error_redirect", &redirects.on_refusal);
            }
            query.append_pair("label", label);
        }
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
        let code = Self::path_segment(code, "sso_code_exists code")?;
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

    /// `GET /sso/settings` — the backend's **default** SSO code, if any.
    /// `Ok(Some(code))` when a default `IdP` is configured, `Ok(None)` when
    /// none is (live-confirmed `{"default_sso_code":null}`,
    /// `docs/observations.md` §107).
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
        let settings: SsoSettings = crate::answer_bound::read_json(resp)
            .await
            .map_err(|e| e.into_error("/sso/settings body", KupferdrahtError::Encoding))?;
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

    fn pair() -> RedirectPair {
        RedirectPair {
            on_success: "wire-app://sso?credential=$cookie".into(),
            on_refusal: "wire-app://sso?reason=$label".into(),
        }
    }

    fn query(url: &str) -> std::collections::HashMap<String, String> {
        url::Url::parse(url)
            .expect("valid URL")
            .query_pairs()
            .into_owned()
            .collect()
    }

    #[test]
    fn sso_initiate_url_builds_unversioned_path_with_code() {
        // Spar's `/sso/initiate-login/{code}` is unversioned
        // (live-confirmed, `docs/observations.md` §107).
        let c = http("https://wire.example");
        let url = c
            .sso_initiate_login_url("9c8a1f00-idp-code", Some(&pair()), "label-1")
            .expect("initiate url must build");
        let parsed = url::Url::parse(&url).expect("returned value must be a valid URL");
        assert_eq!(parsed.path(), "/sso/initiate-login/9c8a1f00-idp-code");
    }

    #[test]
    fn sso_initiate_url_carries_the_redirects_and_the_label_as_query() {
        let c = http("https://wire.example");
        let url = c
            .sso_initiate_login_url("code-1", Some(&pair()), "label-1")
            .expect("initiate url must build");
        let pairs = query(&url);
        assert_eq!(
            pairs.get("success_redirect").map(String::as_str),
            Some("wire-app://sso?credential=$cookie"),
            "success_redirect must round-trip url-decoded"
        );
        assert_eq!(
            pairs.get("error_redirect").map(String::as_str),
            Some("wire-app://sso?reason=$label"),
            "error_redirect must round-trip url-decoded"
        );
        assert_eq!(pairs.get("label").map(String::as_str), Some("label-1"));
    }

    #[test]
    fn sso_initiate_url_without_redirects_carries_only_the_label() {
        let c = http("https://wire.example");
        let url = c
            .sso_initiate_login_url("code-1", None, "label-1")
            .expect("initiate url must build");
        let pairs = query(&url);
        assert_eq!(pairs.len(), 1, "{url}");
        assert_eq!(pairs.get("label").map(String::as_str), Some("label-1"));
    }

    #[test]
    fn sso_initiate_url_rejects_empty_code() {
        let c = http("https://wire.example");
        let err = c
            .sso_initiate_login_url("", None, "label-1")
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
