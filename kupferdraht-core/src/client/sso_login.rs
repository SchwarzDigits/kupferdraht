// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! SSO login in matrix-rust-sdk's shape (Step 162, ADR-0043): a URL the
//! application opens in a browser, then a login from the callback.

use std::sync::Arc;

use zeroize::Zeroizing;

use crate::client::builder::ClientBuilder;
use crate::client::input::require_non_empty;
use crate::client::pending_login::PendingLogin;
use crate::error::KupferdrahtError;
use crate::wire::HttpClient;
use crate::wire::sso::{SsoResult, provider_id, read_callback_url, redirect_pair};

impl ClientBuilder {
    /// The URL that starts an SSO login; the application opens it in a
    /// browser. `provider` names the identity provider by its code; `None`
    /// takes the backend's default one. With a `redirect_url` the browser
    /// comes back to that URL, which the application hands to
    /// [`login_with_sso_callback`](Self::login_with_sso_callback); without
    /// one — in the browser — the backend's page reports the outcome to the
    /// window that opened it.
    ///
    /// # Errors
    /// * `InvalidInput` — `server` unset, an unknown `provider`, no default
    ///   provider on the backend, or a `redirect_url` the backend refuses
    ///   (its scheme must begin with `wire`, its length is bounded).
    /// * `BackendRejected` / `Network` — from looking the provider up.
    pub async fn sso_login_url(
        self,
        redirect_url: Option<&str>,
        provider: Option<&str>,
    ) -> Result<String, KupferdrahtError> {
        let redirects = redirect_url.map(redirect_pair).transpose()?;
        let (http, cookie_label) = self.sso_transport()?;
        let provider = sso_provider(&http, provider).await?;
        http.sso_initiate_login_url(&provider, redirects.as_ref(), &cookie_label)
    }

    /// Log in from the URL the browser came back to after
    /// [`sso_login_url`](Self::sso_login_url). Returns a [`PendingLogin`]
    /// whose `register_device` finishes the login as after a password login.
    ///
    /// # Errors
    /// * `InvalidInput` — `server` unset, or the URL carries no outcome.
    /// * `BackendRejected` — the backend refused the login; the message
    ///   carries its reason.
    /// * `SessionExpired` — the session in the callback is no longer valid.
    pub async fn login_with_sso_callback(
        self,
        callback_url: &str,
    ) -> Result<Arc<PendingLogin>, KupferdrahtError> {
        let outcome = read_callback_url(callback_url)?;
        self.login_with_sso_outcome(outcome).await
    }

    /// Log in from how an SSO login ended: read from the callback, or in the
    /// browser from the backend page's message.
    ///
    /// # Errors
    /// As [`login_with_sso_callback`](Self::login_with_sso_callback).
    pub(crate) async fn login_with_sso_outcome(
        self,
        outcome: SsoResult,
    ) -> Result<Arc<PendingLogin>, KupferdrahtError> {
        let session_cookie = signed_in_cookie(outcome)?;
        let session_cookie = session_cookie.as_ref().map_or("", |c| c.as_str());
        self.login_with_session_cookie(session_cookie).await
    }
}

/// The provider's id: the named one once the backend knows it, else the
/// backend's default.
///
/// # Errors
/// `InvalidInput` for an empty or unknown provider, or none named and no
/// default; `BackendRejected` / `Network` from the lookups.
async fn sso_provider(
    http: &HttpClient,
    provider: Option<&str>,
) -> Result<String, KupferdrahtError> {
    let Some(provider) = provider else {
        let default = http.sso_default_code().await?.ok_or_else(|| {
            KupferdrahtError::InvalidInput(
                "no SSO provider named and the backend has no default one".into(),
            )
        })?;
        return Ok(provider_id(&default).to_owned());
    };
    let id = provider_id(provider);
    require_non_empty("sso_login_url", &[("provider", id)])?;
    if !http.sso_code_exists(id).await? {
        return Err(KupferdrahtError::InvalidInput(format!(
            "SSO provider not recognized: {provider}"
        )));
    }
    Ok(id.to_owned())
}

/// The session cookie of a sign-in — `None` when the browser's jar holds
/// it — or the refusal as an error.
///
/// # Errors
/// `BackendRejected` carrying the backend's reason.
fn signed_in_cookie(outcome: SsoResult) -> Result<Option<Zeroizing<String>>, KupferdrahtError> {
    match outcome {
        SsoResult::SignedIn { session_cookie } => Ok(session_cookie),
        SsoResult::Refused { reason } if reason.is_empty() => Err(
            KupferdrahtError::BackendRejected("the SSO login was refused".into()),
        ),
        SsoResult::Refused { reason } => Err(KupferdrahtError::BackendRejected(format!(
            "the SSO login was refused: {reason}"
        ))),
    }
}
