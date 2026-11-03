// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Service tokens for the embedder's own services (Step 139, ADR-0036
//! revision 1.1): a token of an audience the embedder names, bought from the
//! token exchange with the account's Wire session and handed to the page.
//! One token per audience is kept, served until half its lifetime like the
//! storage token (`token_cache`), bought anew after that or after a refusal
//! — the exchange's per-user limits are shared with the storage token's
//! renewal, so page code must not be able to spend them in a loop.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use zeroize::Zeroizing;

use super::token_exchange::ExchangedToken;
use super::wire_session::{TokenExchange, WireSession, buy_token};
use crate::error::KupferdrahtError;

/// A token of one audience: when it expires and until when it is served
/// (ms since the epoch).
struct CachedToken {
    token: Zeroizing<String>,
    expires_at_ms: i64,
    serve_until_ms: i64,
}

/// One token per audience.
#[derive(Default)]
pub(crate) struct ServiceTokenCache {
    tokens: Mutex<HashMap<String, CachedToken>>,
}

impl ServiceTokenCache {
    /// A token for `audience` with its remaining lifetime: the cached one
    /// while it is within half its lifetime at `now_ms`, else one bought
    /// with `session`. A refusal leaves nothing cached for the audience.
    ///
    /// # Errors
    /// As [`buy_token`].
    pub(crate) async fn service_token(
        &self,
        exchange: &TokenExchange,
        session: &WireSession,
        audience: &str,
        now_ms: i64,
    ) -> Result<ExchangedToken, KupferdrahtError> {
        if let Some(served) = self.served(audience, now_ms) {
            return Ok(served);
        }
        // Forgotten before the purchase: a refusal leaves nothing stale
        // behind that the next call would serve.
        self.forget(audience);
        let bought = buy_token(exchange, session, audience).await?;
        self.remember(audience, &bought, now_ms);
        Ok(bought)
    }

    /// The token of `audience` with the lifetime it has left at `now_ms`,
    /// while it is still served.
    fn served(&self, audience: &str, now_ms: i64) -> Option<ExchangedToken> {
        let tokens = self.lock();
        let cached = tokens
            .get(audience)
            .filter(|cached| now_ms < cached.serve_until_ms)?;
        Some(ExchangedToken {
            token: cached.token.clone(),
            expires_in: remaining(cached.expires_at_ms, now_ms),
        })
    }

    /// Drop the token of `audience`, if any.
    fn forget(&self, audience: &str) {
        self.lock().remove(audience);
    }

    /// Keep `token`, bought at `now_ms` for `audience`, until half its
    /// lifetime.
    fn remember(&self, audience: &str, token: &ExchangedToken, now_ms: i64) {
        let cached = CachedToken {
            token: token.token.clone(),
            expires_at_ms: now_ms.saturating_add(millis(token.expires_in)),
            serve_until_ms: now_ms.saturating_add(millis(token.renew_after())),
        };
        self.lock().insert(audience.to_owned(), cached);
        debug_assert!(
            self.lock()
                .get(audience)
                .is_some_and(|cached| cached.serve_until_ms <= cached.expires_at_ms),
            "the bought token is cached and served no longer than it lives"
        );
    }

    /// The cache, whole even after a panic under the lock.
    fn lock(&self) -> MutexGuard<'_, HashMap<String, CachedToken>> {
        self.tokens
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// The whole seconds from `now_ms` to `expires_at_ms`; none once it passed.
fn remaining(expires_at_ms: i64, now_ms: i64) -> Duration {
    let left_ms = u64::try_from(expires_at_ms.saturating_sub(now_ms)).unwrap_or(0);
    Duration::from_secs(left_ms / 1000)
}

/// `duration` in whole milliseconds, saturating.
fn millis(duration: Duration) -> i64 {
    i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use std::time::Duration;

    use httpmock::Method::POST;
    use httpmock::{Mock, MockServer};

    use super::*;

    fn exchange(server: &MockServer) -> TokenExchange {
        TokenExchange {
            url: server.base_url(),
            permit_http: true,
        }
    }

    fn login() -> WireSession {
        WireSession::Login(Zeroizing::new("login".into()))
    }

    /// The exchange issues `token` for `audience`, good for `expires_in` seconds.
    fn issue<'a>(server: &'a MockServer, audience: &str, token: &str, expires_in: u64) -> Mock<'a> {
        server.mock(|when, then| {
            when.method(POST)
                .path("/v1/token")
                .json_body(serde_json::json!({ "audience": audience }));
            then.status(200)
                .header("content-type", "application/json")
                .body(format!(r#"{{"token":"{token}","expiresIn":{expires_in}}}"#));
        })
    }

    #[tokio::test]
    async fn a_second_call_within_half_the_lifetime_serves_the_cached_token() {
        let server = MockServer::start();
        let issued = issue(&server, "cases", "cases.jwt", 600);
        let cache = ServiceTokenCache::default();

        let first = cache
            .service_token(&exchange(&server), &login(), "cases", 0)
            .await
            .expect("bought");
        let second = cache
            .service_token(&exchange(&server), &login(), "cases", 299_000)
            .await
            .expect("served");

        assert_eq!(issued.calls(), 1, "one request for two calls");
        assert_eq!(first.token.as_str(), "cases.jwt");
        assert_eq!(second.token.as_str(), "cases.jwt");
        assert_eq!(first.expires_in, Duration::from_secs(600));
        assert_eq!(
            second.expires_in,
            Duration::from_secs(301),
            "the lifetime left, not the issued one"
        );
    }

    #[tokio::test]
    async fn a_call_after_half_the_lifetime_buys_anew() {
        let server = MockServer::start();
        let issued = issue(&server, "cases", "cases.jwt", 600);
        let cache = ServiceTokenCache::default();

        cache
            .service_token(&exchange(&server), &login(), "cases", 0)
            .await
            .expect("bought");
        cache
            .service_token(&exchange(&server), &login(), "cases", 300_000)
            .await
            .expect("bought anew");

        assert_eq!(issued.calls(), 2, "half the lifetime passed");
    }

    #[tokio::test]
    async fn each_audience_has_a_token_of_its_own() {
        let server = MockServer::start();
        let cases = issue(&server, "cases", "cases.jwt", 600);
        let files = issue(&server, "files", "files.jwt", 600);
        let cache = ServiceTokenCache::default();

        let first = cache
            .service_token(&exchange(&server), &login(), "cases", 0)
            .await
            .expect("cases");
        let other = cache
            .service_token(&exchange(&server), &login(), "files", 0)
            .await
            .expect("files");
        let again = cache
            .service_token(&exchange(&server), &login(), "cases", 1)
            .await
            .expect("cases again");

        assert_eq!(cases.calls(), 1);
        assert_eq!(files.calls(), 1);
        assert_eq!(first.token.as_str(), "cases.jwt");
        assert_eq!(other.token.as_str(), "files.jwt");
        assert_eq!(again.token.as_str(), "cases.jwt");
    }

    /// A purchase the exchange refuses is reported; the token of before,
    /// still valid for its other half, is not served in its place.
    #[tokio::test]
    async fn a_refused_purchase_serves_no_stale_token() {
        let server = MockServer::start();
        let mut issued = issue(&server, "cases", "cases.jwt", 600);
        let cache = ServiceTokenCache::default();
        cache
            .service_token(&exchange(&server), &login(), "cases", 0)
            .await
            .expect("bought");
        issued.delete();
        let refused = server.mock(|when, then| {
            when.method(POST).path("/v1/token");
            then.status(403)
                .header("content-type", "application/json")
                .body(r#"{"error":"not_allowed"}"#);
        });

        let first = cache
            .service_token(&exchange(&server), &login(), "cases", 300_000)
            .await;
        let second = cache
            .service_token(&exchange(&server), &login(), "cases", 300_001)
            .await;

        assert!(
            matches!(first, Err(KupferdrahtError::StorageAccessDenied(_))),
            "{:?}",
            first.err()
        );
        assert!(
            matches!(second, Err(KupferdrahtError::StorageAccessDenied(_))),
            "{:?}",
            second.err()
        );
        assert_eq!(refused.calls(), 2, "nothing stale stands in for a refusal");
    }
}
