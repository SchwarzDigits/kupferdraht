// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `HttpClient` — thin wrapper around `reqwest::Client` carrying the
//! target server's base URL.
//!
//! The Wire API surface Kupferdraht consumes is decomposed into one
//! submodule per swagger grouping:
//!
//! - [`auth`] — `POST /login`, `POST /access`, `GET /self`.
//! - [`clients`] — `POST /v{N}/clients`, `PUT /v{N}/clients/{id}`.
//! - [`mls`] — `POST /v{N}/mls/key-packages/self/{id}` +
//!   `GET /v{N}/mls/public-keys`.
//! - [`conversations`] — `POST /v{N}/conversations/list-ids` +
//!   `POST /v{N}/conversations/list`.
//! - [`conversation_join`] — `GET` + `POST /v{N}/conversations/join`
//!   (redeeming a guest link, Steps 8 and 122).
//! - [`feature_configs`] — `GET /v{N}/feature-configs` +
//!   `GET /v{N}/properties` + `GET /v{N}/properties/{key}` (Step 10
//!   server-administered configuration).
//! - [`groups`] — `GET /v{N}/conversations/{d}/{id}/groupinfo` +
//!   `POST /v{N}/mls/commit-bundles` (external-commit join flow).
//! - [`key_packages`] — `POST /v{N}/mls/key-packages/claim/{d}/{u}`
//!   (Step 6b invite flow).
//! - [`messages`] — `POST /v{N}/mls/messages` (Step 4 application
//!   message send).
//! - [`notifications`] — `GET /v{N}/notifications` (REST drain
//!   alternative to the WebSocket).
//! - [`users`] — `POST /v{N}/list-users`.
//!
//! This file hosts the type, its constructor, the shared URL helper,
//! the stateless `GET /api-version` call, and the shared non-2xx
//! helper [`reject_from_response`].

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock, Weak};

use reqwest::{Client as ReqwestClient, Url};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use tracing::{debug, warn};

use crate::error::KupferdrahtError;
use crate::wire::dto::ApiVersionResponse;

/// Seam the transport layer uses to recover from a `401`: refresh the
/// access token (cookie-auth `POST /access`) and hand back the fresh
/// one. Implemented by `Client` — the transport never sees `Client`
/// itself, only this trait, so the layer split (ADR-0009) holds.
///
/// Installed once on the [`HttpClient`] that a logged-in `Client` owns
/// (via [`HttpClient::set_refresher`]); the login-phase `HttpClient`
/// has none, so login / register / `/access` calls never auto-refresh.
// The refresher future is `Send` on native (it may be driven from a
// multi-thread tokio runtime) but `!Send` on wasm — the browser is
// single-threaded and reqwest's fetch future is `!Send`. The bound is
// therefore target-conditional (Step 44).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) trait TokenRefresh: Send + Sync {
    /// Acquisition time (ms) of the access token currently in use —
    /// the "stale" marker [`Self::refresh_and_get_token`]'s herd-guard
    /// compares against so N concurrent `401`s collapse to one refresh.
    fn access_token_issued_at_ms(&self) -> i64;

    /// Refresh once (guarded) and return the fresh access token. A
    /// caller whose `stale_issued_at` is already behind the live
    /// acquisition time gets the current token without a second
    /// `/access` round-trip.
    fn refresh_and_get_token<'a>(
        &'a self,
        stale_issued_at: i64,
    ) -> Pin<Box<dyn Future<Output = Result<String, KupferdrahtError>> + Send + 'a>>;
}

/// wasm variant of [`TokenRefresh`] without the `Send`/`Sync` bounds —
/// the browser is single-threaded, and `Weak<dyn Trait>: Sync` would in turn
/// require the trait object to be `Send` (which the fetch-backed refresher is
/// not).
#[cfg(target_arch = "wasm32")]
pub(crate) trait TokenRefresh {
    /// See the native variant.
    fn access_token_issued_at_ms(&self) -> i64;
    /// See the native variant.
    fn refresh_and_get_token<'a>(
        &'a self,
        stale_issued_at: i64,
    ) -> Pin<Box<dyn Future<Output = Result<String, KupferdrahtError>> + 'a>>;
}

#[cfg(not(target_arch = "wasm32"))]
mod asset_redirect;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod asset_redirect_tests;
pub(crate) mod assets;
pub(crate) mod auth;
mod clients;
#[cfg(any(target_arch = "wasm32", test))]
pub(crate) use clients::WEB_DEVICE_MODEL;
mod conversation_join;
mod conversations;
pub(crate) mod feature_configs;
mod groups;
mod key_packages;
mod messages;
mod mls;
pub(crate) mod notifications;
mod one_to_one;
mod registration;
mod sso;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod transfer_timeout_tests;
mod users;

/// Thin wrapper around `reqwest::Client` carrying the target server's
/// base URL. Methods live in the three submodules above plus the
/// [`HttpClient::new`], [`HttpClient::api_version`] entries below.
pub struct HttpClient {
    client: ReqwestClient,
    /// Asset up- and downloads (Step 98): same connect timeout, but no
    /// total deadline — `reqwest`'s `timeout` would cut off a transfer
    /// whose bytes keep flowing. Native only.
    #[cfg(not(target_arch = "wasm32"))]
    transfer: ReqwestClient,
    /// A download fails after this long without a byte (Step 98).
    #[cfg(not(target_arch = "wasm32"))]
    transfer_idle: std::time::Duration,
    base_url: Url,
    /// The `permit_http` opt-in this client was built with; the socket
    /// URL builders apply the same scheme rule (Step 132).
    permit_http: bool,
    /// Token-refresh seam, installed by the owning `Client` once it
    /// exists ([`HttpClient::set_refresher`]). `None` on the
    /// login-phase client, so a `401` there surfaces verbatim instead
    /// of triggering a refresh. A `Weak` ref breaks the
    /// `Client` → `HttpClient` → `Client` ownership cycle.
    refresher: OnceLock<Weak<dyn TokenRefresh>>,
}

/// Hard upper bound on the time a single request may spend
/// connecting to the backend. Short enough to surface a dropped SYN
/// within a few seconds; long enough to forgive a slow TLS handshake
/// on a congested mobile link.
// Native only: the browser `fetch` backend has no connect/request timeout knobs.
#[cfg(not(target_arch = "wasm32"))]
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Hard upper bound on the end-to-end time of a single request.
/// Wire's slowest endpoints today (large conversation list fetches,
/// early-pool key-package claims) complete well inside 30 s; anything
/// past it is almost certainly a stall.
#[cfg(not(target_arch = "wasm32"))]
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Longest silence a download tolerates — no byte for this long while
/// waiting for the response or between two chunks (Step 98).
#[cfg(not(target_arch = "wasm32"))]
const TRANSFER_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// The timeouts of one [`HttpClient`] (Step 98). Native only. Uploads
/// have no bound of their own: a dead path fails when the OS gives up
/// the connection, and the caller can cancel the send.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone, Copy)]
pub(crate) struct HttpTimeouts {
    /// Connecting to the backend (API calls and transfers).
    pub(crate) connect: std::time::Duration,
    /// One API call, end to end.
    pub(crate) request: std::time::Duration,
    /// A download: longest silence while waiting for bytes.
    pub(crate) transfer_idle: std::time::Duration,
}

#[cfg(not(target_arch = "wasm32"))]
impl HttpTimeouts {
    /// The production bounds.
    pub(crate) const PRODUCTION: Self = Self {
        connect: CONNECT_TIMEOUT,
        request: REQUEST_TIMEOUT,
        transfer_idle: TRANSFER_IDLE_TIMEOUT,
    };
}

/// The socket base URL gets the API URL's scheme rule (Step 132, M3):
/// `wss://` always, `ws://` only with `permit_http`; anything else — and a
/// `ws://` without the opt-in — is `KupferdrahtError::InvalidInput` naming
/// the scheme, before any connection is attempted. An `http(s)://` base is
/// accepted as the scheme the browser upgrades (`wasm::push_socket`).
///
/// # Errors
/// `KupferdrahtError::InvalidInput` as described.
pub(crate) fn checked_websocket_base(
    websocket_base: &str,
    permit_http: bool,
) -> Result<(), KupferdrahtError> {
    let url = Url::parse(websocket_base).map_err(|e| {
        KupferdrahtError::InvalidInput(format!("websocket base `{websocket_base}`: {e}"))
    })?;
    match url.scheme() {
        "wss" | "https" => Ok(()),
        "ws" | "http" if permit_http => Ok(()),
        "ws" | "http" => Err(KupferdrahtError::InvalidInput(format!(
            "websocket base must be wss:// (ws:// only with permit_http), got `{}://`",
            url.scheme()
        ))),
        other => Err(KupferdrahtError::InvalidInput(format!(
            "websocket base must be wss:// or ws://, got `{other}://`"
        ))),
    }
}

/// Parse the API base URL; plain HTTP only with `permit_http`.
fn checked_base_url(api_base_url: &str, permit_http: bool) -> Result<Url, KupferdrahtError> {
    let base_url = Url::parse(api_base_url)
        .map_err(|e| KupferdrahtError::InvalidInput(format!("api_base_url: {e}")))?;
    if base_url.scheme() != "https" && !permit_http {
        return Err(KupferdrahtError::InvalidInput(format!(
            "api_base_url must be https:// (got {}); pass ClientBuilder::permit_http() \
             to allow plain HTTP for test-only mock backends",
            base_url.scheme()
        )));
    }
    Ok(base_url)
}

/// Build one `reqwest` client. Deliberately NO `cookie_store(true)`.
/// Wire's cookie semantics force us to set the `Cookie` header
/// explicitly on every /access call (observations.md §18); leaving
/// reqwest's jar enabled then sends BOTH our manual cookie AND a
/// store-derived stale one in a single request, and Wire's nginx picks
/// the older invalidated value, returning `403 invalid-credentials`
/// (live-confirmed — login's rebind issued cookie `ucD8...`,
/// the resume's rebind sent `Cookie: zuid=ucD8...` plus a duplicate
/// store-derived `zuid=byav...`, and Wire rejected).
fn build_reqwest(builder: reqwest::ClientBuilder) -> Result<ReqwestClient, KupferdrahtError> {
    builder
        .build()
        .map_err(|e| KupferdrahtError::Network(format!("build reqwest client: {e}")))
}

impl HttpClient {
    /// Build an HTTP client rooted at the supplied API base URL.
    ///
    /// Rejects non-HTTPS URLs unless `permit_http` is `true`.
    /// `permit_http` is intended **only** for `httpmock`-backed
    /// integration tests — production callers must pass `false`.
    ///
    /// Sets a connect-timeout of [`CONNECT_TIMEOUT`] and an overall
    /// request-timeout of [`REQUEST_TIMEOUT`] so a transport hang
    /// cannot stall the login pipeline indefinitely (open.md §4
    /// from the first-pass audit). Asset transfers get no overall
    /// deadline (Step 98): a download fails after
    /// [`TRANSFER_IDLE_TIMEOUT`] without a byte, an upload only when
    /// the connection fails.
    ///
    /// # Errors
    /// * `KupferdrahtError::InvalidInput` if the base URL cannot be
    ///   parsed, or if it is not `https://` and `permit_http` is
    ///   `false`.
    /// * `KupferdrahtError::Network` if the underlying reqwest client
    ///   fails to build.
    pub fn new(api_base_url: &str, permit_http: bool) -> Result<Self, KupferdrahtError> {
        #[cfg(not(target_arch = "wasm32"))]
        return Self::with_timeouts(api_base_url, permit_http, HttpTimeouts::PRODUCTION);
        #[cfg(target_arch = "wasm32")]
        Ok(Self {
            client: build_reqwest(ReqwestClient::builder())?,
            base_url: checked_base_url(api_base_url, permit_http)?,
            permit_http,
            refresher: OnceLock::new(),
        })
    }

    /// Whether plain `http://` and `ws://` were permitted when this client
    /// was built (Step 132: the socket URL follows the same rule).
    pub(crate) fn permit_http(&self) -> bool {
        self.permit_http
    }

    /// [`Self::new`] with explicit timeouts (Step 98; tests shrink them).
    ///
    /// # Errors
    /// As [`Self::new`].
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn with_timeouts(
        api_base_url: &str,
        permit_http: bool,
        timeouts: HttpTimeouts,
    ) -> Result<Self, KupferdrahtError> {
        let base_url = checked_base_url(api_base_url, permit_http)?;
        // Step 132 (M2): reqwest follows no redirect. A followed 3xx could
        // re-send a login body to a host of the server's choosing (307) or
        // downgrade to plain HTTP; a 3xx answer surfaces as the status it
        // is. The one redirect Wire's API issues — an asset download's 302
        // to the asset store — `download_asset` follows by hand, without
        // credentials (Step 141, `asset_redirect.rs`). (The browser build
        // has no policy knob: `fetch` follows redirects by itself.)
        let client = build_reqwest(
            ReqwestClient::builder()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(timeouts.connect)
                .timeout(timeouts.request),
        )?;
        let transfer = build_reqwest(
            ReqwestClient::builder()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(timeouts.connect),
        )?;
        Ok(Self {
            client,
            transfer,
            transfer_idle: timeouts.transfer_idle,
            base_url,
            permit_http,
            refresher: OnceLock::new(),
        })
    }

    /// Install the token-refresh seam. Called once by `Client::new`
    /// after the owning `Client` exists; a second call is a no-op (the
    /// `OnceLock` keeps the first handle).
    pub(crate) fn set_refresher(&self, refresher: Weak<dyn TokenRefresh>) {
        let _ = self.refresher.set(refresher);
    }

    /// Upgrade the installed refresh seam, if any. `None` on the
    /// login-phase client or once the owning `Client` has been dropped.
    fn refresher(&self) -> Option<Arc<dyn TokenRefresh>> {
        self.refresher.get().and_then(Weak::upgrade)
    }

    /// `segment` checked as one URL path segment built from foreign data
    /// (a peer's asset key, a backend property key, a conversation or
    /// client id). Only RFC 3986 unreserved characters (`A-Z a-z 0-9 - .
    /// _ ~`) pass, and neither `.` nor `..` — the value can neither be
    /// empty, add a segment, start a query or fragment, nor
    /// percent-decode into one. Step 95c; on every dynamic path
    /// parameter and the `?client_id=` query value since Step 137.
    ///
    /// # Errors
    /// `KupferdrahtError::InvalidInput` naming `what` otherwise.
    pub(super) fn path_segment<'a>(
        segment: &'a str,
        what: &str,
    ) -> Result<&'a str, KupferdrahtError> {
        let unreserved = |c: char| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | '~');
        if segment.is_empty()
            || segment == "."
            || segment == ".."
            || !segment.chars().all(unreserved)
        {
            return Err(KupferdrahtError::InvalidInput(format!(
                "{what}: {segment:?} is not a single URL path segment"
            )));
        }
        Ok(segment)
    }

    /// A qualified `<local>@<domain>` id split into its two path
    /// segments, each checked as [`Self::path_segment`] (Step 137).
    ///
    /// # Errors
    /// `KupferdrahtError::InvalidInput` naming `what` for an unqualified
    /// id or a part that is not a single path segment.
    pub(super) fn qualified_segments<'a>(
        id: &'a str,
        what: &str,
    ) -> Result<(&'a str, &'a str), KupferdrahtError> {
        let (local, domain) = id.split_once('@').ok_or_else(|| {
            KupferdrahtError::InvalidInput(format!(
                "{what}: `{id}` must be qualified <uuid>@<domain>"
            ))
        })?;
        Ok((
            Self::path_segment(local, what)?,
            Self::path_segment(domain, what)?,
        ))
    }

    pub(super) fn url(&self, path: &str) -> Result<Url, KupferdrahtError> {
        self.base_url
            .join(path)
            .map_err(|e| KupferdrahtError::InvalidInput(format!("url join {path}: {e}")))
    }

    /// `GET /api-version` — returns the list of server-supported
    /// API versions.
    ///
    /// # Errors
    /// Maps transport and deserialization failures onto `KupferdrahtError`.
    pub async fn api_version(&self) -> Result<ApiVersionResponse, KupferdrahtError> {
        let url = self.url("/api-version")?;
        debug!(%url, "GET /api-version");
        let resp = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| KupferdrahtError::Network(format!("GET /api-version: {e}")))?;
        if !resp.status().is_success() {
            return Err(KupferdrahtError::BackendRejected(format!(
                "/api-version returned {}",
                resp.status()
            )));
        }
        resp.json::<ApiVersionResponse>()
            .await
            .map_err(|e| KupferdrahtError::Encoding(format!("/api-version: {e}")))
    }
}

/// Build a `BackendRejected` error from a non-success response,
/// capturing the response body text for post-hoc debugging — bounded to
/// about 1 KiB and without control characters
/// ([`crate::redact::bounded_detail`], Step 137), since the error crosses
/// `UniFFI` and wasm into host logs. Shared by every submodule that talks
/// to Wire.
///
/// A `409 mls-stale-message` is special-cased to the typed
/// [`KupferdrahtError::MlsStaleMessage`] so the send path can match it and
/// self-heal (resync + retry); only that endpoint family ever returns the
/// label. Step 37. A `429` is [`KupferdrahtError::RateLimited`], carrying the
/// `Retry-After` seconds when present. Step 103.
pub(super) async fn reject_from_response(
    resp: reqwest::Response,
    context: &str,
) -> KupferdrahtError {
    let status = resp.status();
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        let retry_after_secs = retry_after_secs(resp.headers());
        warn!(
            op = context,
            ?retry_after_secs,
            "rate-limited by the backend"
        );
        return KupferdrahtError::RateLimited { retry_after_secs };
    }
    let detail = resp.text().await.unwrap_or_default();
    if status == reqwest::StatusCode::CONFLICT && detail.contains("mls-stale-message") {
        return KupferdrahtError::MlsStaleMessage;
    }
    KupferdrahtError::BackendRejected(format!(
        "{context} {status}: {}",
        crate::redact::bounded_detail(&detail)
    ))
}

/// `Retry-After` in delta-seconds; an HTTP-date, a malformed value or no
/// header → `None` (the caller falls back to its own pause). Step 103.
pub(super) fn retry_after_secs(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    headers
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// The `label` of Wire's error envelope (`{"code","label","message"}`);
/// empty when the body carries none.
#[derive(Deserialize)]
pub(super) struct WireErrorBody {
    #[serde(default)]
    pub(super) label: String,
}

/// Classify a response of the standard Wire endpoint shape: `401` →
/// [`KupferdrahtError::Unauthorized`], any other non-2xx →
/// [`reject_from_response`], 2xx → the body deserialized as `T`.
pub(super) async fn decode_response<T: DeserializeOwned>(
    resp: reqwest::Response,
    op: &str,
) -> Result<T, KupferdrahtError> {
    debug_assert!(!op.is_empty(), "op names the call in error texts");
    if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        return Err(KupferdrahtError::Unauthorized);
    }
    if !resp.status().is_success() {
        return Err(reject_from_response(resp, op).await);
    }
    resp.json::<T>()
        .await
        .map_err(|e| KupferdrahtError::Encoding(format!("{op} body: {e}")))
}

impl HttpClient {
    /// Send `req`, map transport errors to [`KupferdrahtError::Network`],
    /// reject non-2xx via [`reject_from_response`], then deserialize the
    /// response body as `T`. `op` flows into all three error messages
    /// (network / rejection / decode) so failures are self-describing.
    ///
    /// Covers the standard Wire endpoint shape used by every submodule;
    /// flows with non-JSON bodies (e.g. streaming asset download) build
    /// the response themselves.
    pub(super) async fn send_and_decode<T: DeserializeOwned>(
        &self,
        req: reqwest::RequestBuilder,
        op: &str,
    ) -> Result<T, KupferdrahtError> {
        let resp = self.send_with_auth_retry(req, op).await?;
        decode_response(resp, op).await
    }

    /// Same as [`HttpClient::send_and_decode`] but discards the response
    /// body. Used by endpoints whose response is "did it succeed?" only
    /// — e.g. `PUT /v{N}/clients/{id}` (key registration), `POST
    /// /v{N}/mls/messages` (application message), `POST
    /// /v{N}/access/logout`.
    pub(super) async fn send_no_body(
        &self,
        req: reqwest::RequestBuilder,
        op: &str,
    ) -> Result<(), KupferdrahtError> {
        let resp = self.send_with_auth_retry(req, op).await?;
        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(KupferdrahtError::Unauthorized);
        }
        if !resp.status().is_success() {
            return Err(reject_from_response(resp, op).await);
        }
        Ok(())
    }

    /// Send `req`; on a `401` refresh the access token once (guarded)
    /// and replay the request with the rotated bearer. Returns the
    /// final response — the retried one when a refresh happened, else
    /// the original. A surviving `401` is classified by the caller
    /// (→ [`KupferdrahtError::Unauthorized`]). This is the single choke
    /// point that keeps an in-session token alive past its ~15-min TTL
    /// for every authenticated call, without each call site wrapping
    /// itself.
    async fn send_with_auth_retry(
        &self,
        req: reqwest::RequestBuilder,
        op: &str,
    ) -> Result<reqwest::Response, KupferdrahtError> {
        // Clone for a potential replay and snapshot the in-flight
        // token's age BEFORE sending — both must reflect the original
        // attempt, not post-refresh state.
        let replay = req.try_clone();
        let stale = self
            .refresher()
            .map_or(0, |r| r.access_token_issued_at_ms());
        let resp = req
            .send()
            .await
            .map_err(|e| KupferdrahtError::Network(format!("{op}: {e}")))?;
        if resp.status() != reqwest::StatusCode::UNAUTHORIZED {
            return Ok(resp);
        }
        match self.replay_after_refresh(replay, stale, op).await? {
            Some(retried) => Ok(retried),
            None => Ok(resp),
        }
    }

    /// Refresh once and replay `replay` with the new bearer. `Ok(None)`
    /// means no retry was possible — no refresher installed (login
    /// phase) or a non-cloneable streaming body — so the caller
    /// surfaces the original `401`. A failed refresh propagates its
    /// error (e.g. `BackendRejected`) and is **not** retried, so a dead
    /// `zuid` cannot loop.
    async fn replay_after_refresh(
        &self,
        replay: Option<reqwest::RequestBuilder>,
        stale_issued_at: i64,
        op: &str,
    ) -> Result<Option<reqwest::Response>, KupferdrahtError> {
        let (Some(refresher), Some(replay)) = (self.refresher(), replay) else {
            return Ok(None);
        };
        let token = refresher.refresh_and_get_token(stale_issued_at).await?;
        let mut request = replay
            .build()
            .map_err(|e| KupferdrahtError::Network(format!("{op} rebuild: {e}")))?;
        let bearer = reqwest::header::HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|e| KupferdrahtError::InvalidInput(format!("{op} bearer header: {e}")))?;
        request
            .headers_mut()
            .insert(reqwest::header::AUTHORIZATION, bearer);
        let resp = self
            .client
            .execute(request)
            .await
            .map_err(|e| KupferdrahtError::Network(format!("{op} retry: {e}")))?;
        Ok(Some(resp))
    }
}

#[cfg(test)]
mod tests {
    use httpmock::MockServer;

    use super::HttpClient;
    use crate::error::KupferdrahtError;

    fn http(base_url: &str) -> HttpClient {
        HttpClient::new(base_url, true).expect("HttpClient::new (permit_http=true)")
    }

    /// Step 137 (scan finding M5): what a hostile id must not be able to
    /// inject — a second segment, a dot segment (raw or percent-encoded),
    /// a query, a fragment. The empty id is a `debug_assert!` precondition
    /// at the sites whose id is the caller's; where it is runtime data
    /// (the asset download, the property key) Step 95c pins it.
    const HOSTILE: [&str; 6] = ["../../self", "a/b", "key?x=1", "key#frag", "%2e%2e", ".."];

    fn assert_refused<T>(site: &str, hostile: &str, result: Result<T, KupferdrahtError>) {
        match result {
            Err(KupferdrahtError::InvalidInput(_)) => {}
            Err(other) => panic!("{site}({hostile:?}): expected InvalidInput, got {other:?}"),
            Ok(_) => panic!("{site}({hostile:?}): the request was sent"),
        }
    }

    /// The conversation endpoints: a qualified id's local part and domain,
    /// the team and conversation ids, the code endpoints, the group info.
    async fn assert_conversation_sites_refuse(client: &HttpClient, h: &str) {
        let local = format!("{h}@example.test");
        let domain = format!("room@{h}");
        for id in [local.as_str(), domain.as_str()] {
            assert_refused(
                "delete_conversation_member room",
                h,
                client
                    .delete_conversation_member(14, "tok", id, "user@example.test")
                    .await,
            );
            assert_refused(
                "delete_conversation_member user",
                h,
                client
                    .delete_conversation_member(14, "tok", "room@example.test", id)
                    .await,
            );
            assert_refused(
                "rename_conversation",
                h,
                client.rename_conversation(14, "tok", id, "name").await,
            );
            assert_refused(
                "set_conversation_access",
                h,
                client
                    .set_conversation_access(14, "tok", id, &["code".into()], &["guest".into()])
                    .await,
            );
            assert_refused(
                "send_typing",
                h,
                client.send_typing(14, "tok", id, "started").await,
            );
        }
        assert_refused(
            "delete_team_conversation team",
            h,
            client.delete_team_conversation(14, "tok", h, "conv").await,
        );
        assert_refused(
            "delete_team_conversation conv",
            h,
            client.delete_team_conversation(14, "tok", "team", h).await,
        );
        assert_refused(
            "create_conversation_code",
            h,
            client.create_conversation_code(14, "tok", h, None).await,
        );
        assert_refused(
            "get_conversation_code",
            h,
            client.get_conversation_code(14, "tok", h).await,
        );
        assert_refused(
            "delete_conversation_code",
            h,
            client.delete_conversation_code(14, "tok", h).await,
        );
        assert_refused(
            "fetch_group_info room",
            h,
            client.fetch_group_info(14, "tok", h, "example.test").await,
        );
        assert_refused(
            "fetch_group_info domain",
            h,
            client.fetch_group_info(14, "tok", "room", h).await,
        );
    }

    /// The asset endpoints: the delete, the native download into a file,
    /// the in-memory download.
    async fn assert_asset_sites_refuse(client: &HttpClient, h: &str) {
        let sink = tempfile::tempdir().expect("tempdir");
        let sink = sink.path().join("asset.bin");
        assert_refused(
            "delete_asset domain",
            h,
            client.delete_asset(14, "tok", h, "key-1").await,
        );
        assert_refused(
            "delete_asset id",
            h,
            client.delete_asset(14, "tok", "wire.com", h).await,
        );
        assert_refused(
            "download_asset domain",
            h,
            client
                .download_asset(14, "tok", h, "key-1", None, &sink)
                .await,
        );
        assert_refused(
            "download_asset id",
            h,
            client
                .download_asset(14, "tok", "wire.com", h, None, &sink)
                .await,
        );
        assert_refused(
            "download_asset_bytes domain",
            h,
            client
                .download_asset_bytes(14, "tok", h, "key-1", None)
                .await,
        );
        assert_refused(
            "download_asset_bytes id",
            h,
            client
                .download_asset_bytes(14, "tok", "wire.com", h, None)
                .await,
        );
    }

    /// The account endpoints: the property key, clients, the rebind's
    /// query value, key packages, the 1:1 conversation, SSO.
    async fn assert_account_sites_refuse(client: &HttpClient, h: &str) {
        assert_refused(
            "fetch_property_value",
            h,
            client.fetch_property_value(14, "tok", h).await,
        );
        assert_refused("client_exists", h, client.client_exists(14, "tok", h).await);
        assert_refused(
            "update_client_mls_public_key",
            h,
            client
                .update_client_mls_public_key(14, "tok", h, "ed25519", "AA==")
                .await,
        );
        assert_refused(
            "delete_client",
            h,
            client.delete_client(14, "tok", h, None).await,
        );
        assert_refused(
            "update_client_label",
            h,
            client.update_client_label(14, "tok", h, "label").await,
        );
        assert_refused(
            "update_client_capabilities",
            h,
            client
                .update_client_capabilities(14, "tok", h, &["consumable-notifications"])
                .await,
        );
        assert_refused(
            "rebind_access_token client_id",
            h,
            client.rebind_access_token(14, "zuid", h).await,
        );
        assert_refused(
            "claim_key_packages domain",
            h,
            client
                .claim_key_packages(14, "tok", h, "user-uuid", 2, None)
                .await,
        );
        assert_refused(
            "claim_key_packages uuid",
            h,
            client
                .claim_key_packages(14, "tok", "example.test", h, 2, None)
                .await,
        );
        assert_refused(
            "upload_key_packages",
            h,
            client.upload_key_packages(14, "tok", h, &[vec![1u8]]).await,
        );
        assert_refused(
            "fetch_key_package_count",
            h,
            client.fetch_key_package_count(14, "tok", h, 2).await,
        );
        assert_refused(
            "fetch_one_to_one_conversation domain",
            h,
            client
                .fetch_one_to_one_conversation(14, "tok", h, "peer-uuid")
                .await,
        );
        assert_refused(
            "fetch_one_to_one_conversation uuid",
            h,
            client
                .fetch_one_to_one_conversation(14, "tok", "example.test", h)
                .await,
        );
        assert_refused(
            "sso_initiate_login_url",
            h,
            client.sso_initiate_login_url(h, "s", "e"),
        );
        assert_refused("sso_code_exists", h, client.sso_code_exists(h).await);
    }

    /// Step 137 (M5): every dynamic path parameter in `wire/http/` — and
    /// the `?client_id=` query value — goes through `path_segment`; a
    /// hostile id is refused before any request leaves. The Step 95c
    /// guard, generalised from its four sites to all of them; the 95c
    /// sites (the asset download, the property key) are in this table
    /// too, so it is the one place that lists every site. The backend's
    /// own id of `register_client` is checked on the response instead
    /// (`step_95c_hostile_backend`). The other query values — the
    /// notifications' `client` and `since`, the join link's `code` and
    /// `key`, the key packages' `ciphersuite`, the SSO redirects, the
    /// socket's `client` / `sync_marker` / `access_token` — go through
    /// `Url::query_pairs_mut().append_pair`, which percent-encodes
    /// `/ ? # & =`: nothing to guard. Only the rebind formats its value
    /// into the URL string, hence its row above.
    #[tokio::test]
    async fn hostile_ids_are_refused_at_every_path_site_before_any_request() {
        let server = MockServer::start();
        let any_request = server.mock(|when, then| {
            when.any_request();
            then.status(200)
                .header("content-type", "application/json")
                .body("{}");
        });
        let client = http(&server.base_url());
        for h in HOSTILE {
            assert_conversation_sites_refuse(&client, h).await;
            assert_asset_sites_refuse(&client, h).await;
            assert_account_sites_refuse(&client, h).await;
        }
        any_request.assert_calls(0);
    }

    /// Step 137 (L6): a non-2xx body is embedded in `BackendRejected`
    /// bounded to about 1 KiB and without control characters — the error
    /// crosses `UniFFI` and wasm into host logs, where an unbounded body
    /// with escape sequences is the backend's to write.
    #[tokio::test]
    async fn a_huge_error_body_is_capped_and_stripped_of_control_characters() {
        use httpmock::Method::PUT;
        let server = MockServer::start();
        let body = format!("\u{1b}[31m{}\n\u{7}", "A".repeat(1024 * 1024));
        server.mock(move |when, then| {
            when.method(PUT).path("/v14/clients/abc");
            then.status(500).body(body.clone());
        });
        let err = http(&server.base_url())
            .update_client_label(14, "tok", "abc", "label")
            .await
            .expect_err("500 must error");
        let KupferdrahtError::BackendRejected(message) = err else {
            panic!("expected BackendRejected, got {err:?}");
        };
        assert!(
            message.starts_with("update_client_label 500"),
            "the context and status stay: {message}"
        );
        assert!(
            message.len() <= 1024 + 64,
            "the detail is not capped: {} bytes",
            message.len()
        );
        assert!(
            !message.chars().any(char::is_control),
            "control characters survived into the error"
        );
    }
}
