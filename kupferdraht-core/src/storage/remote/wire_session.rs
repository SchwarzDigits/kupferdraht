// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! The Wire session a storage token (Step 111b) and a PIN token (Step 112)
//! are bought with: the login's token while a login is pending, the
//! client's own session once the client exists. A Wire token the exchange
//! refuses is renewed once through the client's session, as every other
//! Wire call does on a `401`.

use std::sync::Weak;

use zeroize::Zeroizing;

use super::token_exchange::{StorageToken, fetch_pin_token, fetch_storage_token};
use crate::error::KupferdrahtError;
use crate::wire::http::TokenRefresh;

/// The token exchange an account's storage tokens come from.
#[derive(Clone)]
pub(crate) struct TokenExchange {
    /// Its base URL, from the embedder's configuration.
    pub(crate) url: String,
    /// Plain HTTP permitted (dev proxies).
    pub(crate) permit_http: bool,
}

/// Where an account's storage gets its Wire access token.
pub(crate) enum WireSession {
    /// A login's token, until the client exists — it cannot be renewed.
    Login(Zeroizing<String>),
    /// The client's session, renewed on demand.
    Client(Weak<dyn TokenRefresh>),
}

impl WireSession {
    /// The current Wire token and when it was issued (`None` for a
    /// login's token).
    async fn token(&self) -> Result<(Zeroizing<String>, Option<i64>), KupferdrahtError> {
        match self {
            Self::Login(token) => Ok((token.clone(), None)),
            Self::Client(client) => {
                let client = client.upgrade().ok_or(KupferdrahtError::SessionExpired)?;
                let issued_at = client.access_token_issued_at_ms();
                // Not stale: the refresher hands out the current token.
                let token = client.refresh_and_get_token(i64::MIN).await?;
                Ok((Zeroizing::new(token), Some(issued_at)))
            }
        }
    }

    /// A Wire token issued after `issued_at`.
    async fn renewed(&self, issued_at: Option<i64>) -> Result<Zeroizing<String>, KupferdrahtError> {
        let (Self::Client(client), Some(issued_at)) = (self, issued_at) else {
            return Err(KupferdrahtError::Unauthorized);
        };
        let client = client.upgrade().ok_or(KupferdrahtError::SessionExpired)?;
        Ok(Zeroizing::new(
            client.refresh_and_get_token(issued_at).await?,
        ))
    }
}

/// Buy a storage token for `public_key` with the session's Wire token; one
/// the exchange refuses (`Unauthorized`) is renewed once through the
/// client's session.
///
/// # Errors
/// As [`fetch_storage_token`]; `Unauthorized` when a login's token is
/// refused; `SessionExpired` once the client is gone or its session is.
pub(crate) async fn buy_storage_token(
    exchange: &TokenExchange,
    session: &WireSession,
    public_key: &[u8; 32],
) -> Result<StorageToken, KupferdrahtError> {
    let (wire_token, issued_at) = session.token().await?;
    debug_assert!(!wire_token.is_empty(), "a session holds a Wire token");
    match fetch_storage_token(&exchange.url, exchange.permit_http, &wire_token, public_key).await {
        Err(KupferdrahtError::Unauthorized) => {
            let renewed = session.renewed(issued_at).await?;
            fetch_storage_token(&exchange.url, exchange.permit_http, &renewed, public_key).await
        }
        bought => bought,
    }
}

/// Buy a PIN token (Step 112) with the session's Wire token; one the
/// exchange refuses (`Unauthorized`) is renewed once through the client's
/// session.
///
/// # Errors
/// As [`fetch_pin_token`]; `Unauthorized` when a login's token is refused;
/// `SessionExpired` once the client is gone or its session is.
pub(crate) async fn buy_pin_token(
    exchange: &TokenExchange,
    session: &WireSession,
) -> Result<Zeroizing<String>, KupferdrahtError> {
    let (wire_token, issued_at) = session.token().await?;
    debug_assert!(!wire_token.is_empty(), "a session holds a Wire token");
    match fetch_pin_token(&exchange.url, exchange.permit_http, &wire_token).await {
        Err(KupferdrahtError::Unauthorized) => {
            let renewed = session.renewed(issued_at).await?;
            fetch_pin_token(&exchange.url, exchange.permit_http, &renewed).await
        }
        bought => bought,
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};

    use httpmock::Method::POST;
    use httpmock::MockServer;

    use super::*;

    const KEY: [u8; 32] = [7; 32];

    /// A client session: `expired` until renewed, then `fresh`.
    struct FakeClient {
        token: Mutex<String>,
        renewals: AtomicU32,
    }

    impl FakeClient {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                token: Mutex::new("expired".into()),
                renewals: AtomicU32::new(0),
            })
        }
    }

    impl TokenRefresh for FakeClient {
        fn access_token_issued_at_ms(&self) -> i64 {
            i64::from(self.renewals.load(Ordering::SeqCst))
        }

        fn refresh_and_get_token<'a>(
            &'a self,
            stale_issued_at: i64,
        ) -> Pin<Box<dyn Future<Output = Result<String, KupferdrahtError>> + Send + 'a>> {
            Box::pin(async move {
                if self.access_token_issued_at_ms() <= stale_issued_at {
                    self.renewals.fetch_add(1, Ordering::SeqCst);
                    *self.token.lock().expect("token") = "fresh".into();
                }
                Ok(self.token.lock().expect("token").clone())
            })
        }
    }

    fn exchange(server: &MockServer) -> TokenExchange {
        TokenExchange {
            url: server.base_url(),
            permit_http: true,
        }
    }

    fn answer_for(server: &MockServer, wire_token: &str, status: u16, body: &str) {
        server.mock(|when, then| {
            when.method(POST)
                .path("/v1/token")
                .header("authorization", format!("Bearer {wire_token}"));
            then.status(status)
                .header("content-type", "application/json")
                .body(body);
        });
    }

    #[tokio::test]
    async fn a_logins_token_buys_the_storage_token() {
        let server = MockServer::start();
        answer_for(&server, "login", 200, r#"{"token":"jwt","expiresIn":3600}"#);
        let session = WireSession::Login(Zeroizing::new("login".into()));

        let bought = buy_storage_token(&exchange(&server), &session, &KEY)
            .await
            .expect("bought");

        assert_eq!(bought.token.as_str(), "jwt");
    }

    /// The exchange refuses an expired Wire token: the client's session is
    /// renewed once, and the renewed token buys it.
    #[tokio::test]
    async fn a_refused_client_token_is_renewed_once() {
        let server = MockServer::start();
        answer_for(&server, "expired", 401, "{}");
        answer_for(&server, "fresh", 200, r#"{"token":"jwt","expiresIn":3600}"#);
        let client = FakeClient::new();
        let weak: Weak<dyn TokenRefresh> = Arc::downgrade(&client) as _;

        let bought = buy_storage_token(&exchange(&server), &WireSession::Client(weak), &KEY)
            .await
            .expect("bought");

        assert_eq!(bought.token.as_str(), "jwt");
        assert_eq!(client.renewals.load(Ordering::SeqCst), 1, "renewed once");
    }

    /// A login's token cannot be renewed: its refusal stands.
    #[tokio::test]
    async fn a_refused_login_token_is_unauthorized() {
        let server = MockServer::start();
        answer_for(&server, "login", 401, "{}");
        let session = WireSession::Login(Zeroizing::new("login".into()));

        let result = buy_storage_token(&exchange(&server), &session, &KEY).await;

        assert!(
            matches!(result, Err(KupferdrahtError::Unauthorized)),
            "{:?}",
            result.err()
        );
    }

    /// Step 112b: the PIN token is bought the same way — a refused client
    /// token is renewed once.
    #[tokio::test]
    async fn a_refused_client_token_buys_the_pin_token_after_one_renewal() {
        let server = MockServer::start();
        answer_for(&server, "expired", 401, "{}");
        answer_for(
            &server,
            "fresh",
            200,
            r#"{"token":"pin.jwt","expiresIn":600}"#,
        );
        let client = FakeClient::new();
        let weak: Weak<dyn TokenRefresh> = Arc::downgrade(&client) as _;

        let bought = buy_pin_token(&exchange(&server), &WireSession::Client(weak))
            .await
            .expect("bought");

        assert_eq!(bought.as_str(), "pin.jwt");
        assert_eq!(client.renewals.load(Ordering::SeqCst), 1, "renewed once");
    }

    /// The client is gone (logged out, dropped): its session with it.
    #[tokio::test]
    async fn a_dropped_client_has_no_session() {
        let server = MockServer::start();
        let weak: Weak<dyn TokenRefresh> = Arc::downgrade(&FakeClient::new()) as _;

        let result = buy_storage_token(&exchange(&server), &WireSession::Client(weak), &KEY).await;

        assert!(
            matches!(result, Err(KupferdrahtError::SessionExpired)),
            "{:?}",
            result.err()
        );
    }
}
