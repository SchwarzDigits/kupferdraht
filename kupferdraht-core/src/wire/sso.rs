// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! How the backend hands a finished SSO login back — Step 162. Pure, no HTTP.
//!
//! Observed in the backend's source (`docs/wire/kalium-readings.md` #30) and
//! live-confirmed (`docs/observations.md` §107): given a pair of redirect URLs
//! on the start URL, the backend sends the browser to one of them after the
//! identity provider and writes the outcome into a placeholder of that URL —
//! `$cookie` on the success URL becomes the session cookie's whole
//! `Set-Cookie` line, percent-encoded; `$label` on the failure URL the reason.
//! Each redirect's scheme must begin with `wire`, and each URL may be at most
//! 140 bytes long as the backend re-serializes it: every query byte outside
//! `A–Z a–z 0–9 - . _ ~` percent-encoded, the placeholder's `$` too. Without
//! the pair the backend answers with a page that reports `AUTH_SUCCESS` or
//! `AUTH_ERROR` to the window that opened it, and the cookie lands in the
//! browser's own cookie jar.

use url::{Position, Url};
use zeroize::Zeroizing;

use crate::error::KupferdrahtError;

/// Our query parameter on the success URL; the backend fills its value.
const SIGNED_IN_PARAMETER: &str = "credential";
/// Our query parameter on the failure URL; the backend fills its value.
const REFUSED_PARAMETER: &str = "reason";
/// The backend's placeholder for the session cookie's `Set-Cookie` line.
const COOKIE_PLACEHOLDER: &str = "$cookie";
/// The backend's placeholder for the reason of a refusal.
const REASON_PLACEHOLDER: &str = "$label";
/// What a redirect's scheme must begin with.
const REDIRECT_SCHEME_PREFIX: &str = "wire";
/// The longest redirect URL the backend accepts, in bytes as it counts them.
const REDIRECT_MAX_BYTES: usize = 140;
/// The session cookie's name, as the start of its `Set-Cookie` line.
const SESSION_COOKIE_PREFIX: &str = "zuid=";
/// The prefix of the provider code shown to users; the endpoint wants the
/// bare id.
const PROVIDER_CODE_PREFIX: &str = "wire-";
/// The page message of a successful login.
#[cfg(any(target_arch = "wasm32", test))]
const PAGE_SIGNED_IN: &str = "AUTH_SUCCESS";
/// The page message of a refused login.
#[cfg(any(target_arch = "wasm32", test))]
const PAGE_REFUSED: &str = "AUTH_ERROR";

/// The two redirect URLs the start URL carries, built from the one
/// redirect URL the application names.
pub(crate) struct RedirectPair {
    /// Where the backend sends a successful login, with the placeholder for
    /// the session cookie.
    pub(crate) on_success: String,
    /// Where the backend sends a refused login, with the placeholder for the
    /// reason.
    pub(crate) on_refusal: String,
}

/// How an SSO login ended, read from the callback.
pub(crate) enum SsoResult {
    /// The user signed in. `session_cookie` is the cookie's value from a
    /// redirect callback; `None` when the browser's jar holds it.
    SignedIn {
        session_cookie: Option<Zeroizing<String>>,
    },
    /// The backend refused the login, for `reason` (empty when it named none).
    Refused { reason: String },
}

/// The provider's id for the endpoint: the code shown to users carries a
/// `wire-` prefix the endpoint refuses; a bare id passes through.
pub(crate) fn provider_id(code: &str) -> &str {
    code.strip_prefix(PROVIDER_CODE_PREFIX).unwrap_or(code)
}

/// The redirect pair for `redirect_url`: the URL with our success parameter
/// and with our refusal parameter, each carrying the backend's placeholder
/// unescaped.
///
/// # Errors
/// `InvalidInput` when `redirect_url` is no URL, its scheme does not begin
/// with `wire`, or a URL of the pair is longer than the backend accepts.
pub(crate) fn redirect_pair(redirect_url: &str) -> Result<RedirectPair, KupferdrahtError> {
    let base = Url::parse(redirect_url)
        .map_err(|e| KupferdrahtError::InvalidInput(format!("SSO redirect URL: {e}")))?;
    if !base.scheme().starts_with(REDIRECT_SCHEME_PREFIX) {
        return Err(KupferdrahtError::InvalidInput(format!(
            "SSO redirect URL: the backend takes only schemes beginning with \
             `{REDIRECT_SCHEME_PREFIX}`, not `{}`",
            base.scheme()
        )));
    }
    let on_success = with_parameter(&base, SIGNED_IN_PARAMETER, COOKIE_PLACEHOLDER);
    let on_refusal = with_parameter(&base, REFUSED_PARAMETER, REASON_PLACEHOLDER);
    for url in [&on_success, &on_refusal] {
        let measured = measured_length(url);
        if measured > REDIRECT_MAX_BYTES {
            return Err(KupferdrahtError::InvalidInput(format!(
                "SSO redirect URL: {measured} bytes as the backend counts them, \
                 at most {REDIRECT_MAX_BYTES}"
            )));
        }
    }
    Ok(RedirectPair {
        on_success: on_success.into(),
        on_refusal: on_refusal.into(),
    })
}

/// `base` with `name=value` appended to its query, `value` unescaped.
fn with_parameter(base: &Url, name: &str, value: &str) -> Url {
    let mut url = base.clone();
    let query = match base.query() {
        Some(own) if !own.is_empty() => format!("{own}&{name}={value}"),
        _ => format!("{name}={value}"),
    };
    url.set_query(Some(&query));
    url
}

/// The length of `url` as the backend counts it: re-serialized, with every
/// query key and value percent-encoded outside `A–Z a–z 0–9 - . _ ~`.
fn measured_length(url: &Url) -> usize {
    let pairs: Vec<usize> = url
        .query_pairs()
        .map(|(key, value)| encoded_length(&key) + 1 + encoded_length(&value))
        .collect();
    let query = if pairs.is_empty() {
        0
    } else {
        pairs.iter().sum::<usize>() + pairs.len()
    };
    let fragment = url.fragment().map_or(0, |f| 1 + f.len());
    url[..Position::AfterPath].len() + query + fragment
}

/// The length of `text` with every byte outside `A–Z a–z 0–9 - . _ ~`
/// percent-encoded.
fn encoded_length(text: &str) -> usize {
    text.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                1
            } else {
                3
            }
        })
        .sum()
}

/// How the login ended, read from the URL the backend redirected to.
///
/// # Errors
/// `InvalidInput` when the URL carries neither a session cookie nor a reason.
pub(crate) fn read_callback_url(callback_url: &str) -> Result<SsoResult, KupferdrahtError> {
    let url = Url::parse(callback_url)
        .map_err(|e| KupferdrahtError::InvalidInput(format!("SSO callback URL: {e}")))?;
    let mut cookie_line = None;
    let mut reason = None;
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            SIGNED_IN_PARAMETER => cookie_line = Some(Zeroizing::new(value.into_owned())),
            REFUSED_PARAMETER => reason = Some(value.into_owned()),
            _ => {}
        }
    }
    if let Some(line) = cookie_line {
        return Ok(SsoResult::SignedIn {
            session_cookie: Some(session_cookie(&line)?),
        });
    }
    reason
        .map(|reason| SsoResult::Refused { reason })
        .ok_or_else(|| KupferdrahtError::InvalidInput("SSO callback URL: no outcome".into()))
}

/// The session cookie's value from its `Set-Cookie` line: the first
/// attribute, up to the first `;`. The value itself holds `=`.
///
/// # Errors
/// `InvalidInput` when the line names another cookie or an empty one.
fn session_cookie(line: &str) -> Result<Zeroizing<String>, KupferdrahtError> {
    line.split(';')
        .next()
        .and_then(|first| first.trim().strip_prefix(SESSION_COOKIE_PREFIX))
        .filter(|value| !value.is_empty())
        .map(|value| Zeroizing::new(value.to_owned()))
        .ok_or_else(|| KupferdrahtError::InvalidInput("SSO callback URL: no session cookie".into()))
}

/// How the login ended, read from the message the backend's page posts to
/// the window that opened it: its `type` as `kind`, its reason if any.
///
/// # Errors
/// `InvalidInput` for a message of any other kind.
#[cfg(any(target_arch = "wasm32", test))]
pub(crate) fn read_page_message(
    kind: &str,
    reason: Option<&str>,
) -> Result<SsoResult, KupferdrahtError> {
    match kind {
        PAGE_SIGNED_IN => Ok(SsoResult::SignedIn {
            session_cookie: None,
        }),
        PAGE_REFUSED => Ok(SsoResult::Refused {
            reason: reason.unwrap_or_default().to_owned(),
        }),
        other => Err(KupferdrahtError::InvalidInput(format!(
            "SSO page message: `{other}` reports no SSO outcome"
        ))),
    }
}

/// The data the backend's page posts to the window that opened it:
/// `{type, payload: {label, errors}}`.
#[cfg(any(target_arch = "wasm32", test))]
#[derive(serde::Deserialize)]
pub(crate) struct PageMessage {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    payload: Option<PagePayload>,
}

/// A refusal's details in a [`PageMessage`]; only the reason is read.
#[cfg(any(target_arch = "wasm32", test))]
#[derive(serde::Deserialize)]
struct PagePayload {
    #[serde(default)]
    label: Option<String>,
}

#[cfg(any(target_arch = "wasm32", test))]
impl PageMessage {
    /// How the login ended, by [`read_page_message`].
    ///
    /// # Errors
    /// `InvalidInput` for a message of any other kind.
    pub(crate) fn outcome(&self) -> Result<SsoResult, KupferdrahtError> {
        let reason = self.payload.as_ref().and_then(|p| p.label.as_deref());
        read_page_message(&self.kind, reason)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REDIRECT: &str = "wire-kupferdraht://sso";

    fn session_cookie(result: SsoResult) -> Option<String> {
        match result {
            SsoResult::SignedIn { session_cookie } => session_cookie.map(|c| c.to_string()),
            SsoResult::Refused { reason } => panic!("expected a sign-in, got a refusal: {reason}"),
        }
    }

    fn reason(result: SsoResult) -> String {
        match result {
            SsoResult::Refused { reason } => reason,
            SsoResult::SignedIn { .. } => panic!("expected a refusal, got a sign-in"),
        }
    }

    // --- the redirect pair ----------------------------------------------

    /// The placeholders stand unescaped, the form the backend replaced
    /// (live-confirmed, `docs/observations.md` §107).
    #[test]
    fn the_pair_carries_our_parameters_with_the_placeholders() {
        let pair = redirect_pair(REDIRECT).expect("a wire scheme is accepted");
        assert_eq!(pair.on_success, "wire-kupferdraht://sso?credential=$cookie");
        assert_eq!(pair.on_refusal, "wire-kupferdraht://sso?reason=$label");
    }

    #[test]
    fn the_pair_keeps_the_applications_own_query() {
        let pair = redirect_pair("wire-kupferdraht://sso?account=7").expect("accepted");
        assert_eq!(
            pair.on_success,
            "wire-kupferdraht://sso?account=7&credential=$cookie"
        );
        assert_eq!(
            pair.on_refusal,
            "wire-kupferdraht://sso?account=7&reason=$label"
        );
    }

    #[test]
    fn a_redirect_the_backend_refuses_is_invalid_input() {
        for redirect in [
            "",
            "not a url",
            "kupferdraht://sso",
            "https://app.example.test/sso",
        ] {
            assert!(
                matches!(
                    redirect_pair(redirect),
                    Err(KupferdrahtError::InvalidInput(_))
                ),
                "{redirect:?} must be refused"
            );
        }
    }

    /// The backend counts a URL re-serialized, the placeholder's `$` as
    /// `%24`: the success URL's `?credential=%24cookie` is the longer end.
    #[test]
    fn a_pair_longer_than_the_backend_accepts_is_invalid_input() {
        let spare = 140 - REDIRECT.len() - "?credential=%24cookie".len();
        let at_limit = format!("{REDIRECT}/{}", "x".repeat(spare - 1));
        redirect_pair(&at_limit).expect("140 bytes as the backend counts them are accepted");
        let over = format!("{REDIRECT}/{}", "x".repeat(spare));
        assert!(
            matches!(redirect_pair(&over), Err(KupferdrahtError::InvalidInput(_))),
            "one byte over the limit must be refused"
        );
    }

    /// The application's own query counts encoded too: 40 slashes are 120
    /// bytes to the backend.
    #[test]
    fn the_applications_own_query_counts_as_the_backend_encodes_it() {
        let redirect = format!("{REDIRECT}?next={}", "/".repeat(40));
        assert!(redirect.len() + "&credential=$cookie".len() <= 140);
        assert!(
            matches!(
                redirect_pair(&redirect),
                Err(KupferdrahtError::InvalidInput(_))
            ),
            "{redirect:?} is longer than 140 bytes encoded"
        );
    }

    // --- the provider -------------------------------------------------

    #[test]
    fn the_provider_code_loses_its_prefix() {
        assert_eq!(
            provider_id("wire-0f1e2d3c-4b5a-4978-8796-a5b4c3d2e1f0"),
            "0f1e2d3c-4b5a-4978-8796-a5b4c3d2e1f0"
        );
        assert_eq!(
            provider_id("0f1e2d3c-4b5a-4978-8796-a5b4c3d2e1f0"),
            "0f1e2d3c-4b5a-4978-8796-a5b4c3d2e1f0"
        );
    }

    // --- the callback URL -------------------------------------------------

    /// The cookie's whole `Set-Cookie` line, percent-encoded, its value
    /// with `=`, `.`, `-` and `_` and its expiry with a comma
    /// (live-confirmed, `docs/observations.md` §107).
    const COOKIE_LINE: &str = "zuid%3Da1-b2_c3.d%3D4.e%3D5%3B%20Path%3D%2Faccess%3B%20Expires%3DWed%2C%2018-Nov-2026%2010%3A00%3A00%20GMT%3B%20HttpOnly";

    #[test]
    fn the_callback_yields_the_session_cookie() {
        for callback in [
            format!("wire-kupferdraht://sso?credential={COOKIE_LINE}"),
            format!("wire-kupferdraht://sso?account=7&credential={COOKIE_LINE}"),
        ] {
            let result = read_callback_url(&callback).expect("a callback with a cookie");
            assert_eq!(
                session_cookie(result).as_deref(),
                Some("a1-b2_c3.d=4.e=5"),
                "{callback}"
            );
        }
    }

    #[test]
    fn a_refusal_yields_its_reason() {
        let result = read_callback_url("wire-kupferdraht://sso?reason=forbidden")
            .expect("a callback with a reason");
        assert_eq!(reason(result), "forbidden");
    }

    #[test]
    fn a_callback_without_an_outcome_is_invalid_input() {
        for callback in [
            "wire-kupferdraht://sso",
            // The placeholder, not replaced.
            "wire-kupferdraht://sso?credential=%24cookie",
            // A cookie line without the session cookie.
            "wire-kupferdraht://sso?credential=userid%3Dabc",
            // The session cookie, empty.
            "wire-kupferdraht://sso?credential=zuid%3D",
            "not a url",
        ] {
            assert!(
                matches!(
                    read_callback_url(callback),
                    Err(KupferdrahtError::InvalidInput(_))
                ),
                "{callback:?} must be refused"
            );
        }
    }

    // --- the page's message -----------------------------------------------

    #[test]
    fn the_page_message_reports_a_sign_in_or_a_refusal() {
        let signed_in = read_page_message("AUTH_SUCCESS", None).expect("success");
        assert_eq!(
            session_cookie(signed_in),
            None,
            "the browser's jar holds the cookie"
        );
        let refused = read_page_message("AUTH_ERROR", Some("forbidden")).expect("refusal");
        assert_eq!(reason(refused), "forbidden");
        assert!(
            matches!(
                read_page_message("AUTH_ERROR", None),
                Ok(SsoResult::Refused { .. })
            ),
            "a refusal without a reason is still a refusal"
        );
        assert!(
            matches!(
                read_page_message("SOMETHING_ELSE", None),
                Err(KupferdrahtError::InvalidInput(_))
            ),
            "any other message is not an SSO outcome"
        );
    }

    #[test]
    fn the_page_message_is_read_from_its_data() {
        let signed_in: PageMessage =
            serde_json::from_str(r#"{"type":"AUTH_SUCCESS"}"#).expect("parses");
        assert_eq!(session_cookie(signed_in.outcome().expect("success")), None);
        let refused: PageMessage = serde_json::from_str(
            r#"{"type":"AUTH_ERROR","payload":{"label":"forbidden","errors":["denied"]}}"#,
        )
        .expect("parses");
        assert_eq!(reason(refused.outcome().expect("refusal")), "forbidden");
    }
}
