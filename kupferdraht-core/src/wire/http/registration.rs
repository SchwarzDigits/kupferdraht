// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Account-registration HTTP surface (Step 16):
//! `POST /register` (unversioned).
//!
//! Step 16a covered the personal-flow `register_user`. Step 16b adds
//! the activation path: `POST /activate` to redeem the email-delivered
//! code, plus `POST /activate/send` to re-issue it. The builder /
//! `PendingClient` shape lands in 16c; the CLI in 16d.

use tracing::debug;

use crate::error::KupferdrahtError;
use crate::wire::dto::{
    ActivateRequest, ActivationResponse, NewUserRequest, RegisterResponse,
    SendActivationCodeRequest,
};

use super::HttpClient;

/// `/register` requires an `X-Forwarded-For` header (swagger marks it
/// required for the route). Wire's edge reverse-proxy normally
/// overwrites this with the connection's source IP, but the request
/// still has to carry *some* value or the servant route returns 400.
const PLACEHOLDER_FORWARDED_FOR: &str = "127.0.0.1";

impl HttpClient {
    /// `POST /register` (unversioned, like `/login` and
    /// `/access/logout`).
    ///
    /// Creates a fresh Wire account. The caller must follow up with
    /// [`HttpClient::activate`] (Step 16b) before the account can
    /// `login`.
    ///
    /// **Required header:** Wire's swagger marks `X-Forwarded-For` as
    /// required on this route; without it the servant returns 400.
    /// `register_user` injects a placeholder that Wire's edge replaces
    /// with the connection's source IP.
    ///
    /// `sso_id` is plumbed verbatim into the body when supplied —
    /// Step 16 does not implement SAML, only the field-level handoff
    /// to Step 17.
    ///
    /// # Errors
    /// * `BackendRejected` for every non-2xx (e.g. `409 key-exists`
    ///   for a duplicate email, `403 blacklisted-email`, `400
    ///   invalid-email`). The Wire label is preserved in the message
    ///   body for caller diagnosis.
    /// * `Network` for transport failures.
    /// * `Encoding` for malformed response bodies.
    pub async fn register_user(
        &self,
        body: NewUserRequest,
    ) -> Result<RegisterResponse, KupferdrahtError> {
        debug_assert!(
            !body.name.is_empty(),
            "NewUserRequest.name must be non-empty"
        );
        let url = self.url("/register")?;
        debug!(%url, "POST register");
        let req = self
            .client
            .post(url)
            .header("X-Forwarded-For", PLACEHOLDER_FORWARDED_FOR)
            .json(&body);
        self.send_and_decode::<RegisterResponse>(req, "register_user")
            .await
    }

    /// `POST /activate` (unversioned) — redeem the email-delivered
    /// activation code for a pending account.
    ///
    /// Wire's `Activate` schema requires `code + dryrun` plus at least
    /// one identity hint (`key`, `email`, or `phone`). Kupferdraht-CLI's
    /// flow always pairs the typed-in code with the registration email
    /// and hardcodes `dryrun: false` — Step 16 does not expose a
    /// "validate without consuming" surface.
    ///
    /// On success Wire returns an `ActivationResponse` carrying the
    /// account's email, a `first` flag distinguishing initial
    /// activation from later email-change confirmations, and an
    /// optional `sso_id` (Step 17 territory).
    ///
    /// # Errors
    /// * `BackendRejected` for non-2xx (e.g. `404 invalid-code` for a
    ///   wrong / expired code, `400 invalid-email`, `409 key-exists`).
    ///   The Wire label is preserved in the error message.
    /// * `Network` for transport failures.
    /// * `Encoding` for malformed response bodies.
    pub async fn activate(
        &self,
        email: &str,
        code: &str,
    ) -> Result<ActivationResponse, KupferdrahtError> {
        debug_assert!(!email.is_empty(), "email must be non-empty");
        debug_assert!(!code.is_empty(), "code must be non-empty");
        let url = self.url("/activate")?;
        debug!(%url, "POST activate");
        let body = ActivateRequest {
            code,
            dryrun: false,
            email: Some(email),
            key: None,
            phone: None,
        };
        let req = self.client.post(url).json(&body);
        self.send_and_decode::<ActivationResponse>(req, "activate")
            .await
    }

    /// `POST /activate/send` (unversioned) — request a fresh
    /// activation email for an account stuck in `pending-activation`.
    ///
    /// Useful when the original code expired or the email never
    /// arrived. Caller supplies the registration email; Wire's
    /// `SendActivationCode.locale` is left unset (Wire defaults to
    /// `en`).
    ///
    /// # Errors
    /// * `BackendRejected` for non-2xx (e.g. `400 invalid-email`,
    ///   `403 blacklisted-email`, `409 key-exists` if the account is
    ///   already active, `451 domain-blocked-for-registration` for
    ///   regionally blocked domains).
    /// * `Network` for transport failures.
    pub async fn request_activation_resend(&self, email: &str) -> Result<(), KupferdrahtError> {
        debug_assert!(!email.is_empty(), "email must be non-empty");
        let url = self.url("/activate/send")?;
        debug!(%url, "POST activate/send");
        let body = SendActivationCodeRequest {
            email,
            locale: None,
        };
        let req = self.client.post(url).json(&body);
        self.send_no_body(req, "request_activation_resend").await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::wire::HttpClient;
    use crate::wire::dto::{AccountStatus, UserSSOId};

    fn http(base_url: &str) -> HttpClient {
        HttpClient::new(base_url, true).expect("HttpClient::new (permit_http=true)")
    }

    fn personal_body(email: &str) -> NewUserRequest {
        NewUserRequest {
            name: "Alice".into(),
            email: Some(email.into()),
            password: Some("hunter2hunter2".into()),
            ..Default::default()
        }
    }

    // --- path + headers ---------------------------------------------

    #[tokio::test]
    async fn register_user_posts_to_unversioned_register_path() {
        use httpmock::Method::POST;
        use httpmock::MockServer;
        let server = MockServer::start();
        // Strict matcher: the path must be exactly `/register`,
        // not `/v14/register` (Wire treats /register as unversioned,
        // same family as /login and /access/logout).
        let mock = server.mock(|when, then| {
            when.method(POST).path("/register");
            then.status(201)
                .header("content-type", "application/json")
                .body(register_ok_body());
        });
        http(&server.base_url())
            .register_user(personal_body("alice@example.test"))
            .await
            .expect("register_user must succeed");
        mock.assert_calls(1);
    }

    #[tokio::test]
    async fn register_user_sends_x_forwarded_for_header() {
        use httpmock::Method::POST;
        use httpmock::MockServer;
        let server = MockServer::start();
        // Wire's swagger marks `X-Forwarded-For` as required on
        // /register; without it nginx returns 400 before the body
        // is parsed. We do not assert a specific value here — only
        // that *some* value is present.
        let mock = server.mock(|when, then| {
            when.method(POST).path("/register").is_true(|req| {
                req.headers()
                    .get("x-forwarded-for")
                    .is_some_and(|v| !v.is_empty())
            });
            then.status(201)
                .header("content-type", "application/json")
                .body(register_ok_body());
        });
        http(&server.base_url())
            .register_user(personal_body("alice@example.test"))
            .await
            .expect("register_user must succeed");
        mock.assert_calls(1);
    }

    // --- request body shape -----------------------------------------

    #[tokio::test]
    async fn register_user_sends_name_email_password_in_body() {
        use httpmock::Method::POST;
        use httpmock::MockServer;
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST).path("/register").is_true(|req| {
                let bytes = req.body_ref();
                let Ok(v): Result<serde_json::Value, _> = serde_json::from_slice(bytes) else {
                    return false;
                };
                v.get("name").and_then(|x| x.as_str()) == Some("Alice")
                    && v.get("email").and_then(|x| x.as_str()) == Some("alice@example.test")
                    && v.get("password").and_then(|x| x.as_str()) == Some("hunter2hunter2")
            });
            then.status(201)
                .header("content-type", "application/json")
                .body(register_ok_body());
        });
        http(&server.base_url())
            .register_user(personal_body("alice@example.test"))
            .await
            .expect("register_user must succeed");
        mock.assert_calls(1);
    }

    #[tokio::test]
    async fn register_user_omits_sso_id_when_none() {
        // Pure plumbing check: `sso_id: None` must produce a body
        // that does NOT carry an `sso_id` key (not even `null`).
        // Wire's NewUser schema treats `null` and absent
        // identically server-side, but `skip_serializing_if`
        // keeps the wire bytes minimal — live-validated requests
        // from working Wire clients omit the field entirely.
        use httpmock::Method::POST;
        use httpmock::MockServer;
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST).path("/register").is_true(|req| {
                let bytes = req.body_ref();
                let Ok(v): Result<serde_json::Value, _> = serde_json::from_slice(bytes) else {
                    return false;
                };
                v.as_object().is_some_and(|o| !o.contains_key("sso_id"))
            });
            then.status(201)
                .header("content-type", "application/json")
                .body(register_ok_body());
        });
        http(&server.base_url())
            .register_user(personal_body("alice@example.test"))
            .await
            .expect("register_user must succeed");
        mock.assert_calls(1);
    }

    #[tokio::test]
    async fn register_user_includes_sso_id_when_some() {
        use httpmock::Method::POST;
        use httpmock::MockServer;
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST).path("/register").is_true(|req| {
                let bytes = req.body_ref();
                let Ok(v): Result<serde_json::Value, _> = serde_json::from_slice(bytes) else {
                    return false;
                };
                let Some(sso) = v.get("sso_id") else {
                    return false;
                };
                sso.get("subject").and_then(|x| x.as_str()) == Some("samlSubjectXyz")
                    && sso.get("scim_external_id").and_then(|x| x.as_str()) == Some("scim-42")
                    && sso.get("tenant").and_then(|x| x.as_str()) == Some("acme")
            });
            then.status(201)
                .header("content-type", "application/json")
                .body(register_ok_body());
        });
        let body = NewUserRequest {
            name: "Alice".into(),
            email: Some("alice@example.test".into()),
            password: Some("hunter2hunter2".into()),
            sso_id: Some(UserSSOId {
                subject: Some("samlSubjectXyz".into()),
                scim_external_id: Some("scim-42".into()),
                tenant: Some("acme".into()),
            }),
            ..Default::default()
        };
        http(&server.base_url())
            .register_user(body)
            .await
            .expect("register_user must succeed");
        mock.assert_calls(1);
    }

    // --- response body parsing --------------------------------------

    #[tokio::test]
    async fn register_user_parses_qualified_id_status_and_name() {
        use httpmock::Method::POST;
        use httpmock::MockServer;
        let server = MockServer::start();
        // Wire's swagger `AccountStatus` enum is
        // `[active, suspended, deleted, ephemeral, pending-invitation]`.
        // The post-register status is to be confirmed by live-probe
        // (plan-doc Open Question #2); `ephemeral` is the most
        // plausible variant for a pending-activation account, so
        // we exercise the kebab→`PendingInvitation` mapping with
        // an explicit fixture instead.
        server.mock(|when, then| {
            when.method(POST).path("/register");
            then.status(201)
                .header("content-type", "application/json")
                .body(
                    r#"{
                      "qualified_id": {"id":"00000000-0000-0000-0000-000000000abc",
                                       "domain":"example.test"},
                      "name": "Alice",
                      "accent_id": 0,
                      "status": "pending-invitation",
                      "locale": "en",
                      "email": "alice@example.test"
                    }"#,
                );
        });
        let resp = http(&server.base_url())
            .register_user(personal_body("alice@example.test"))
            .await
            .expect("201 must parse");
        assert_eq!(resp.qualified_id.id, "00000000-0000-0000-0000-000000000abc");
        assert_eq!(resp.qualified_id.domain, "example.test");
        assert_eq!(resp.name, "Alice");
        assert_eq!(resp.status, AccountStatus::PendingInvitation);
        assert_eq!(resp.email.as_deref(), Some("alice@example.test"));
    }

    // --- error paths ------------------------------------------------

    #[tokio::test]
    async fn register_user_surfaces_409_key_exists_as_backend_rejected() {
        use httpmock::Method::POST;
        use httpmock::MockServer;
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/register");
            then.status(409)
                .header("content-type", "application/json")
                .body(r#"{"code":409,"label":"key-exists","message":"The given e-mail address is in use."}"#);
        });
        let err = http(&server.base_url())
            .register_user(personal_body("dup@example.test"))
            .await
            .expect_err("duplicate email must surface as BackendRejected");
        let KupferdrahtError::BackendRejected(msg) = err else {
            panic!("expected BackendRejected, got {err:?}");
        };
        assert!(
            msg.contains("key-exists"),
            "Wire label must be preserved in the error message; got {msg:?}"
        );
    }

    #[tokio::test]
    async fn register_user_surfaces_400_invalid_email_as_backend_rejected() {
        use httpmock::Method::POST;
        use httpmock::MockServer;
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/register");
            then.status(400)
                .header("content-type", "application/json")
                .body(
                    r#"{"code":400,"label":"invalid-email","message":"Invalid e-mail address."}"#,
                );
        });
        let err = http(&server.base_url())
            .register_user(personal_body("not-an-email"))
            .await
            .expect_err("malformed email must surface as BackendRejected");
        assert!(matches!(err, KupferdrahtError::BackendRejected(_)));
    }

    // ================================================================
    // Step 16b — activate + request_activation_resend
    // ================================================================

    // --- activate: path + body shape --------------------------------

    #[tokio::test]
    async fn activate_posts_to_unversioned_activate_path() {
        use httpmock::Method::POST;
        use httpmock::MockServer;
        let server = MockServer::start();
        // Path is exactly `/activate`, not `/v14/activate`.
        let mock = server.mock(|when, then| {
            when.method(POST).path("/activate");
            then.status(200)
                .header("content-type", "application/json")
                .body(activation_ok_body());
        });
        http(&server.base_url())
            .activate("alice@example.test", "abc123")
            .await
            .expect("activate must succeed");
        mock.assert_calls(1);
    }

    #[tokio::test]
    async fn activate_body_carries_code_email_and_dryrun_false() {
        // Wire's `Activate` schema requires `code + dryrun` and at
        // least one of key/email/phone. Kupferdraht always pairs `code`
        // with `email` and hardcodes `dryrun: false` (no "validate
        // without consuming" surface in Step 16).
        use httpmock::Method::POST;
        use httpmock::MockServer;
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST).path("/activate").is_true(|req| {
                let bytes = req.body_ref();
                let Ok(v): Result<serde_json::Value, _> = serde_json::from_slice(bytes) else {
                    return false;
                };
                v.get("code").and_then(|x| x.as_str()) == Some("abc123")
                    && v.get("email").and_then(|x| x.as_str()) == Some("alice@example.test")
                    && v.get("dryrun").and_then(serde_json::Value::as_bool) == Some(false)
                    // `key` + `phone` must be absent (not null) when not used.
                    && v.as_object().is_some_and(|o| !o.contains_key("key") && !o.contains_key("phone"))
            });
            then.status(200)
                .header("content-type", "application/json")
                .body(activation_ok_body());
        });
        http(&server.base_url())
            .activate("alice@example.test", "abc123")
            .await
            .expect("activate must succeed");
        mock.assert_calls(1);
    }

    // --- activate: response decoding --------------------------------

    #[tokio::test]
    async fn activate_parses_activation_response_email_first_and_sso_id() {
        use httpmock::Method::POST;
        use httpmock::MockServer;
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/activate");
            then.status(200)
                .header("content-type", "application/json")
                .body(
                    r#"{
                      "email": "alice@example.test",
                      "first": true,
                      "sso_id": {"subject":"samlSubjectXyz","tenant":"acme"}
                    }"#,
                );
        });
        let resp = http(&server.base_url())
            .activate("alice@example.test", "abc123")
            .await
            .expect("200 must parse");
        assert_eq!(resp.email.as_deref(), Some("alice@example.test"));
        assert_eq!(resp.first, Some(true));
        let sso = resp.sso_id.expect("sso_id must decode when present");
        assert_eq!(sso.subject.as_deref(), Some("samlSubjectXyz"));
        assert_eq!(sso.tenant.as_deref(), Some("acme"));
        assert!(
            sso.scim_external_id.is_none(),
            "missing scim_external_id must remain None, not error"
        );
    }

    #[tokio::test]
    async fn activate_parses_empty_activation_response_body() {
        // All fields on ActivationResponse are optional. Wire may
        // legitimately return an empty `{}` (e.g. for non-SSO
        // accounts on certain activation paths). The DTO must
        // tolerate that.
        use httpmock::Method::POST;
        use httpmock::MockServer;
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/activate");
            then.status(200)
                .header("content-type", "application/json")
                .body("{}");
        });
        let resp = http(&server.base_url())
            .activate("alice@example.test", "abc123")
            .await
            .expect("empty body must still decode");
        assert!(resp.email.is_none());
        assert!(resp.first.is_none());
        assert!(resp.sso_id.is_none());
    }

    // --- activate: error paths --------------------------------------

    #[tokio::test]
    async fn activate_surfaces_404_invalid_code_as_backend_rejected() {
        use httpmock::Method::POST;
        use httpmock::MockServer;
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/activate");
            then.status(404)
                .header("content-type", "application/json")
                .body(r#"{"code":404,"label":"invalid-code","message":"Invalid activation code"}"#);
        });
        let err = http(&server.base_url())
            .activate("alice@example.test", "wrong-code")
            .await
            .expect_err("wrong code must surface as BackendRejected");
        let KupferdrahtError::BackendRejected(msg) = err else {
            panic!("expected BackendRejected, got {err:?}");
        };
        assert!(
            msg.contains("invalid-code"),
            "Wire label must be preserved in the error message; got {msg:?}"
        );
    }

    #[tokio::test]
    async fn activate_surfaces_400_invalid_email_as_backend_rejected() {
        use httpmock::Method::POST;
        use httpmock::MockServer;
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/activate");
            then.status(400)
                .header("content-type", "application/json")
                .body(
                    r#"{"code":400,"label":"invalid-email","message":"Invalid e-mail address."}"#,
                );
        });
        let err = http(&server.base_url())
            .activate("not-an-email", "abc123")
            .await
            .expect_err("malformed email must surface as BackendRejected");
        assert!(matches!(err, KupferdrahtError::BackendRejected(_)));
    }

    // --- request_activation_resend: path + body shape ---------------

    #[tokio::test]
    async fn request_activation_resend_posts_to_activate_send_path() {
        use httpmock::Method::POST;
        use httpmock::MockServer;
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST).path("/activate/send");
            then.status(200);
        });
        http(&server.base_url())
            .request_activation_resend("alice@example.test")
            .await
            .expect("resend must succeed");
        mock.assert_calls(1);
    }

    #[tokio::test]
    async fn request_activation_resend_body_carries_email() {
        use httpmock::Method::POST;
        use httpmock::MockServer;
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST).path("/activate/send").is_true(|req| {
                let bytes = req.body_ref();
                let Ok(v): Result<serde_json::Value, _> = serde_json::from_slice(bytes) else {
                    return false;
                };
                v.get("email").and_then(|x| x.as_str()) == Some("alice@example.test")
                    // `locale` is optional and Kupferdraht leaves it unset.
                    && v.as_object().is_some_and(|o| !o.contains_key("locale"))
            });
            then.status(200);
        });
        http(&server.base_url())
            .request_activation_resend("alice@example.test")
            .await
            .expect("resend must succeed");
        mock.assert_calls(1);
    }

    // --- request_activation_resend: error paths ---------------------

    #[tokio::test]
    async fn request_activation_resend_surfaces_409_key_exists_as_backend_rejected() {
        // 409 key-exists from /activate/send means the account is
        // already active — re-issuing a code makes no sense, so Wire
        // refuses. Surface as BackendRejected with label preserved.
        use httpmock::Method::POST;
        use httpmock::MockServer;
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/activate/send");
            then.status(409)
                .header("content-type", "application/json")
                .body(
                    r#"{"code":409,"label":"key-exists","message":"The given e-mail address is in use."}"#,
                );
        });
        let err = http(&server.base_url())
            .request_activation_resend("already-active@example.test")
            .await
            .expect_err("already-active email must surface as BackendRejected");
        let KupferdrahtError::BackendRejected(msg) = err else {
            panic!("expected BackendRejected, got {err:?}");
        };
        assert!(
            msg.contains("key-exists"),
            "Wire label must be preserved in the error message; got {msg:?}"
        );
    }

    #[tokio::test]
    async fn request_activation_resend_surfaces_403_blacklisted_as_backend_rejected() {
        use httpmock::Method::POST;
        use httpmock::MockServer;
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/activate/send");
            then.status(403)
                .header("content-type", "application/json")
                .body(
                    r#"{"code":403,"label":"blacklisted-email","message":"The given e-mail address has been blacklisted."}"#,
                );
        });
        let err = http(&server.base_url())
            .request_activation_resend("bounced@example.test")
            .await
            .expect_err("blacklisted email must surface as BackendRejected");
        assert!(matches!(err, KupferdrahtError::BackendRejected(_)));
    }

    // --- fixtures ---------------------------------------------------

    /// Minimal `User`-schema body matching swagger's `required` set
    /// (`qualified_id`, `name`, `accent_id`, `status`, `locale`).
    fn register_ok_body() -> String {
        r#"{
          "qualified_id": {"id":"00000000-0000-0000-0000-000000000abc",
                           "domain":"example.test"},
          "name": "Alice",
          "accent_id": 0,
          "status": "ephemeral",
          "locale": "en"
        }"#
        .to_string()
    }

    /// Minimal `ActivationResponse` body — every field is optional
    /// per swagger; this fixture sets the most common shape (email
    /// echoed, `first: true` for initial activation, no SSO).
    fn activation_ok_body() -> String {
        r#"{
          "email": "alice@example.test",
          "first": true
        }"#
        .to_string()
    }
}
