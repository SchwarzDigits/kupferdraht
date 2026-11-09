// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 139 (ADR-0036 revision 1.1) — service tokens on the browser
//! surface: `Client.serviceToken(audience)` hands the page a token of the
//! token exchange for an audience the embedder names, for its own services
//! beside the storage server and the PIN service. The answer crosses as the
//! exchange gave it — `{ token, expiresIn }`, the seconds left — and
//! Kupferdraht neither parses nor checks the JWT. The server mode only: the
//! local mode has no exchange.

use std::sync::{Arc, Weak};

use serde::Serialize;
use wasm_bindgen::JsValue;

use crate::client::Client;
use crate::error::KupferdrahtError;
use crate::storage::remote::service_tokens::ServiceTokenCache;
use crate::storage::remote::wire_session::{TokenExchange, WireSession};
use crate::wire::http::TokenRefresh;

/// Plain, JS-serializable view of a service token — `{ token, expiresIn }`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceTokenView {
    pub token: String,
    /// Seconds until the token expires.
    pub expires_in: u64,
}

/// The token exchange of the server mode, as service tokens are bought from
/// it.
pub struct ServiceTokenExchange(pub(crate) TokenExchange);

/// The exchange service tokens come from.
///
/// # Errors
/// `InvalidInput` in the local mode — no token exchange is configured.
pub fn exchange() -> Result<ServiceTokenExchange, KupferdrahtError> {
    crate::storage::service_token_exchange().map(ServiceTokenExchange)
}

/// `Client.serviceToken`: a token for `audience` from `cache` — served while
/// within half its lifetime, else bought from the exchange with the
/// client's Wire session (`SessionExpired` once the client is gone).
pub(super) async fn service_token(
    client: &Arc<Client>,
    cache: &ServiceTokenCache,
    audience: &str,
) -> Result<JsValue, JsValue> {
    let exchange = exchange().map_err(|e| super::manager::js_error(&e))?;
    let refresher: Weak<dyn TokenRefresh> = Arc::downgrade(client) as Weak<Client>;
    let session = WireSession::Client(refresher);
    let token = cache
        .service_token(&exchange.0, &session, audience, crate::time::now_ms())
        .await
        .map_err(|e| super::manager::js_error(&e))?;
    let view = ServiceTokenView {
        token: token.token.as_str().to_owned(),
        expires_in: token.expires_in.as_secs(),
    };
    debug_assert!(!view.token.is_empty(), "the exchange answers with a token");
    super::js_view::to_js(&view)
}
