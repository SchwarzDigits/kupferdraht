// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `PendingClient` — Step 16c.
//!
//! Returned by [`super::ClientBuilder::register`] when a fresh account
//! has been created on Wire's side but not yet activated. The caller
//! drives [`PendingClient::activate`] with the email-delivered code to
//! transition into a usable [`super::Client`]; if the code never
//! arrives or expires, [`PendingClient::resend_activation_email`]
//! re-issues it.
//!
//! `PendingClient` is **in-memory only** — a consumer that crashes
//! between `register` and `activate` has to start over with a fresh
//! `register` (see Plan-Doc §16c Open Question 3).
//!
//! The held password is wrapped in `Zeroizing<String>` per
//! `CLAUDE.md` §Sensitive Material — when the `PendingClient` is
//! dropped, the memory is overwritten.
//!
//! `PendingClient` is the post-register intermediate state between
//! "Wire-side account created" and "usable Client" (the
//! email-activation gap); `activate` closes it.

use std::sync::Arc;

use zeroize::Zeroizing;

use crate::client::Client;
use crate::client::builder::{ClientBuilder, ServerConfig};
use crate::error::KupferdrahtError;
use crate::keystore::SecureKeyStore;
use crate::storage::StorageProvider;
use crate::wire::HttpClient;

/// Post-register, pre-activation handle. Returned by
/// [`ClientBuilder::register`]; consumed by
/// [`PendingClient::activate`].
///
/// Cheap to clone (the inner state is held behind an `Arc`).
pub struct PendingClient {
    user_id: String,
    email: String,
    password: Zeroizing<String>,

    // Bits we need to run the post-activate login pipeline. Same
    // shape `ClientBuilder` carries.
    server: ServerConfig,
    base_dir: Option<std::path::PathBuf>,
    permit_http: bool,
    storage_provider: Option<Arc<dyn StorageProvider>>,
    keystore: Arc<dyn SecureKeyStore>,
    /// HTTP client pre-built against `server.api_base_url`; reused
    /// by `activate` (POST /activate) and `resend_activation_email`
    /// (POST /activate/send).
    http: HttpClient,
}

impl PendingClient {
    /// The new account's Wire user id (UUID from the `qualified_id`
    /// in the `POST /register` response).
    #[must_use]
    pub fn user_id(&self) -> &str {
        &self.user_id
    }

    /// The email address the account was registered under. Echoes
    /// the caller's input verbatim.
    #[must_use]
    pub fn email(&self) -> &str {
        &self.email
    }

    /// Redeem the email-delivered activation code and transition
    /// into a fully usable [`Client`].
    ///
    /// Internally:
    /// 1. `POST /activate` with `{ code, email, dryrun: false }`.
    /// 2. The same login pipeline `ClientBuilder::login` runs —
    ///    authenticate, register the device, rebind the access token,
    ///    upload MLS key packages — and returns a ready `Client`.
    ///
    /// On a wrong / expired code (404 invalid-code) the function
    /// returns a `BackendRejected` and `Arc<PendingClient>` remains
    /// usable for another attempt via a fresh `Arc`-clone.
    ///
    /// # Errors
    /// * `BackendRejected` for non-2xx from `POST /activate` (404
    ///   invalid-code, 400 invalid-email, …) — the Wire label is
    ///   preserved in the error message.
    /// * Whatever the login pipeline surfaces if the post-activate
    ///   `POST /login` fails (should not happen on a fresh
    ///   activation but propagated honestly).
    /// * `Network` / `Encoding` for transport / decode failures.
    pub async fn activate(self: Arc<Self>, code: &str) -> Result<Arc<Client>, KupferdrahtError> {
        crate::client::input::require_non_empty("activate", &[("code", code)])?;
        self.http.activate(&self.email, code).await?;
        drive_post_activate_login(&self).await
    }

    /// Request Wire to re-send the activation email. Useful when
    /// the original code expired or never arrived.
    ///
    /// # Errors
    /// * `BackendRejected` for non-2xx from `POST /activate/send`
    ///   (e.g. `409 key-exists` once the account is already active,
    ///   `403 blacklisted-email`).
    /// * `Network` for transport failures.
    pub async fn resend_activation_email(&self) -> Result<(), KupferdrahtError> {
        self.http.request_activation_resend(&self.email).await
    }
}

impl std::fmt::Debug for PendingClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Deliberately omit `password` — `Debug` is a logging /
        // assertion-failure surface and must never leak credentials.
        f.debug_struct("PendingClient")
            .field("user_id", &self.user_id)
            .field("email", &self.email)
            .finish_non_exhaustive()
    }
}

/// Crate-internal constructor used by `ClientBuilder::register`. Keeps
/// `PendingClient`'s fields private to this module so callers can't
/// forge a half-constructed pending state.
///
/// The argument count mirrors `PendingClient`'s field count — bundling
/// them into a sub-struct would just rename the noise, not remove it.
#[allow(clippy::too_many_arguments)]
pub(super) fn new_pending(
    user_id: String,
    email: String,
    password: Zeroizing<String>,
    server: ServerConfig,
    base_dir: Option<std::path::PathBuf>,
    permit_http: bool,
    storage_provider: Option<Arc<dyn StorageProvider>>,
    keystore: Arc<dyn SecureKeyStore>,
    http: HttpClient,
) -> PendingClient {
    PendingClient {
        user_id,
        email,
        password,
        server,
        base_dir,
        permit_http,
        storage_provider,
        keystore,
        http,
    }
}

/// Run the post-activate login pipeline. Hosts the field access so
/// `PendingClient` can keep its fields private. Returns the unlocked
/// `Client` per Plan-Doc §16c.
///
/// Crate-internal; called only from `PendingClient::activate`.
pub(super) async fn drive_post_activate_login(
    pending: &PendingClient,
) -> Result<Arc<Client>, KupferdrahtError> {
    let mut builder = ClientBuilder::new()
        .server(pending.server.clone())
        .keystore(pending.keystore.clone());
    if pending.permit_http {
        builder = builder.permit_http();
    }
    if let Some(path) = pending.base_dir.clone() {
        builder = builder.base_dir(path);
    }
    if let Some(provider) = pending.storage_provider.clone() {
        builder = builder.storage_provider(provider);
    }
    builder
        .login(pending.email.clone(), pending.password.to_string(), None)
        .await?
        .register_device()
        .await
}
