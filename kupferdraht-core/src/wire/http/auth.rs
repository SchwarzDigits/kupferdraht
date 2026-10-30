// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Authentication HTTP surface: `POST /login`, `POST /access`
//! (cookie-auth rebind), `GET /self`.

use reqwest::StatusCode;
use tracing::debug;
// `warn` fires only in the native `zuid` Set-Cookie path (W6); on wasm the
// browser jar owns the cookie and that path is `cfg`-ed out.
#[cfg(not(target_arch = "wasm32"))]
use tracing::warn;

use crate::error::KupferdrahtError;
use crate::wire::dto::{LoginOutcome, LoginResponse, SelfUser};

use super::{HttpClient, WireErrorBody};

/// Outcome of [`HttpClient::rebind_access_token`].
///
/// Wire rotates the `zuid` refresh cookie on every successful
/// `/access` call (live-confirmed — sending the original
/// login zuid to a second `/access` returns
/// `403 invalid-credentials: "Invalid zauth token"`). The caller must
/// persist `refresh_token` from this struct, **not** the value it
/// passed in, otherwise the next process resume sends a stale cookie.
#[derive(Clone)]
pub struct RebindOutcome {
    /// Fresh access token bound to `client_id`.
    pub access_token: String,
    /// Latest `zuid` cookie value. Either freshly extracted from the
    /// `/access` response's `Set-Cookie` header (cookie was rotated)
    /// or the value the caller supplied (Wire did not rotate this
    /// time). Either way, this is what to persist.
    pub refresh_token: String,
}

impl std::fmt::Debug for RebindOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Step 101: secrets redacted (`crate::redact`).
        f.debug_struct("RebindOutcome")
            .field("access_token", &crate::redact::Hidden)
            .field("refresh_token", &crate::redact::Hidden)
            .finish()
    }
}

// Wire's login flow accepts the email/password body without a `label` field,
// but only sets the persistent `zuid` Set-Cookie when both `?persist=true` AND a
// non-empty `label` are supplied (else `POST /access` later returns 403 "Missing
// cookie and token", observations.md §16). The label is the **per-install** value
// threaded in from `ClientBuilder` (Step 41) so two devices of one account get
// distinct cookie slots.

// ── W6: browser cookie jar vs. explicit `zuid` cookie ───────────────────────
//
// Wire authenticates `/access` with the `zuid` refresh cookie and rotates it via
// `Set-Cookie`. Native reads/writes `zuid` itself (reqwest's own jar is off —
// observations.md §18 — so it forwards Wire's `Path=/access` cookie reliably).
// In a browser JS/wasm **cannot** read `Set-Cookie` or set the `Cookie` header,
// so the browser jar carries `zuid`: auth requests opt in with
// `fetch(credentials:'include')` and the explicit cookie handling is elided.

/// Opt a request into the browser cookie jar (wasm) / no-op (native).
#[cfg(target_arch = "wasm32")]
fn browser_credentials(rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    rb.fetch_credentials_include()
}
#[cfg(not(target_arch = "wasm32"))]
fn browser_credentials(rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    rb
}

/// Attach the `zuid` refresh cookie explicitly (native); on wasm the browser jar
/// carries it, so this is a no-op and `refresh_token` is unused.
#[cfg(not(target_arch = "wasm32"))]
fn with_zuid_cookie(rb: reqwest::RequestBuilder, refresh_token: &str) -> reqwest::RequestBuilder {
    rb.header(reqwest::header::COOKIE, format!("zuid={refresh_token}"))
}
#[cfg(target_arch = "wasm32")]
fn with_zuid_cookie(rb: reqwest::RequestBuilder, _refresh_token: &str) -> reqwest::RequestBuilder {
    rb
}

impl HttpClient {
    /// `POST /v{api_version}/login?persist=true`.
    ///
    /// # Errors
    /// * `InvalidCredentials` on HTTP 403 with label `invalid-credentials`.
    /// * `SecondFactorRequired` on HTTP 403 with label `code-authentication-required`.
    /// * `InvalidSecondFactor` on HTTP 403 with label `code-authentication-failed`.
    /// * `BackendRejected` for other non-2xx statuses.
    /// * `Network` for transport failures.
    pub async fn login(
        &self,
        api_version: u32,
        email: &str,
        password: &str,
        second_factor_code: Option<&str>,
        cookie_label: &str,
    ) -> Result<LoginOutcome, KupferdrahtError> {
        debug_assert!(!email.is_empty(), "email must be non-empty");
        debug_assert!(!password.is_empty(), "password must be non-empty");
        let path = format!("/v{api_version}/login?persist=true");
        let url = self.url(&path)?;
        let body = build_login_body(email, password, second_factor_code, cookie_label);
        debug!(%url, "POST login");
        let resp = browser_credentials(self.client.post(url).json(&body))
            .send()
            .await
            .map_err(|e| KupferdrahtError::Network(format!("POST /v{api_version}/login: {e}")))?;
        decode_login_response(resp).await
    }

    /// `POST /v{api_version}/access` — mint the **first** access token from
    /// the SSO `zuid` refresh cookie handed back by Wire's
    /// `/sso/finalize-login` redirect (Step 17β). Unlike
    /// [`rebind_access_token`](Self::rebind_access_token) there is **no**
    /// `client_id` — the device is not registered yet; this is the SSO
    /// analogue of [`login`](Self::login), yielding the same
    /// [`LoginOutcome`] (access token + rotated `zuid` + user) that the
    /// login pipeline consumes.
    ///
    /// # Errors
    /// * `SessionExpired` when the cookie is already rotated away / revoked
    ///   (403 `invalid-credentials`), via [`decode_login_response`].
    /// * `BackendRejected` for other non-2xx; `Network` for transport.
    pub async fn access_from_sso_cookie(
        &self,
        api_version: u32,
        sso_cookie: &str,
    ) -> Result<LoginOutcome, KupferdrahtError> {
        #[cfg(not(target_arch = "wasm32"))]
        debug_assert!(!sso_cookie.is_empty(), "sso cookie must be non-empty");
        let path = format!("/v{api_version}/access");
        let url = self.url(&path)?;
        debug!(%url, "POST access (SSO zuid cookie-auth, no client_id)");
        let resp = browser_credentials(with_zuid_cookie(self.client.post(url), sso_cookie))
            .send()
            .await
            .map_err(|e| {
                KupferdrahtError::Network(format!("POST /v{api_version}/access (sso): {e}"))
            })?;
        if !resp.status().is_success() {
            return Err(super::reject_from_response(resp, "access_from_sso_cookie").await);
        }
        // Unlike `/login`, Wire's `/access` rotates the `zuid` only sometimes and
        // otherwise omits the Set-Cookie — in which case the caller's cookie stays
        // valid, so keep it (mirrors `rebind_access_token`). W6: on wasm the
        // browser jar owns the cookie, so the persisted token is the empty sentinel.
        #[cfg(not(target_arch = "wasm32"))]
        let refresh_token = extract_zuid(resp.headers()).unwrap_or_else(|| sso_cookie.to_string());
        #[cfg(target_arch = "wasm32")]
        let refresh_token = String::new();
        let body = decode_token_body(resp, "/access (sso)").await?;
        Ok(LoginOutcome {
            access_token: body.access_token,
            user: body.user,
            refresh_token,
        })
    }

    /// `POST /v{api_version}/access` with the browser's login cookie alone —
    /// no `client_id`, which is optional: a user-scoped token for whoever
    /// the cookie belongs to. Step 111b: `resume` needs one before the
    /// account's databases open on the storage server, and the device id
    /// a rebind would name sits in them.
    ///
    /// # Errors
    /// * `SessionExpired` when the cookie is revoked or rotated away (403
    ///   `invalid-credentials`).
    /// * `RateLimited` on 429, `BackendRejected` for other non-2xx.
    /// * `Network` / `Encoding` from the call and its body.
    #[cfg(target_arch = "wasm32")]
    pub async fn access_with_browser_cookie(
        &self,
        api_version: u32,
    ) -> Result<LoginOutcome, KupferdrahtError> {
        let path = format!("/v{api_version}/access");
        let url = self.url(&path)?;
        debug!(%url, "POST access (browser cookie, no client_id)");
        let resp = browser_credentials(self.client.post(url))
            .send()
            .await
            .map_err(|e| KupferdrahtError::Network(format!("POST {path}: {e}")))?;
        if !resp.status().is_success() {
            return Err(rebind_rejection(resp).await);
        }
        let body = decode_token_body(resp, "/access (browser cookie)").await?;
        Ok(LoginOutcome {
            access_token: body.access_token,
            user: body.user,
            refresh_token: String::new(),
        })
    }

    /// `POST /v{api_version}/access?client_id=<client_id>` — rebind the
    /// access token to the freshly-registered device so the next
    /// `Z-Client`-scoped call succeeds.
    ///
    /// This is **mandatory** after `register_client`. Without the rebind
    /// Wire's nginx returns `400 "Header Z-Client is required"` on every
    /// subsequent client-scoped call (LEARNINGS §1.4).
    ///
    /// **Auth model:** `/access` is authenticated by the `zuid` refresh
    /// **cookie** only — never by a Bearer token. reqwest's cookie store
    /// does not reliably forward Wire's `Path=/access`-scoped cookie
    /// (observations.md §18), so the caller provides the extracted `zuid`
    /// value and we set the `Cookie:` header explicitly.
    ///
    /// # Errors
    /// * `KupferdrahtError::SessionExpired` when `refresh_token` is empty
    ///   (native) — corrupt/cleared persisted state; re-login required.
    /// * Transport and deserialization failures map onto `KupferdrahtError`.
    pub async fn rebind_access_token(
        &self,
        api_version: u32,
        refresh_token: &str,
        client_id: &str,
    ) -> Result<RebindOutcome, KupferdrahtError> {
        // An empty refresh token is reachable from persisted state (a
        // clearing Set-Cookie used to be captured as ""), so this is a
        // handleable error, not an assert — SessionExpired is the
        // documented re-login signal every consumer already handles.
        // wasm is exempt: there the empty string is the deliberate
        // sentinel (the browser jar owns the cookie — W6).
        #[cfg(not(target_arch = "wasm32"))]
        {
            if refresh_token.is_empty() {
                return Err(KupferdrahtError::SessionExpired);
            }
        }
        debug_assert!(!client_id.is_empty(), "client_id must be non-empty");
        let path = format!("/v{api_version}/access?client_id={client_id}");
        let url = self.url(&path)?;
        debug!(%url, "POST rebind access (zuid cookie-auth)");
        let resp = browser_credentials(with_zuid_cookie(self.client.post(url), refresh_token))
            .send()
            .await
            .map_err(|e| KupferdrahtError::Network(format!("POST /v{api_version}/access: {e}")))?;
        if !resp.status().is_success() {
            return Err(rebind_rejection(resp).await);
        }
        // Wire rotates the zuid on every successful /access. Capture
        // the new value before consuming the response body — once
        // `.json()` is called the headers are no longer accessible.
        // On wasm the browser jar rotates it transparently (Set-Cookie is
        // unreadable), so there is nothing to capture — W6.
        #[cfg(not(target_arch = "wasm32"))]
        let refresh_token = {
            let rotated = extract_zuid(resp.headers());
            debug!(
                rotated = rotated.is_some(),
                "/access response — zuid rotation analysis"
            );
            rotated.unwrap_or_else(|| refresh_token.to_string())
        };
        // wasm: the browser jar holds + rotates `zuid`; nothing to capture, the
        // (empty) sentinel is carried through — W6.
        #[cfg(target_arch = "wasm32")]
        let refresh_token = refresh_token.to_string();
        let body = decode_token_body(resp, "rebind response").await?;
        Ok(RebindOutcome {
            access_token: body.access_token,
            refresh_token,
        })
    }

    /// `POST /v{api_version}/access/logout` — revoke the `zuid` refresh
    /// cookie server-side. Subsequent `/access` calls with the same
    /// cookie return 403 (per Wire swagger). The access token itself
    /// is not invalidated synchronously — it expires naturally within
    /// its remaining ~15 min TTL — but no further refreshes are
    /// possible. matrix-rust-sdk equivalent: `Client::logout`.
    ///
    /// **Auth model**: same as `rebind_access_token` — Bearer +
    /// explicit `Cookie: zuid=<refresh_token>`. Wire's cookie store
    /// is path-scoped and reqwest does not reliably forward it
    /// (observations.md §18).
    ///
    /// # Errors
    /// * `BackendRejected` on non-2xx (e.g. 403 if the cookie is
    ///   already invalid).
    /// * `Network` for transport failures.
    pub async fn logout(
        &self,
        api_version: u32,
        access_token: &str,
        refresh_token: &str,
    ) -> Result<(), KupferdrahtError> {
        debug_assert!(!access_token.is_empty(), "access_token must be non-empty");
        #[cfg(not(target_arch = "wasm32"))]
        debug_assert!(!refresh_token.is_empty(), "refresh_token must be non-empty");
        let path = format!("/v{api_version}/access/logout");
        let url = self.url(&path)?;
        debug!(%url, "POST access/logout");
        let req = browser_credentials(with_zuid_cookie(
            self.client.post(url).bearer_auth(access_token),
            refresh_token,
        ));
        self.send_no_body(req, "logout").await
    }

    /// `PUT /v{api_version}/self` — patch the authenticated user's
    /// profile. Today Kupferdraht only sends `name` (display name); the
    /// other fields Wire's `UserUpdate` schema accepts (`accent_id`,
    /// `assets`, `picture`, `text_status`) are out-of-scope for the
    /// Step 2g surface.
    ///
    /// # Errors
    /// * `BackendRejected` on non-2xx statuses.
    /// * `Network` for transport failures.
    pub async fn update_self_profile_name(
        &self,
        api_version: u32,
        access_token: &str,
        display_name: &str,
    ) -> Result<(), KupferdrahtError> {
        debug_assert!(!access_token.is_empty(), "access_token must be non-empty");
        debug_assert!(!display_name.is_empty(), "display_name must be non-empty");
        let path = format!("/v{api_version}/self");
        let url = self.url(&path)?;
        let body = serde_json::json!({ "name": display_name });
        debug!(%url, "PUT self (name)");
        let req = self.client.put(url).bearer_auth(access_token).json(&body);
        self.send_no_body(req, "update_self_profile_name").await
    }

    /// `PUT /v{api_version}/self/handle` — change the authenticated
    /// user's handle. Wire enforces global handle uniqueness; a
    /// collision surfaces as `409 handle-exists`.
    ///
    /// # Errors
    /// * `BackendRejected` on non-2xx statuses (collision, malformed
    ///   handle, …).
    /// * `Network` for transport failures.
    pub async fn update_self_handle(
        &self,
        api_version: u32,
        access_token: &str,
        handle: &str,
    ) -> Result<(), KupferdrahtError> {
        debug_assert!(!access_token.is_empty(), "access_token must be non-empty");
        debug_assert!(!handle.is_empty(), "handle must be non-empty");
        let path = format!("/v{api_version}/self/handle");
        let url = self.url(&path)?;
        let body = serde_json::json!({ "handle": handle });
        debug!(%url, "PUT self/handle");
        let req = self.client.put(url).bearer_auth(access_token).json(&body);
        self.send_no_body(req, "update_self_handle").await
    }

    /// `PUT /v{api_version}/access/self/email` — request a change of the
    /// authenticated user's email. Wire treats this as a **verification
    /// flow**: it sends a confirmation mail to the new address and
    /// returns `202 Accepted`; the address only actually changes once
    /// the user clicks the link (`204` is also accepted). The caller's
    /// local profile is therefore *not* updated by this call.
    ///
    /// # Errors
    /// * `BackendRejected` on non-2xx — notably `400 invalid-email` and
    ///   `409 key-exists` (the address is already in use).
    /// * `Network` for transport failures.
    pub async fn update_self_email(
        &self,
        api_version: u32,
        access_token: &str,
        email: &str,
    ) -> Result<(), KupferdrahtError> {
        debug_assert!(!access_token.is_empty(), "access_token must be non-empty");
        debug_assert!(!email.is_empty(), "email must be non-empty");
        let path = format!("/v{api_version}/access/self/email");
        let url = self.url(&path)?;
        let body = serde_json::json!({ "email": email });
        debug!(%url, "PUT access/self/email");
        let req = self.client.put(url).bearer_auth(access_token).json(&body);
        self.send_no_body(req, "update_self_email").await
    }

    /// `PUT /v{api_version}/self/password` — change the authenticated
    /// user's password (Wire `change-password`). The body carries both
    /// `old_password` (re-auth) and `new_password`. Wire returns `200`
    /// on success and invalidates every device's refresh cookie, so all
    /// sessions must re-login afterwards (devices + MLS state survive).
    ///
    /// # Errors
    /// * `BackendRejected` on non-2xx — notably `403 invalid-credentials`
    ///   (wrong `old_password`) and `400` (password-policy violation).
    /// * `Network` for transport failures.
    pub async fn update_self_password(
        &self,
        api_version: u32,
        access_token: &str,
        old_password: &str,
        new_password: &str,
    ) -> Result<(), KupferdrahtError> {
        debug_assert!(!access_token.is_empty(), "access_token must be non-empty");
        debug_assert!(
            !old_password.is_empty() && !new_password.is_empty(),
            "passwords must be non-empty"
        );
        let path = format!("/v{api_version}/self/password");
        let url = self.url(&path)?;
        let body = serde_json::json!({
            "old_password": old_password,
            "new_password": new_password,
        });
        debug!(%url, "PUT self/password");
        let req = self.client.put(url).bearer_auth(access_token).json(&body);
        self.send_no_body(req, "update_self_password").await
    }

    /// `GET /v{api_version}/self`.
    ///
    /// Requires a valid access token; the token is sent as
    /// `Authorization: Bearer <token>`.
    ///
    /// # Errors
    /// Maps transport, auth, and deserialization failures onto `KupferdrahtError`.
    pub async fn fetch_self(
        &self,
        api_version: u32,
        access_token: &str,
    ) -> Result<SelfUser, KupferdrahtError> {
        debug_assert!(!access_token.is_empty(), "access_token must be non-empty");
        let path = format!("/v{api_version}/self");
        let url = self.url(&path)?;
        debug!(%url, "GET self");
        let req = self.client.get(url).bearer_auth(access_token);
        let self_user = self.send_and_decode::<SelfUser>(req, "GET /self").await?;
        // Step 95c: the qualified id becomes the account's user id.
        if crate::identity::UserId::from_parts(
            &self_user.qualified_id.id,
            &self_user.qualified_id.domain,
        )
        .is_err()
        {
            return Err(KupferdrahtError::Encoding(
                "GET /self: malformed qualified_id".into(),
            ));
        }
        Ok(self_user)
    }
}

/// Build the JSON body for `POST /v{N}/login`. Wire only sets the
/// persistent `zuid` cookie when `label` is present — `?persist=true`
/// alone is not enough (observations.md §16).
fn build_login_body(
    email: &str,
    password: &str,
    second_factor_code: Option<&str>,
    cookie_label: &str,
) -> serde_json::Value {
    serde_json::json!({
        "email": email,
        "password": password,
        "label": cookie_label,
        "verification_code": second_factor_code,
    })
}

/// Classify a non-2xx `POST /access`. A `429` is `RateLimited` — the
/// refresh runs inside every sync loop's authenticated calls, so the loops
/// wait it out like any other rate limit (Step 103). A 403 with label
/// `invalid-credentials` ("Invalid zauth token") means the zuid was already
/// rotated away / revoked — terminal, not refreshable: `SessionExpired`, so
/// the sync loop stops + signals a re-login instead of retrying forever
/// (Step 39; D3: narrow — only this label is terminal). Every other
/// non-2xx stays `BackendRejected`.
async fn rebind_rejection(resp: reqwest::Response) -> KupferdrahtError {
    let status = resp.status();
    if status == StatusCode::TOO_MANY_REQUESTS {
        return KupferdrahtError::RateLimited {
            retry_after_secs: super::retry_after_secs(resp.headers()),
        };
    }
    let body = resp.text().await.unwrap_or_default();
    if status == StatusCode::FORBIDDEN
        && serde_json::from_str::<WireErrorBody>(&body)
            .is_ok_and(|b| b.label == "invalid-credentials")
    {
        return KupferdrahtError::SessionExpired;
    }
    KupferdrahtError::BackendRejected(format!("rebind_access_token: {status} body={body}"))
}

/// The names of `Set-Cookie` headers (`name=value; …` → `name`) — what a
/// log or an error may show of them (Step 101).
#[cfg(not(target_arch = "wasm32"))]
fn cookie_names(set_cookies: &[String]) -> Vec<&str> {
    set_cookies
        .iter()
        .map(|c| c.split(['=', ';']).next().unwrap_or_default().trim())
        .collect()
}

/// Decode the response of `POST /v{N}/login` into a `LoginOutcome`.
/// Handles the 403 label mapping, non-2xx passthrough, and the `zuid`
/// extraction required because reqwest's cookie store does not reliably
/// forward Wire's `Path=/access`-scoped cookie (observations.md §18).
async fn decode_login_response(resp: reqwest::Response) -> Result<LoginOutcome, KupferdrahtError> {
    let status = resp.status();
    if status == StatusCode::FORBIDDEN {
        return Err(map_login_forbidden(resp).await);
    }
    if !status.is_success() {
        return Err(KupferdrahtError::BackendRejected(format!(
            "login: {status}"
        )));
    }
    // Native captures the rotated `zuid` from `Set-Cookie` to persist it itself;
    // on wasm the browser jar holds it (Set-Cookie is unreadable), so the
    // persisted refresh token is an empty sentinel — W6.
    #[cfg(not(target_arch = "wasm32"))]
    let refresh_token = {
        let set_cookies: Vec<String> = resp
            .headers()
            .get_all(reqwest::header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok().map(str::to_string))
            .collect();
        // Step 101: cookie names only — a cookie value (the `zuid` above
        // all) is a session on its own and never reaches a log or an error.
        let names = cookie_names(&set_cookies);
        debug!(?names, "login Set-Cookie names");
        extract_zuid(resp.headers()).ok_or_else(|| {
            warn!(?names, "no zuid in login response Set-Cookie");
            KupferdrahtError::BackendRejected(format!(
                "login response did not include a `zuid` Set-Cookie (cookies: {names:?})"
            ))
        })?
    };
    #[cfg(target_arch = "wasm32")]
    let refresh_token = String::new();
    let body = decode_token_body(resp, "login response").await?;
    Ok(LoginOutcome {
        access_token: body.access_token,
        user: body.user,
        refresh_token,
    })
}

/// Decode a `/login` or `/access` body. Step 95c: an empty
/// `access_token` or `user` is a malformed response — the token is the
/// bearer of every later request, `user` the account's user id.
async fn decode_token_body(
    resp: reqwest::Response,
    what: &str,
) -> Result<LoginResponse, KupferdrahtError> {
    let body: LoginResponse = resp
        .json()
        .await
        .map_err(|e| KupferdrahtError::Encoding(format!("{what} body: {e}")))?;
    if body.access_token.is_empty() || body.user.is_empty() || body.user.contains('@') {
        return Err(KupferdrahtError::Encoding(format!(
            "{what} body: empty access_token, or a user that is not a bare id"
        )));
    }
    Ok(body)
}

/// Extract the `zuid` value from one or more `Set-Cookie` headers.
///
/// Wire's login response sets the refresh cookie as
/// `zuid=<value>; HttpOnly; ...`. Returns `None` if no Set-Cookie
/// carries a `zuid` attribute.
///
/// Native-only: on wasm `Set-Cookie` is unreadable and the browser jar owns
/// the cookie, so no code path extracts it — W6.
#[cfg(not(target_arch = "wasm32"))]
fn extract_zuid(headers: &reqwest::header::HeaderMap) -> Option<String> {
    headers
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|s| s.split(';').map(str::trim))
        // A clearing cookie (`zuid=; Expires=1970…`) carries no token —
        // filter it so the caller's fallback keeps the old one instead
        // of clobbering it with "" (a consumer's bug report).
        .find_map(|p| {
            p.strip_prefix("zuid=")
                .filter(|z| !z.is_empty())
                .map(str::to_string)
        })
}

async fn map_login_forbidden(resp: reqwest::Response) -> KupferdrahtError {
    let label = resp
        .json::<WireErrorBody>()
        .await
        .map(|b| b.label)
        .unwrap_or_default();
    match label.as_str() {
        "invalid-credentials" => KupferdrahtError::InvalidCredentials,
        "code-authentication-required" => KupferdrahtError::SecondFactorRequired,
        "code-authentication-failed" => KupferdrahtError::InvalidSecondFactor,
        other => KupferdrahtError::BackendRejected(format!("login 403 label={other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn access_from_sso_cookie_mints_token_without_client_id() {
        use httpmock::Method::POST;
        use httpmock::MockServer;
        let server = MockServer::start();
        // /access seeded by the SSO `zuid` cookie, and crucially NO
        // `client_id` query (the device is not registered yet). Wire
        // rotates the `zuid` via Set-Cookie; the rotated value is what
        // `LoginOutcome.refresh_token` must carry.
        let mock = server.mock(|when, then| {
            when.method(POST).path("/v14/access").is_true(|req| {
                let carries_cookie = req
                    .headers()
                    .get_all("cookie")
                    .iter()
                    .any(|v| v.to_str().is_ok_and(|v| v.contains("zuid=sso-zuid-xyz")));
                let no_client_id = req.query_params().iter().all(|(k, _)| k != "client_id");
                carries_cookie && no_client_id
            });
            then.status(200)
                .header("content-type", "application/json")
                .header("set-cookie", "zuid=rotated-zuid; HttpOnly; Path=/access")
                .body(
                    r#"{"access_token":"at-123","token_type":"Bearer","expires_in":900,
                        "user":"00000000-0000-0000-0000-000000000abc"}"#,
                );
        });
        let http = HttpClient::new(&server.base_url(), true).expect("HttpClient::new");
        let outcome = http
            .access_from_sso_cookie(14, "sso-zuid-xyz")
            .await
            .expect("SSO cookie must mint an access token");
        assert_eq!(outcome.access_token, "at-123");
        assert_eq!(outcome.refresh_token, "rotated-zuid");
        assert_eq!(outcome.user, "00000000-0000-0000-0000-000000000abc");
        mock.assert_calls(1);
    }

    #[tokio::test]
    async fn access_from_sso_cookie_rejects_revoked_cookie() {
        // A revoked / already-rotated SSO cookie 403s; surfaced as
        // `BackendRejected` with the Wire label preserved.
        use httpmock::Method::POST;
        use httpmock::MockServer;
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/v14/access");
            then.status(403)
                .header("content-type", "application/json")
                .body(
                    r#"{"code":403,"label":"invalid-credentials","message":"Invalid zauth token"}"#,
                );
        });
        let http = HttpClient::new(&server.base_url(), true).expect("HttpClient::new");
        let err = http
            .access_from_sso_cookie(14, "stale-zuid")
            .await
            .expect_err("a revoked cookie must not mint a token");
        assert!(
            matches!(err, KupferdrahtError::BackendRejected(_)),
            "got {err:?}"
        );
    }

    #[tokio::test]
    async fn access_from_sso_cookie_keeps_input_cookie_when_not_rotated() {
        // Unlike `/login`, Wire's `/access` may return 200 **without** a `zuid`
        // Set-Cookie (no rotation). The caller's cookie stays valid, so the
        // outcome must carry it forward — not error on the missing header.
        use httpmock::Method::POST;
        use httpmock::MockServer;
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/v14/access");
            then.status(200)
                .header("content-type", "application/json")
                .body(
                    r#"{"access_token":"at-xyz","token_type":"Bearer","expires_in":900,"user":"00000000-0000-0000-0000-000000000abc"}"#,
                );
        });
        let http = HttpClient::new(&server.base_url(), true).expect("HttpClient::new");
        let outcome = http
            .access_from_sso_cookie(14, "still-valid-zuid")
            .await
            .expect("200 without Set-Cookie must still mint a token");
        assert_eq!(outcome.access_token, "at-xyz");
        assert_eq!(
            outcome.refresh_token, "still-valid-zuid",
            "no rotation → the input cookie is carried forward"
        );
    }

    #[test]
    fn build_login_body_carries_cookie_label() {
        let body = build_login_body("alice@example.test", "pw", None, "kupferdraht-install-xyz");
        assert_eq!(body["label"], "kupferdraht-install-xyz");
        assert_eq!(body["email"], "alice@example.test");
        assert_eq!(body["password"], "pw");
        assert!(
            body["verification_code"].is_null(),
            "verification_code must be JSON null when no 2FA code supplied"
        );
    }

    #[test]
    fn build_login_body_carries_second_factor_code_when_present() {
        let body = build_login_body(
            "alice@example.test",
            "pw",
            Some("123456"),
            "kupferdraht-install",
        );
        assert_eq!(body["verification_code"], "123456");
    }

    #[test]
    fn extract_zuid_parses_from_set_cookie() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::SET_COOKIE,
            "zuid=abc.def.ghi; HttpOnly; Path=/access".parse().unwrap(),
        );
        assert_eq!(extract_zuid(&headers), Some("abc.def.ghi".to_string()));
    }

    #[test]
    fn extract_zuid_returns_none_when_no_zuid_cookie() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::SET_COOKIE,
            "session=other-value; HttpOnly".parse().unwrap(),
        );
        assert_eq!(extract_zuid(&headers), None);
    }

    #[test]
    fn extract_zuid_finds_value_across_multiple_set_cookie_headers() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.append(
            reqwest::header::SET_COOKIE,
            "first=ignored; HttpOnly".parse().unwrap(),
        );
        headers.append(
            reqwest::header::SET_COOKIE,
            "zuid=real.refresh.token; HttpOnly; Path=/access"
                .parse()
                .unwrap(),
        );
        assert_eq!(
            extract_zuid(&headers),
            Some("real.refresh.token".to_string())
        );
    }

    /// Regression guard for the cookie-collision bug:
    /// the `HttpClient`'s reqwest builder must NOT enable
    /// `cookie_store(true)`. With the cookie store on, reqwest
    /// auto-attaches stored cookies in addition to the manual
    /// `Cookie` header we set on `/access` calls — Wire's nginx
    /// then receives both the rotated and the pre-rotation `zuid`
    /// in one request and picks the older invalidated one,
    /// returning `403 invalid-credentials` (live-confirmed against
    /// a Wire v14 backend). observations.md §18, §21 record the diagnosis.
    ///
    /// The check below issues two consecutive `/access`-style POSTs
    /// against an httpmock server: the first response sets a `zuid`
    /// cookie, the second strictly requires the second request to
    /// carry **only** the explicit cookie we set, with no append
    /// from any prior response. If this test ever turns red,
    /// somebody has re-enabled `cookie_store(true)` on the reqwest
    /// builder and the resume path will silently fail at runtime.
    #[tokio::test]
    async fn rebind_does_not_attach_store_held_cookies_from_earlier_responses() {
        use httpmock::Method::POST;
        use httpmock::MockServer;
        let server = MockServer::start();
        // First call — Wire-style: returns a Set-Cookie that a
        // jar-enabled client would auto-attach to subsequent
        // requests.
        server.mock(|when, then| {
            when.method(POST)
                .path("/v14/access")
                .query_param("client_id", "first");
            then.status(200)
                .header("content-type", "application/json")
                .header(
                    "set-cookie",
                    "zuid=stale-stored-value; Path=/access; HttpOnly",
                )
                .body(r#"{"access_token":"a1","token_type":"Bearer","expires_in":900,"user":"u"}"#);
        });
        // Second call — strict matcher: the only Cookie header
        // value Wire should see is `zuid=fresh-explicit-value`.
        // If the reqwest cookie store were enabled, reqwest would
        // ALSO send `zuid=stale-stored-value` from the first
        // response, the matcher would reject, and the mock would
        // 0-hit.
        let mock_strict = server.mock(|when, then| {
            when.method(POST)
                .path("/v14/access")
                .query_param("client_id", "second")
                .header("cookie", "zuid=fresh-explicit-value");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"access_token":"a2","token_type":"Bearer","expires_in":900,"user":"u"}"#);
        });
        let http = crate::wire::HttpClient::new(&server.base_url(), true)
            .expect("HttpClient (permit_http=true)");
        // First call seeds the would-be jar with `stale-stored-value`.
        let _ = http
            .rebind_access_token(14, "doesnt-matter", "first")
            .await
            .expect("first /access");
        // Second call — manually set zuid=fresh-explicit-value;
        // strict matcher rejects if the store also appends the
        // stale cookie.
        let _ = http
            .rebind_access_token(14, "fresh-explicit-value", "second")
            .await
            .expect("second /access — would 403 if cookie store re-attached the stale cookie");
        mock_strict.assert_calls(1);
    }

    #[tokio::test]
    async fn rebind_403_invalid_credentials_is_session_expired() {
        use httpmock::Method::POST;
        use httpmock::MockServer;
        // Step 39: a dead zuid (403 invalid-credentials, "Invalid zauth token")
        // is terminal — typed as SessionExpired so the sync loop stops + signals
        // re-login instead of retrying.
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/v14/access");
            then.status(403)
                .header("content-type", "application/json")
                .body(
                    r#"{"code":403,"label":"invalid-credentials","message":"Invalid zauth token"}"#,
                );
        });
        let err = http(&server.base_url())
            .rebind_access_token(14, "dead-zuid", "client")
            .await
            .expect_err("403 invalid-credentials must error");
        assert!(
            matches!(err, KupferdrahtError::SessionExpired),
            "expected SessionExpired, got {err:?}"
        );
    }

    #[tokio::test]
    async fn rebind_403_other_label_stays_backend_rejected() {
        use httpmock::Method::POST;
        use httpmock::MockServer;
        // D3 (narrow): only `invalid-credentials` is terminal; a different 403
        // label stays the opaque BackendRejected.
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/v14/access");
            then.status(403)
                .header("content-type", "application/json")
                .body(r#"{"code":403,"label":"missing-auth","message":"nope"}"#);
        });
        let err = http(&server.base_url())
            .rebind_access_token(14, "zuid", "client")
            .await
            .expect_err("403 must error");
        assert!(
            matches!(err, KupferdrahtError::BackendRejected(_)),
            "expected BackendRejected, got {err:?}"
        );
    }

    #[tokio::test]
    async fn rebind_429_is_rate_limited_with_its_retry_after() {
        use httpmock::Method::POST;
        use httpmock::MockServer;
        // Step 103 (review of #73): the refresh runs inside the sync loops'
        // authenticated calls, so its `429` must be the variant they wait out.
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/v14/access");
            then.status(429)
                .header("content-type", "application/json")
                .header("retry-after", "3")
                .body(r#"{"code":429,"label":"too-many-requests","message":"Please try again later."}"#);
        });
        let err = http(&server.base_url())
            .rebind_access_token(14, "zuid", "client")
            .await
            .expect_err("429 must error");
        assert!(
            matches!(
                err,
                KupferdrahtError::RateLimited {
                    retry_after_secs: Some(3)
                }
            ),
            "expected RateLimited, got {err:?}"
        );
    }

    // --- update_self_profile_name / update_self_handle ---------------

    fn http(base_url: &str) -> crate::wire::HttpClient {
        crate::wire::HttpClient::new(base_url, true).expect("HttpClient (permit_http=true)")
    }

    #[tokio::test]
    async fn update_self_profile_name_sends_only_name_field() {
        use httpmock::Method::PUT;
        use httpmock::MockServer;
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(PUT)
                .path("/v14/self")
                .header("authorization", "Bearer tok")
                .is_true(|req| {
                    let bytes = req.body_ref();
                    let Ok(v): Result<serde_json::Value, _> = serde_json::from_slice(bytes) else {
                        return false;
                    };
                    // Must contain `name` and nothing else from
                    // UserUpdate (no accent_id, picture, assets, …).
                    v.get("name").and_then(|x| x.as_str()) == Some("Alice (Kupferdraht)")
                        && v.get("accent_id").is_none()
                        && v.get("assets").is_none()
                        && v.get("picture").is_none()
                });
            then.status(200);
        });
        http(&server.base_url())
            .update_self_profile_name(14, "tok", "Alice (Kupferdraht)")
            .await
            .expect("update profile name must succeed");
        mock.assert_calls(1);
    }

    #[tokio::test]
    async fn update_self_handle_sends_handle_field() {
        use httpmock::Method::PUT;
        use httpmock::MockServer;
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(PUT).path("/v14/self/handle").is_true(|req| {
                let bytes = req.body_ref();
                let Ok(v): Result<serde_json::Value, _> = serde_json::from_slice(bytes) else {
                    return false;
                };
                v.get("handle").and_then(|x| x.as_str()) == Some("alice")
            });
            then.status(200);
        });
        http(&server.base_url())
            .update_self_handle(14, "tok", "alice")
            .await
            .expect("update handle must succeed");
        mock.assert_calls(1);
    }

    // --- logout ------------------------------------------------------

    #[tokio::test]
    async fn logout_posts_with_bearer_and_zuid_cookie() {
        use httpmock::Method::POST;
        use httpmock::MockServer;
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/v14/access/logout")
                .header("authorization", "Bearer tok")
                .header("cookie", "zuid=refresh-cookie-value");
            then.status(200);
        });
        http(&server.base_url())
            .logout(14, "tok", "refresh-cookie-value")
            .await
            .expect("logout must succeed");
        mock.assert_calls(1);
    }

    #[tokio::test]
    async fn logout_surfaces_403_invalid_credentials_as_backend_rejected() {
        use httpmock::Method::POST;
        use httpmock::MockServer;
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/v14/access/logout");
            then.status(403)
                .header("content-type", "application/json")
                .body(r#"{"code":403,"label":"invalid-credentials","message":"x"}"#);
        });
        let err = http(&server.base_url())
            .logout(14, "tok", "stale-refresh")
            .await
            .expect_err("403 must surface as BackendRejected");
        assert!(matches!(err, KupferdrahtError::BackendRejected(_)));
    }

    #[tokio::test]
    async fn update_self_handle_surfaces_409_collision_as_backend_rejected() {
        use httpmock::Method::PUT;
        use httpmock::MockServer;
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(PUT).path("/v14/self/handle");
            then.status(409)
                .header("content-type", "application/json")
                .body(r#"{"code":409,"label":"handle-exists","message":"taken"}"#);
        });
        let err = http(&server.base_url())
            .update_self_handle(14, "tok", "alreadytaken")
            .await
            .expect_err("collision must surface as BackendRejected");
        assert!(matches!(err, KupferdrahtError::BackendRejected(_)));
    }

    // --- update_self_email -------------------------------------------

    #[tokio::test]
    async fn update_self_email_sends_email_to_access_self_email() {
        use httpmock::Method::PUT;
        use httpmock::MockServer;
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(PUT)
                .path("/v14/access/self/email")
                .header("authorization", "Bearer tok")
                .is_true(|req| {
                    let bytes = req.body_ref();
                    let Ok(v): Result<serde_json::Value, _> = serde_json::from_slice(bytes) else {
                        return false;
                    };
                    v.get("email").and_then(|x| x.as_str()) == Some("alice@example.test")
                });
            // 202 Accepted — Wire has sent a verification mail; the
            // change is pending until the user confirms.
            then.status(202);
        });
        http(&server.base_url())
            .update_self_email(14, "tok", "alice@example.test")
            .await
            .expect("202 (verification mail sent) must be treated as success");
        mock.assert_calls(1);
    }

    #[tokio::test]
    async fn update_self_email_accepts_204() {
        use httpmock::Method::PUT;
        use httpmock::MockServer;
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(PUT).path("/v14/access/self/email");
            then.status(204);
        });
        http(&server.base_url())
            .update_self_email(14, "tok", "v@example.test")
            .await
            .expect("204 must be treated as success");
    }

    #[tokio::test]
    async fn update_self_email_surfaces_400_invalid_email_as_backend_rejected() {
        use httpmock::Method::PUT;
        use httpmock::MockServer;
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(PUT).path("/v14/access/self/email");
            then.status(400)
                .header("content-type", "application/json")
                .body(
                    r#"{"code":400,"label":"invalid-email","message":"Invalid e-mail address."}"#,
                );
        });
        let err = http(&server.base_url())
            .update_self_email(14, "tok", "not-an-email")
            .await
            .expect_err("400 invalid-email must surface as BackendRejected");
        assert!(matches!(err, KupferdrahtError::BackendRejected(_)));
    }

    #[tokio::test]
    async fn update_self_email_surfaces_409_key_exists_as_backend_rejected() {
        use httpmock::Method::PUT;
        use httpmock::MockServer;
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(PUT).path("/v14/access/self/email");
            then.status(409).header("content-type", "application/json").body(
                r#"{"code":409,"label":"key-exists","message":"The given e-mail address is in use."}"#,
            );
        });
        let err = http(&server.base_url())
            .update_self_email(14, "tok", "taken@example.test")
            .await
            .expect_err("409 key-exists must surface as BackendRejected");
        assert!(matches!(err, KupferdrahtError::BackendRejected(_)));
    }

    // --- update_self_password ----------------------------------------

    #[tokio::test]
    async fn update_self_password_puts_old_and_new_to_self_password() {
        use httpmock::Method::PUT;
        use httpmock::MockServer;
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(PUT)
                .path("/v14/self/password")
                .header("authorization", "Bearer tok")
                .is_true(|req| {
                    let bytes = req.body_ref();
                    let Ok(v): Result<serde_json::Value, _> = serde_json::from_slice(bytes) else {
                        return false;
                    };
                    v.get("old_password").and_then(|x| x.as_str()) == Some("oldpw")
                        && v.get("new_password").and_then(|x| x.as_str()) == Some("newpw1234")
                });
            then.status(200);
        });
        http(&server.base_url())
            .update_self_password(14, "tok", "oldpw", "newpw1234")
            .await
            .expect("200 must be treated as success");
        mock.assert_calls(1);
    }

    #[tokio::test]
    async fn update_self_password_surfaces_403_invalid_credentials_as_backend_rejected() {
        use httpmock::Method::PUT;
        use httpmock::MockServer;
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(PUT).path("/v14/self/password");
            then.status(403).header("content-type", "application/json").body(
                r#"{"code":403,"label":"invalid-credentials","message":"Authentication failed"}"#,
            );
        });
        let err = http(&server.base_url())
            .update_self_password(14, "tok", "wrongpw", "newpw1234")
            .await
            .expect_err(
                "403 invalid-credentials (wrong old password) must surface as BackendRejected",
            );
        assert!(matches!(err, KupferdrahtError::BackendRejected(_)));
    }

    #[tokio::test]
    async fn update_self_password_surfaces_400_policy_violation_as_backend_rejected() {
        use httpmock::Method::PUT;
        use httpmock::MockServer;
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(PUT).path("/v14/self/password");
            then.status(400)
                .header("content-type", "application/json")
                .body(r#"{"code":400,"label":"bad-request","message":"password too short"}"#);
        });
        let err = http(&server.base_url())
            .update_self_password(14, "tok", "oldpw", "short")
            .await
            .expect_err("400 policy violation must surface as BackendRejected");
        assert!(matches!(err, KupferdrahtError::BackendRejected(_)));
    }

    // ── zuid deletion-cookie hardening (a consumer's bug report):
    //    a clearing Set-Cookie (`zuid=`) must never
    //    overwrite a valid persisted refresh token, and an empty
    //    persisted token must surface as the recoverable
    //    SessionExpired — a consumer's runtime is not a test harness.

    #[test]
    fn extract_zuid_ignores_a_deletion_cookie() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::SET_COOKIE,
            "zuid=; Expires=Thu, 01 Jan 1970 00:00:00 GMT; Path=/access"
                .parse()
                .unwrap(),
        );
        assert_eq!(
            extract_zuid(&headers),
            None,
            "a clearing cookie carries no token — None lets the caller keep the old one"
        );
    }

    #[test]
    fn extract_zuid_skips_deletion_cookie_and_finds_the_real_one() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.append(
            reqwest::header::SET_COOKIE,
            "zuid=; Expires=Thu, 01 Jan 1970 00:00:00 GMT; Path=/access"
                .parse()
                .unwrap(),
        );
        headers.append(
            reqwest::header::SET_COOKIE,
            "zuid=fresh-zuid; HttpOnly; Path=/access".parse().unwrap(),
        );
        assert_eq!(extract_zuid(&headers).as_deref(), Some("fresh-zuid"));
    }

    #[tokio::test]
    async fn rebind_keeps_the_old_refresh_token_on_a_deletion_cookie() {
        use httpmock::Method::POST;
        use httpmock::MockServer;
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/v14/access");
            then.status(200)
                .header("content-type", "application/json")
                .header(
                    "set-cookie",
                    "zuid=; Expires=Thu, 01 Jan 1970 00:00:00 GMT; Path=/access",
                )
                .body(
                    r#"{"access_token":"at-456","token_type":"Bearer","expires_in":900,
                        "user":"00000000-0000-0000-0000-000000000abc"}"#,
                );
        });
        let http = HttpClient::new(&server.base_url(), true).expect("HttpClient::new");
        let outcome = http
            .rebind_access_token(14, "old-good-zuid", "client-1")
            .await
            .expect("rebind succeeds");
        assert_eq!(
            outcome.refresh_token, "old-good-zuid",
            "a deletion cookie must not clobber the persisted refresh token"
        );
    }

    #[tokio::test]
    async fn rebind_with_empty_refresh_token_is_session_expired_not_a_panic() {
        // Persisted state can be empty (this very bug used to write
        // it); the boundary must return the documented recoverable
        // error instead of tripping a debug assert in consumer builds.
        let server = httpmock::MockServer::start();
        let http = HttpClient::new(&server.base_url(), true).expect("HttpClient::new");
        let err = http
            .rebind_access_token(14, "", "client-1")
            .await
            .expect_err("empty refresh token cannot mint an access token");
        assert!(
            matches!(err, KupferdrahtError::SessionExpired),
            "expected SessionExpired (re-login signal), got {err:?}"
        );
    }
}
