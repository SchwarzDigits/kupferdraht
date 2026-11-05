// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 141 — the one redirect an asset download follows.
//!
//! `GET /v14/assets/{domain}/{id}` answers `302` with `Location` a
//! pre-signed URL on cargohold's asset store (backend-spec §15.2). Both
//! reqwest clients keep `redirect::Policy::none()` (Step 132, M2);
//! `download_asset` follows this one hop by hand, as a GET without the
//! bearer token or the `Asset-Token` — the URL carries its own signature.

use reqwest::StatusCode;
use reqwest::header::LOCATION;
use tracing::debug;
use url::Url;

use crate::error::KupferdrahtError;

use super::HttpClient;
use super::assets::stalled_download;

impl HttpClient {
    /// Follow an asset download's redirect: one GET on the transfer
    /// client to the `Location` of `redirect`, without the bearer token,
    /// the `Asset-Token` or any other header. The answer must be a
    /// success — a further redirect is refused (one hop).
    ///
    /// # Errors
    /// `BackendRejected` for a missing or refused location (see
    /// [`redirect_target`]) and for a non-success answer of the target,
    /// naming its host and status only; `Network` when the target is
    /// unreachable or silent past the transfer idle timeout (Step 98).
    pub(super) async fn fetch_redirected_asset(
        &self,
        redirect: reqwest::Response,
    ) -> Result<reqwest::Response, KupferdrahtError> {
        debug_assert!(
            is_followed_redirect(redirect.status()),
            "fetch_redirected_asset: {} is not a followed redirect",
            redirect.status()
        );
        let location = redirect
            .headers()
            .get(LOCATION)
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| refused("no Location"))?;
        let target = redirect_target(location, self.permit_http)?;
        let host = target.host_str().unwrap_or_default().to_string();
        debug!(%host, "GET asset download, redirected");
        let resp = tokio::time::timeout(self.transfer_idle, self.transfer.get(target).send())
            .await
            .map_err(|_| stalled_download(self.transfer_idle))?
            .map_err(|e| {
                KupferdrahtError::Network(format!("GET asset at {host}: {}", e.without_url()))
            })?;
        if !resp.status().is_success() {
            return Err(KupferdrahtError::BackendRejected(format!(
                "download_asset redirected to {host}: {}",
                resp.status()
            )));
        }
        Ok(resp)
    }
}

/// The statuses an asset download follows: `302`, `303` and `307` all
/// mean "GET it there" for a body-less GET. A permanent redirect (`301`,
/// `308`) is not followed — cargohold's link is valid for minutes.
pub(super) fn is_followed_redirect(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::FOUND | StatusCode::SEE_OTHER | StatusCode::TEMPORARY_REDIRECT
    )
}

/// The target of an asset download's redirect: `location` as an absolute
/// `https` URL — `http` only with `permit_http`, as for the base URL —
/// without userinfo.
///
/// # Errors
/// `BackendRejected` for a relative or unparsable location, another
/// scheme, userinfo, or `http` without `permit_http`. The message names
/// the host at most, never the query (the signature is a credential).
pub(super) fn redirect_target(location: &str, permit_http: bool) -> Result<Url, KupferdrahtError> {
    let target = Url::parse(location).map_err(|_| refused("not an absolute URL"))?;
    match target.scheme() {
        "https" => {}
        "http" if permit_http => {}
        "http" => return Err(refused("http without permit_http")),
        _ => return Err(refused("not an https URL")),
    }
    if !target.username().is_empty() || target.password().is_some() {
        return Err(refused("userinfo in the location"));
    }
    Ok(target)
}

/// A refused redirect of an asset download; `reason` never carries the
/// location itself.
fn refused(reason: &str) -> KupferdrahtError {
    KupferdrahtError::BackendRejected(format!("download_asset redirect refused: {reason}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIGNED: &str =
        "https://assets.example.com/wire-assets/v3/otr/3-2-key?X-Amz-Signature=sig141";

    fn refused(location: &str, permit_http: bool) -> String {
        match redirect_target(location, permit_http) {
            Err(KupferdrahtError::BackendRejected(message)) => message,
            other => {
                panic!("{location:?} (permit_http={permit_http}) must be refused, got {other:?}")
            }
        }
    }

    #[test]
    fn an_absolute_https_location_is_the_target() {
        for permit_http in [false, true] {
            let target = redirect_target(SIGNED, permit_http).expect("an https location");
            assert_eq!(target.as_str(), SIGNED);
        }
    }

    #[test]
    fn http_is_followed_only_with_permit_http() {
        let plain = "http://assets.example.com/wire-assets/v3/otr/3-2-key?X-Amz-Signature=sig141";
        refused(plain, false);
        let target = redirect_target(plain, true).expect("http with permit_http");
        assert_eq!(target.as_str(), plain);
    }

    #[test]
    fn a_location_that_is_no_absolute_https_url_is_refused() {
        for location in [
            "",
            "/wire-assets/v3/otr/3-2-key",
            "wire-assets/v3/otr/3-2-key",
            "//assets.example.com/k",
            "not a url",
            "https://",
            "ftp://assets.example.com/k",
            "file:///etc/passwd",
            "data:text/plain,x",
            "ws://assets.example.com/k",
        ] {
            refused(location, true);
        }
    }

    #[test]
    fn a_location_with_userinfo_is_refused() {
        for location in [
            "https://user:pw@assets.example.com/k",
            "https://user@assets.example.com/k",
            "https://:pw@assets.example.com/k",
        ] {
            refused(location, true);
        }
    }

    #[test]
    fn a_refusal_never_repeats_the_query() {
        let message = refused("http://assets.example.com/k?X-Amz-Signature=sig141", false);
        assert!(
            !message.contains("sig141"),
            "the signature must not reach an error: {message}"
        );
    }
}
