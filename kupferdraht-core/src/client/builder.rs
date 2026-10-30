// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `ClientBuilder` — the entry point for constructing a `Client`.
//!
//! `ClientBuilder` carries the per-session config (Wire backend,
//! storage path, attached `SqliteStore`, http-permit opt-in) and
//! exposes two terminal methods:
//!
//! - [`ClientBuilder::login`] runs phase 1 of the Wire login pipeline
//!   ([`super::login_pipeline::LoginPipeline`]) — authentication + storage
//!   setup — and returns a [`super::PendingLogin`]; its
//!   `register_device` runs phase 2 and yields a fresh `Client`.
//! - [`ClientBuilder::resume`] reconstructs a `Client` from a
//!   persisted `SqliteStore`, no Wire round-trip required. The
//!   per-step helpers live in [`super::resume`].
//!
//! `ServerConfig` is also defined here because it's the only thing
//! the caller passes in before deciding which terminal to invoke.
//! `Client` itself lives in [`super::client`].

use std::sync::Arc;

use zeroize::Zeroizing;

use crate::client::Client;
use crate::client::input::require_non_empty;
use crate::client::login_pipeline::LoginPipeline;
use crate::client::pending::{PendingClient, new_pending};
use crate::client::pending_login::{PendingLogin, new_pending_login};
use crate::client::resume::{
    assemble_resumed_client, build_server_from_record, decode_persisted_mls_public_key,
    load_resume_state, refresh_stale_access_token, resume_mls_state,
};
use crate::error::KupferdrahtError;
#[cfg(target_arch = "wasm32")]
use crate::keystore::IndexedDbKeyStore;
#[cfg(not(target_arch = "wasm32"))]
use crate::keystore::{FileKeyStore, KeyringStore};
use crate::keystore::{InMemoryKeyStore, SecureKeyStore, key_id};
use crate::storage::account_keys::AccountKeySource;
use crate::storage::db_key::DbKey;
use crate::storage::{
    KeyLock, KupferdrahtHome, SqliteStorageProvider, StorageProvider, Store,
    ensure_account_keys_durable, ephemeral_cookie_label, load_or_create_key_locked,
};
use crate::wire::HttpClient;
use crate::wire::dto::{LoginOutcome, NewUserRequest, SelfUser};

/// Wire-server addressing tuple supplied to `ClientBuilder`.
///
/// **No direct v14-Swagger counterpart.** Wire's endpoint discovery
/// is out-of-band: clients fetch a deployment-specific backend
/// config JSON at bootstrap and pass the values into `ServerConfig`.
/// The v14 API itself does not expose a schema for "where do I
/// reach this Wire backend?" (closest swagger-side neighbours —
/// `BackendConfig`, `DomainRedirectConfig` — describe a different
/// concern and don't carry these three fields).
///
/// **Field selection — exactly three are sufficient:**
/// - `api_base_url` — prefixed to every HTTP request in the
///   `wire/http` modules (login, register, MLS, conversations, assets…).
/// - `websocket_url` — host where `GET /v14/events` is served;
///   Wire deploys it on a different host than the REST API, so
///   one URL cannot stand in for both.
/// - `title` — human-readable backend label (e.g. for an
///   account-picker UI); never sent on the wire.
///
/// Kupferdraht-specific because Wire pairs an HTTPS API host with a
/// separate WebSocket host (the matrix-rust-sdk `homeserver_url`
/// model is a single URL). See `docs/api-mapping-matrix-rust-sdk.md`.
#[derive(Debug, Clone, uniffi::Record)]
pub struct ServerConfig {
    pub api_base_url: String,
    pub websocket_url: String,
    pub title: String,
}

#[derive(Default)]
pub struct ClientBuilder {
    server: Option<ServerConfig>,
    base_dir: Option<std::path::PathBuf>,
    permit_http: bool,
    /// Optional override of the app-data + registry backend. When unset,
    /// persistence uses the shipped default `SQLite` provider rooted at
    /// `base_dir`. A cloud / bot backend injects its own here.
    storage_provider: Option<Arc<dyn StorageProvider>>,
    /// Per-platform `SecureKeyStore`. Defaults to
    /// `KeyringStore::new("kupferdraht")` on desktop when unset; tests inject
    /// `InMemoryKeyStore`.
    keystore: Option<Arc<dyn SecureKeyStore>>,
    /// The account to [`resume`](ClientBuilder::resume). Required by
    /// `resume`, which errors when this is unset; registry-based selection
    /// lives in `Kupferdraht::resume`, not here.
    account_id: Option<String>,
    /// Human-readable device name shown in Wire's device list (Step 41),
    /// supplied by the embedder (e.g. the OS device name). `None` → a
    /// platform-derived default.
    device_label: Option<String>,
    /// Where a new key of an account comes from (ADR-0035): `Seed` in the
    /// browser, set by the device manager; `Random` otherwise.
    account_keys: AccountKeySource,
}

impl ClientBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn server(mut self, config: ServerConfig) -> Self {
        self.server = Some(config);
        self
    }

    /// The persistence root. When set, `login` / `resume` open (and
    /// create as needed) the per-account `base_dir/accounts/<id>/kupferdraht.db`
    /// (via the default provider) and `mls.db` under the same directory.
    /// When unset, persistence is in-memory only (tests / single-shot CLI).
    #[must_use]
    pub fn base_dir(mut self, base_dir: std::path::PathBuf) -> Self {
        self.base_dir = Some(base_dir);
        self
    }

    /// Where a new key of an account comes from (ADR-0035). The device
    /// manager sets it from its home; the browser's is `Seed`.
    #[must_use]
    pub(crate) fn account_keys(mut self, source: AccountKeySource) -> Self {
        self.account_keys = source;
        self
    }

    /// Override the app-data + registry backend with a custom
    /// [`StorageProvider`] (e.g. a server-side backend for cloud bots).
    /// When unset, the SDK uses the default `SQLite` provider rooted at
    /// [`base_dir`](Self::base_dir). `base_dir` is still required for
    /// `mls.db`, which is not yet pluggable.
    #[must_use]
    pub fn storage_provider(mut self, provider: Arc<dyn StorageProvider>) -> Self {
        self.storage_provider = Some(provider);
        self
    }

    /// Opt in to plain-HTTP connections.
    ///
    /// By default `login` rejects any `ServerConfig` whose
    /// `api_base_url` is not `https://`. Integration tests that run
    /// against a local `httpmock::MockServer` (plain HTTP) call this
    /// method to bypass that check. Production callers must not.
    ///
    /// The opt-in is deliberately explicit and not overridable via
    /// environment variable or config file.
    #[must_use]
    pub fn permit_http(mut self) -> Self {
        self.permit_http = true;
        self
    }

    /// Attach the [`SecureKeyStore`] the default store fetches its
    /// per-account database key from. When unset, falls back to a
    /// `KeyringStore::new("kupferdraht")` — the desktop default. Mobile / Web
    /// embedders supply their own implementation via `UniFFI`;
    /// integration tests inject `InMemoryKeyStore` here.
    #[must_use]
    pub fn keystore(mut self, keystore: Arc<dyn SecureKeyStore>) -> Self {
        self.keystore = Some(keystore);
        self
    }

    /// Select which account [`resume`](Self::resume) reconstructs.
    /// `resume` requires this to be set and errors otherwise; for
    /// registry-based selection use `Kupferdraht::resume` instead.
    #[must_use]
    pub fn account(mut self, account_id: impl Into<String>) -> Self {
        self.account_id = Some(account_id.into());
        self
    }

    /// Human-readable device name shown in Wire's device list (Step 41), e.g.
    /// the OS device name. When unset, a platform default ("Kupferdraht (iOS)", …)
    /// is used.
    #[must_use]
    pub fn device_label(mut self, label: impl Into<String>) -> Self {
        self.device_label = Some(label.into());
        self
    }

    /// SSO — validate the SSO `code` resolves to a configured `IdP`
    /// (`HEAD /sso/initiate-login/{idp}`) and return the interactive
    /// initiate URL the consumer opens in a browser
    /// (`GET /sso/initiate-login/{idp}?success_redirect&error_redirect`).
    /// Step 17β; the `zuid` cookie from the post-`IdP` redirect feeds the
    /// finalize path (`ClientBuilder::finalize_sso`, 17b).
    ///
    /// Wire's user-facing code is `wire-<idp-uuid>` but the SP endpoint wants
    /// the bare `IdP` UUID, so a `wire-` prefix is stripped (see [`idp_id`]).
    ///
    /// # Errors
    /// * `InvalidInput` — `server` unset, or the code is unknown (404).
    /// * `BackendRejected` / `Network` — from the code-existence probe.
    pub async fn initiate_sso(
        self,
        code: &str,
        success_redirect: &str,
        error_redirect: &str,
    ) -> Result<String, KupferdrahtError> {
        let server = self.server.ok_or_else(|| {
            KupferdrahtError::InvalidInput("ClientBuilder::server was not called".into())
        })?;
        let idp = idp_id(code);
        require_non_empty("initiate_sso", &[("code", idp)])?;
        let http = HttpClient::new(&server.api_base_url, self.permit_http)?;
        if !http.sso_code_exists(idp).await? {
            return Err(KupferdrahtError::InvalidInput(format!(
                "SSO code not recognized: {code}"
            )));
        }
        http.sso_initiate_login_url(idp, success_redirect, error_redirect)
    }

    /// SSO email discovery (Step 17β) — resolve the backend's **default** SSO
    /// code (`GET /sso/settings`) and return the interactive initiate URL.
    /// Wire v14 exposes no per-email-domain discovery, so the team default is
    /// used for every `email`; multi-IdP backends must call [`initiate_sso`]
    /// with an explicit code. `email` is the account identifier but does not
    /// select the `IdP` under v14.
    ///
    /// # Errors
    /// * `InvalidInput` — `server` unset, empty `email`, or no default SSO
    ///   code configured on the backend.
    /// * `BackendRejected` / `Network` — from `GET /sso/settings`.
    pub async fn initiate_sso_by_email(
        self,
        email: &str,
        success_redirect: &str,
        error_redirect: &str,
    ) -> Result<String, KupferdrahtError> {
        if email.is_empty() {
            return Err(KupferdrahtError::InvalidInput(
                "email must be non-empty".into(),
            ));
        }
        let server = self.server.ok_or_else(|| {
            KupferdrahtError::InvalidInput("ClientBuilder::server was not called".into())
        })?;
        let http = HttpClient::new(&server.api_base_url, self.permit_http)?;
        let code = http.sso_default_code().await?.ok_or_else(|| {
            KupferdrahtError::InvalidInput("no SSO provider is configured for this backend".into())
        })?;
        http.sso_initiate_login_url(idp_id(&code), success_redirect, error_redirect)
    }

    /// Phase 1 of login: authenticate with email/password (optional 2FA),
    /// open the per-account store, and return a [`PendingLogin`] that holds
    /// the captured session. Call [`PendingLogin::register_device`] to run
    /// phase 2 (`POST /clients` + MLS identity) and obtain a ready `Client`.
    ///
    /// Splitting the two lets a consumer recover from Wire's per-user device
    /// cap without re-authenticating (see [`PendingLogin`]). Authentication
    /// failures (wrong password, missing 2FA) surface here; the device cap
    /// surfaces from `register_device`.
    ///
    /// The per-account store is opened (ready, self-keyed) by the
    /// `StorageProvider` during phase 1 — there is no separate unlock step.
    ///
    /// # Errors
    /// See `KupferdrahtError` (authentication / storage failures).
    pub async fn login(
        self,
        email: String,
        password: String,
        second_factor_code: Option<String>,
    ) -> Result<Arc<PendingLogin>, KupferdrahtError> {
        require_non_empty("login", &[("email", &email), ("password", &password)])?;
        let server = self.server.ok_or_else(|| {
            KupferdrahtError::InvalidInput("ClientBuilder::server was not called".into())
        })?;
        // Step 93 (ADR-0025): the home is resolved first — its id (created
        // under the home lock on a first open) is what the cookie label and
        // every keystore id carry. Wire keeps one refresh cookie per
        // `(user, label)`; one label per home keeps two homes of one user
        // from invalidating each other's `zuid` (Step 41's per-install
        // label was per keystore, observations §83).
        let home = open_home(self.base_dir.clone(), self.account_keys)?;
        let keystore = resolve_keystore(self.keystore.clone(), self.permit_http).await?;
        let cookie_label = home_cookie_label(home.as_ref());
        let device_label = self.device_label.clone();
        // Dedicated transport for the post-auth device-list / device-delete
        // calls `PendingLogin` exposes (`HttpClient` is not `Clone`, so it
        // can't be borrowed out of the consume-on-success pipeline state).
        let device_ops_http = HttpClient::new(&server.api_base_url, self.permit_http)?;
        // The per-account store directory (`base/accounts/<id>/`) is not
        // known until `authenticate` returns the account id, so the
        // pipeline starts storage-less; the provider opens the (ready,
        // self-keyed) per-account store below and it is bound onto it.
        let pipeline = LoginPipeline::start(
            server,
            None,
            self.permit_http,
            None,
            cookie_label,
            device_label,
        )
        .await?;
        let (login_outcome, self_user) = pipeline
            .authenticate(&email, &password, second_factor_code.as_deref())
            .await?;
        assemble_pending_login(
            self.storage_provider,
            home,
            &keystore,
            pipeline,
            login_outcome,
            self_user,
            email,
            Some(Zeroizing::new(password)),
            device_ops_http,
        )
        .await
    }

    /// SSO finalize (Step 17β) — complete login from the `zuid` cookie handed
    /// back by Wire's `/sso/finalize-login` redirect. Mints the first access
    /// token (no password, no `client_id`), fetches the self-profile, opens
    /// the per-account store, and returns a [`PendingLogin`] whose
    /// `register_device` runs phase 2 exactly as the password flow does. The
    /// account email is taken from the fetched profile.
    ///
    /// # Errors
    /// * `InvalidInput` — `server` was not set.
    /// * `SessionExpired` — the `zuid` cookie is already revoked / rotated.
    /// * Other `KupferdrahtError` — network / storage failures.
    pub async fn finalize_sso(
        self,
        sso_cookie: &str,
    ) -> Result<Arc<PendingLogin>, KupferdrahtError> {
        // wasm: the browser jar holds the cookie, the argument may be empty.
        #[cfg(not(target_arch = "wasm32"))]
        require_non_empty("finalize_sso", &[("sso_cookie", sso_cookie)])?;
        let server = self.server.ok_or_else(|| {
            KupferdrahtError::InvalidInput("ClientBuilder::server was not called".into())
        })?;
        let home = open_home(self.base_dir.clone(), self.account_keys)?;
        let keystore = resolve_keystore(self.keystore.clone(), self.permit_http).await?;
        let cookie_label = home_cookie_label(home.as_ref());
        let device_label = self.device_label.clone();
        let device_ops_http = HttpClient::new(&server.api_base_url, self.permit_http)?;
        let pipeline = LoginPipeline::start(
            server,
            None,
            self.permit_http,
            None,
            cookie_label,
            device_label,
        )
        .await?;
        let (login_outcome, self_user) = pipeline.finalize_sso(sso_cookie).await?;
        let email = self_user.email.clone().unwrap_or_default();
        assemble_pending_login(
            self.storage_provider,
            home,
            &keystore,
            pipeline,
            login_outcome,
            self_user,
            email,
            None,
            device_ops_http,
        )
        .await
    }

    /// Reconstruct a ready `Client` from a previously-persisted session
    /// without re-entering the password. Reads every artefact from the
    /// per-account store, refreshes the access token via
    /// `POST /v{N}/access?client_id=<id>` using the persisted `zuid`
    /// refresh-cookie, and returns an `Arc<Client>` just like `login`.
    ///
    /// Requires [`Self::base_dir`] (or a [`Self::storage_provider`]);
    /// `server_config` comes from the store.
    ///
    /// # Errors
    /// * `KupferdrahtError::InvalidInput` if no storage config is set, if the
    ///   store has no persisted session / device / `self_user` /
    ///   `server_config` / removal key, or if `device.mls_public_key`
    ///   is missing or not base64.
    /// * `KupferdrahtError::Network` / `BackendRejected` from the
    ///   `/access` refresh.
    pub async fn resume(self) -> Result<Arc<Client>, KupferdrahtError> {
        let Self {
            base_dir,
            permit_http,
            storage_provider,
            keystore,
            account_id: requested_account,
            account_keys,
            ..
        } = self;
        let keystore = resolve_keystore(keystore, permit_http).await?;
        // Step 93: the home (and its id) is resolved once, up front.
        let home = open_home(base_dir, account_keys)?;
        // The account must be named explicitly (`account(id)`); open its
        // READY per-account store via the provider, and read tokens /
        // removal key / `self_user.email` from it. mls.db is opened with
        // its own separate key (`kupferdraht.<home-id>.account-<id>-mls`).
        let account_id = requested_account.ok_or_else(|| {
            KupferdrahtError::InvalidInput(
                "ClientBuilder::resume requires an explicit account — set .account(id), \
                 or use Kupferdraht::resume for registry-based selection"
                    .into(),
            )
        })?;
        // Step 104: a key created here exists durably before its database.
        if let Some(home) = home.as_ref() {
            // Step 111a: the browser registers each database's VFS first.
            #[cfg(target_arch = "wasm32")]
            crate::storage::prepare_account_databases(home, &account_id).await?;
            ensure_account_keys_durable(home, &keystore, &account_id, storage_provider.is_none())
                .await?;
        }
        let provider = resolve_provider(storage_provider, home.as_ref(), &keystore);
        let store = provider
            .as_ref()
            .map(|p| p.open_store(&account_id))
            .transpose()?
            .ok_or_else(|| {
                KupferdrahtError::InvalidInput(
                    "ClientBuilder::resume requires base_dir(..) or storage_provider(..)".into(),
                )
            })?;
        // `mut` because the access-token refresh below rewrites
        // `parts.session` in place when the persisted token has aged out.
        let mut parts = load_resume_state(&*store, &account_id).await?;
        let server = build_server_from_record(parts.server_cfg.clone());
        let http = HttpClient::new(&server.api_base_url, permit_http)?;
        // If the persisted access token has aged out, refresh it in
        // place with the persisted zuid instead of forcing a re-login
        // (see `refresh_stale_access_token`) — under the account's
        // process lock when a home is configured (Step 85).
        let client_id = parts.device.client_id.clone();
        refresh_stale_access_token(
            &mut parts.session,
            &http,
            store.as_ref(),
            &client_id,
            home.as_ref(),
        )
        .await?;
        let mls_public_key =
            decode_persisted_mls_public_key(parts.device.mls_public_key.as_deref())?;
        let mls_db_key = home
            .as_ref()
            .map(|home| {
                load_or_create_key_locked(
                    home,
                    KeyLock::Account(&account_id),
                    &keystore,
                    &key_id::account_mls(home.home_id(), &account_id),
                    &home.account_mls(&account_id),
                )
            })
            .transpose()?;
        let mls_client = resume_mls_state(
            home.as_ref(),
            &parts.session.user_id,
            &parts.device.client_id,
            &mls_public_key,
            mls_db_key.as_ref(),
        )?;
        assemble_resumed_client(parts, server, http, store, mls_client, mls_public_key, home)
    }

    /// Create a fresh Wire account and return an `Arc<PendingClient>`
    /// that the caller drives through
    /// [`PendingClient::activate`](super::PendingClient::activate)
    /// with the email-delivered activation code.
    ///
    /// `password` is wrapped in `Zeroizing` for in-memory holding
    /// inside the returned `PendingClient` (per
    /// `CLAUDE.md` §Sensitive Material). Wire holds the password
    /// server-side once registered; the in-memory copy exists only
    /// so the post-activate `POST /login` can complete without
    /// re-prompting the user.
    ///
    /// **In-memory only** — a consumer that crashes after `register`
    /// but before `activate` must re-call `register` (Wire then
    /// returns `409 key-exists`; the consumer should fall back to
    /// [`PendingClient::resend_activation_email`](super::PendingClient::resend_activation_email)
    /// on a *successfully-`register`ed* handle, see Plan-Doc §16c
    /// Open Question 3).
    ///
    /// # Errors
    /// * `InvalidInput` if `ClientBuilder::server` was not called.
    /// * `BackendRejected` for non-2xx from `POST /register`
    ///   (`409 key-exists` for a duplicate email, `403
    ///   blacklisted-email`, `400 invalid-email`, …). Wire label is
    ///   preserved in the error message.
    /// * `Network` / `Encoding` for transport / decode failures.
    pub async fn register(
        self,
        name: String,
        email: String,
        password: String,
    ) -> Result<Arc<PendingClient>, KupferdrahtError> {
        require_non_empty(
            "register",
            &[("name", &name), ("email", &email), ("password", &password)],
        )?;
        let server = self.server.clone().ok_or_else(|| {
            KupferdrahtError::InvalidInput("ClientBuilder::server was not called".into())
        })?;
        let keystore = resolve_keystore(self.keystore.clone(), self.permit_http).await?;
        let http = HttpClient::new(&server.api_base_url, self.permit_http)?;
        let body = NewUserRequest {
            name,
            email: Some(email.clone()),
            password: Some(password.clone()),
            ..Default::default()
        };
        let response = http.register_user(body).await?;
        Ok(Arc::new(new_pending(
            response.qualified_id.id,
            email,
            Zeroizing::new(password),
            server,
            self.base_dir,
            self.permit_http,
            self.storage_provider,
            keystore,
            http,
        )))
    }

    /// Process-symmetric counterpart to
    /// [`PendingClient::activate`](super::PendingClient::activate)
    /// for CLI use cases that lost the in-memory `PendingClient`
    /// (a separate `kupferdraht-cli activate` invocation can't share state
    /// with the prior `kupferdraht-cli register`). Takes the same inputs
    /// explicitly and threads them through the same two-step
    /// activation handshake (POST /activate + the existing login
    /// pipeline).
    ///
    /// Returns a ready `Arc<Client>`, just like `login` / `resume`.
    ///
    /// # Errors
    /// * `InvalidInput` if `ClientBuilder::server` was not called.
    /// * `BackendRejected` for non-2xx from `POST /activate`
    ///   (e.g. `404 invalid-code`, `400 invalid-email`). Wire label
    ///   is preserved.
    /// * Whatever `ClientBuilder::login` surfaces if the post-activate
    ///   `POST /login` fails (shouldn't happen for a fresh activation,
    ///   but propagated honestly).
    /// * `Network` / `Encoding` for transport / decode failures.
    pub async fn activate(
        self,
        email: String,
        code: String,
        password: String,
    ) -> Result<Arc<Client>, KupferdrahtError> {
        require_non_empty(
            "activate",
            &[("email", &email), ("code", &code), ("password", &password)],
        )?;
        let server = self.server.as_ref().ok_or_else(|| {
            KupferdrahtError::InvalidInput("ClientBuilder::server was not called".into())
        })?;
        let http = HttpClient::new(&server.api_base_url, self.permit_http)?;
        http.activate(&email, &code).await?;
        self.login(email, password, None)
            .await?
            .register_device()
            .await
    }
}

/// Pick the default **native** `SecureKeyStore` when the builder caller didn't
/// inject one. Branches, in order:
///
/// 1. `permit_http` (integration tests / `--insecure-http`): in-memory only —
///    CI has no desktop session, so an OS-keyring call would be flaky.
/// 2. `KUPFERDRAHT_DEV_KEYSTORE` set: file-backed `FileKeyStore` (dev / smoke escape
///    hatch on macOS where unsigned binaries can't satisfy the keychain ACL).
///    Loud by construction: `FileKeyStore::new` emits one `WARN` naming the
///    plaintext key directory (Step 77) — an accidental leftover of the
///    variable never downgrades a process silently.
/// 3. Otherwise: desktop-keyring-backed `KeyringStore` (production).
///
/// The wasm default is the persistent `IndexedDbKeyStore` (W2), resolved
/// asynchronously in [`resolve_keystore`].
#[cfg(not(target_arch = "wasm32"))]
fn default_keystore(permit_http: bool) -> Arc<dyn SecureKeyStore> {
    if permit_http {
        return Arc::new(InMemoryKeyStore::new());
    }
    if let Ok(dir) = std::env::var("KUPFERDRAHT_DEV_KEYSTORE") {
        return Arc::new(FileKeyStore::new(std::path::PathBuf::from(dir)));
    }
    Arc::new(KeyringStore::new("kupferdraht"))
}

/// Resolve the `SecureKeyStore` for a build: a caller-injected one wins,
/// otherwise the platform default. Async because the wasm default — the
/// persistent [`IndexedDbKeyStore`] (W2) — preloads its secrets from `IndexedDB`
/// at open. `permit_http` keeps the ephemeral in-memory store (tests).
///
/// # Errors
/// `KupferdrahtError::Generic` if the wasm keystore fails to open.
// `async` is load-bearing on wasm (it awaits `IndexedDbKeyStore::open`); on
// native that branch is `cfg`-ed out, leaving no await.
#[cfg_attr(not(target_arch = "wasm32"), allow(clippy::unused_async))]
async fn resolve_keystore(
    injected: Option<Arc<dyn SecureKeyStore>>,
    permit_http: bool,
) -> Result<Arc<dyn SecureKeyStore>, KupferdrahtError> {
    if let Some(keystore) = injected {
        return Ok(keystore);
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        Ok(default_keystore(permit_http))
    }
    #[cfg(target_arch = "wasm32")]
    {
        if permit_http {
            Ok(Arc::new(InMemoryKeyStore::new()))
        } else {
            Ok(Arc::new(IndexedDbKeyStore::open().await?))
        }
    }
}

/// The login cookie label of a build (Step 93, ADR-0025): the home's
/// `kupferdraht-<home-id>`, or — for a builder without a home (an in-memory
/// session, which persists no device and never resumes a cookie) — an
/// ephemeral one.
fn home_cookie_label(home: Option<&KupferdrahtHome>) -> String {
    home.map_or_else(ephemeral_cookie_label, KupferdrahtHome::cookie_label)
}

/// Resolve the storage provider: a custom override wins; otherwise the
/// shipped default `SQLite` provider on the resolved home; otherwise `None`
/// (in-memory, no persistence — tests / single-shot CLI). The keystore
/// is a dependency of the default provider only.
fn resolve_provider(
    custom: Option<Arc<dyn StorageProvider>>,
    home: Option<&KupferdrahtHome>,
    keystore: &Arc<dyn SecureKeyStore>,
) -> Option<Arc<dyn StorageProvider>> {
    custom.or_else(|| {
        home.map(|home| {
            Arc::new(SqliteStorageProvider::new(home.clone(), keystore.clone()))
                as Arc<dyn StorageProvider>
        })
    })
}

/// Wire's user-facing SSO code is `wire-<idp-uuid>`, but the SP endpoint
/// `/sso/initiate-login/{idp}` wants the bare `IdP` UUID (the prefixed form
/// 400s). Strip a leading `wire-`; a bare id passes through unchanged.
fn idp_id(code: &str) -> &str {
    code.strip_prefix("wire-").unwrap_or(code)
}

/// Shared post-authentication assembly for [`ClientBuilder::login`] and
/// [`ClientBuilder::finalize_sso`]: open the per-account store + `mls.db` key
/// from the resolved identity and build the [`PendingLogin`]. `password` is
/// `None` for the SSO path (Step 17β). Both flows converge here the moment
/// they hold a `(LoginOutcome, SelfUser)` — the pipeline is auth-agnostic
/// from this point on.
///
/// ADR-0035: a `Seed` home — the browser — opens nothing at login, with or
/// without the account's seed: `registerDevice` (a first setup, a restore)
/// opens the account's storage, so a takeover never needs a second login.
#[allow(clippy::too_many_arguments)]
async fn assemble_pending_login(
    storage_provider: Option<Arc<dyn StorageProvider>>,
    home: Option<KupferdrahtHome>,
    keystore: &Arc<dyn SecureKeyStore>,
    mut pipeline: LoginPipeline,
    login_outcome: LoginOutcome,
    self_user: SelfUser,
    email: String,
    password: Option<Zeroizing<String>>,
    device_ops_http: HttpClient,
) -> Result<Arc<PendingLogin>, KupferdrahtError> {
    let account_id = self_user.qualified_id.to_string();
    let open = opens_at_login(home.as_ref());
    let mut mls_db_key = None;
    if open {
        let (store, key) =
            open_account_storage(storage_provider, home.as_ref(), keystore, &account_id).await?;
        pipeline.bind_storage(store, home.clone());
        mls_db_key = key;
    }
    Ok(new_pending_login(
        self_user,
        email,
        login_outcome,
        pipeline,
        device_ops_http,
        password,
        mls_db_key,
        home,
        open,
    ))
}

/// Resolve the home at `base_dir` (Step 93) with the source of its account
/// keys (ADR-0035).
fn open_home(
    base_dir: Option<std::path::PathBuf>,
    account_keys: AccountKeySource,
) -> Result<Option<KupferdrahtHome>, KupferdrahtError> {
    base_dir
        .map(|base| KupferdrahtHome::open(base).map(|home| home.with_account_keys(account_keys)))
        .transpose()
}

/// Whether a login opens the account's storage (ADR-0035): natively yes;
/// in a `Seed` home — the browser — never: `registerDevice` opens it.
fn opens_at_login(home: Option<&KupferdrahtHome>) -> bool {
    home.is_none_or(|home| home.account_keys() != AccountKeySource::Seed)
}

/// Open an account's storage: its keys durably first (Step 104), then
/// `kupferdraht.db` through the provider, and the `mls.db` key — a separate key
/// from the `kupferdraht.db` one the provider self-keys with (ADR-0021).
///
/// # Errors
/// Key creation (`RecoveryDataRequired` in a `Seed` home without the seed)
/// and storage errors.
pub(super) async fn open_account_storage(
    storage_provider: Option<Arc<dyn StorageProvider>>,
    home: Option<&KupferdrahtHome>,
    keystore: &Arc<dyn SecureKeyStore>,
    account_id: &str,
) -> Result<(Option<Arc<dyn Store>>, Option<DbKey>), KupferdrahtError> {
    if let Some(home) = home {
        // Step 111a: the browser registers each database's VFS first.
        #[cfg(target_arch = "wasm32")]
        crate::storage::prepare_account_databases(home, account_id).await?;
        ensure_account_keys_durable(home, keystore, account_id, storage_provider.is_none()).await?;
    }
    let provider = resolve_provider(storage_provider, home, keystore);
    let store = provider
        .as_ref()
        .map(|p| p.open_store(account_id))
        .transpose()?;
    let mls_db_key = home
        .map(|home| {
            load_or_create_key_locked(
                home,
                KeyLock::Account(account_id),
                keystore,
                &key_id::account_mls(home.home_id(), account_id),
                &home.account_mls(account_id),
            )
        })
        .transpose()?;
    debug_assert!(
        home.is_none() || mls_db_key.is_some(),
        "a home keys its mls.db"
    );
    Ok((store, mls_db_key))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idp_id_strips_wire_prefix() {
        // Wire's user-facing code `wire-<uuid>` → the SP endpoint's bare id.
        assert_eq!(
            idp_id("wire-0f1e2d3c-4b5a-4978-8796-a5b4c3d2e1f0"),
            "0f1e2d3c-4b5a-4978-8796-a5b4c3d2e1f0"
        );
    }

    #[test]
    fn idp_id_passes_bare_id_through() {
        assert_eq!(
            idp_id("0f1e2d3c-4b5a-4978-8796-a5b4c3d2e1f0"),
            "0f1e2d3c-4b5a-4978-8796-a5b4c3d2e1f0"
        );
    }

    /// Step 93: the label is the home's; without a home it is ephemeral —
    /// a fresh one per build, never shared between two builds.
    #[test]
    fn cookie_label_is_the_homes_or_an_ephemeral_one_without_a_home() {
        let home = KupferdrahtHome::with_home_id(
            std::path::PathBuf::from("/tmp/kupferdraht-home"),
            "0123456789abcdef0123456789abcdef".to_owned(),
        );
        assert_eq!(
            home_cookie_label(Some(&home)),
            "kupferdraht-0123456789abcdef0123456789abcdef"
        );
        let first = home_cookie_label(None);
        let second = home_cookie_label(None);
        assert!(first.starts_with("kupferdraht-"));
        assert_ne!(first, second, "no home, no shared label");
    }
}
