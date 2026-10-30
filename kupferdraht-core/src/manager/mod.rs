// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `Kupferdraht` — the device-level account manager.
//!
//! Per-account [`Client`](crate::client::Client)s are each locked behind
//! their own key, so `Kupferdraht` is the entry point that sits above them:
//! it owns the device-wide [`GlobalStore`] registry (openable at app
//! start without unlocking any account), lists accounts for a picker,
//! and builds the per-account `Client` on login or resume — registering
//! the account as it goes.

mod pending;
mod pending_login;
// The integration tests' login mocks, shared by the crate's test modules.
#[cfg(test)]
#[path = "../../tests/common/login_mocks.rs"]
pub(crate) mod login_mocks;
// Step 114 (ADR-0035): the browser's logout with its purge options.
#[cfg(any(target_arch = "wasm32", test))]
mod purge;
#[cfg(test)]
mod purge_tests;
#[cfg(test)]
mod recovery_tests;
#[cfg(target_arch = "wasm32")]
mod remote_resume;
#[cfg(test)]
mod seed_login_tests;

pub use pending::PendingAccount;
pub use pending_login::PendingLogin;
#[cfg(any(target_arch = "wasm32", test))]
pub(crate) use pending_login::{KeysState, RegisterOptions};
#[cfg(any(target_arch = "wasm32", test))]
pub(crate) use purge::PurgeOptions;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Weak};

use crate::client::{Client, ClientBuilder, ServerConfig};
use crate::error::KupferdrahtError;
use crate::identity::UserId;
use crate::keystore::SecureKeyStore;
use crate::storage::account_keys::{self, AccountKeySource};
use crate::storage::{
    AccountEntry, BackendMlsKey, DeviceRecord, GlobalStore, KupferdrahtHome, SelfUserRecord,
    ServerConfigRecord, SessionRecord, SqliteStorageProvider, StorageProvider,
};

/// Device-level entry point. Open once at app start (no account needs
/// opening first); list accounts for a picker; log in or resume a
/// specific account.
pub struct Kupferdraht {
    /// The resolved home (Step 93, ADR-0025): its id is read once, at
    /// `open`, and every keystore id and the login label derive from it.
    home: KupferdrahtHome,
    keystore: Arc<dyn SecureKeyStore>,
    provider: Arc<dyn StorageProvider>,
    global: Arc<dyn GlobalStore>,
    permit_http: bool,
    /// The single live `Client` per `account_id` (Step 39). The `zuid`
    /// refresh cookie, `mls.db`, and notification cursor are account-level
    /// singletons, but per-`Client` state (tokens, `refresh_lock`, sync loop)
    /// is not — two Clients for one account race on the shared state and one
    /// `/access` rebind invalidates the other's cookie (`403`). `resume`
    /// returns the cached live instance instead of building a second. `Weak`
    /// so a fully torn-down session (consumer drops its `Arc`, sync loop
    /// cancelled) falls out naturally — no explicit eviction needed.
    clients: tokio::sync::Mutex<HashMap<String, Weak<Client>>>,
    /// Step 112 (ADR-0035): where the browser's recovery data is sealed and
    /// opened — the token exchange and the PIN service; `None` without a
    /// PIN service.
    #[cfg(any(target_arch = "wasm32", test))]
    recovery_services: Option<crate::recovery::flow::RecoveryServices>,
}

/// The device-manager config a login needs, bundled so the `Kupferdraht`
/// terminal methods (`login`, `resume`, `register`) build the per-account
/// `ClientBuilder` the same way.
#[derive(Clone)]
pub(crate) struct ManagerConfig {
    pub(crate) home: KupferdrahtHome,
    pub(crate) keystore: Arc<dyn SecureKeyStore>,
    pub(crate) provider: Arc<dyn StorageProvider>,
    pub(crate) permit_http: bool,
    #[cfg(any(target_arch = "wasm32", test))]
    pub(crate) recovery_services: Option<crate::recovery::flow::RecoveryServices>,
}

/// Build the per-account [`ClientBuilder`] from a [`ManagerConfig`].
fn build_client_builder(config: &ManagerConfig, server: Option<ServerConfig>) -> ClientBuilder {
    let mut builder = ClientBuilder::new()
        .base_dir(config.home.base_dir().to_path_buf())
        .storage_provider(config.provider.clone())
        .keystore(config.keystore.clone())
        .account_keys(config.home.account_keys());
    if let Some(server) = server {
        builder = builder.server(server);
    }
    if config.permit_http {
        builder = builder.permit_http();
    }
    builder
}

impl Kupferdraht {
    /// Open the device manager rooted at `base_dir`, backed by the shipped
    /// default `SQLite` [`StorageProvider`] (encrypted `global.db`
    /// registry plus per-account stores). For a custom backend (e.g. a
    /// cloud-bot server), use [`Kupferdraht::builder`].
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` if the registry cannot be opened.
    pub fn open(
        base_dir: PathBuf,
        keystore: Arc<dyn SecureKeyStore>,
    ) -> Result<Self, KupferdrahtError> {
        Self::builder()
            .base_dir(base_dir)
            .keystore(keystore)
            .build()
    }

    /// Start a [`KupferdrahtBuilder`] to inject a custom [`StorageProvider`]
    /// (e.g. a server-side backend for cloud bots).
    #[must_use]
    pub fn builder() -> KupferdrahtBuilder {
        KupferdrahtBuilder::default()
    }

    /// Permit plain-HTTP server URLs (local dev / tests); production uses
    /// HTTPS only.
    #[must_use]
    pub fn permit_http(mut self) -> Self {
        self.permit_http = true;
        self
    }

    /// The device account registry, listable without unlocking any
    /// account.
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` on a registry read failure.
    pub async fn accounts(&self) -> Result<Vec<AccountEntry>, KupferdrahtError> {
        let accounts = self.global.list_accounts().await?;
        if self.home.account_keys() == AccountKeySource::Random {
            return Ok(accounts);
        }
        // ADR-0035: an account is listed only while its seed is here.
        let mut listed = Vec::with_capacity(accounts.len());
        for account in accounts {
            if self.holds_seed(&account.account_id)? {
                listed.push(account);
            }
        }
        Ok(listed)
    }

    /// In a home whose account keys come from a seed (the browser), attach
    /// the first setup to a pending login (ADR-0035).
    #[cfg(any(target_arch = "wasm32", test))]
    async fn attach_first_setup(
        &self,
        pending: PendingLogin,
    ) -> Result<PendingLogin, KupferdrahtError> {
        if self.home.account_keys() == AccountKeySource::Random {
            return Ok(pending);
        }
        pending.with_first_setup(self.config()).await
    }

    /// Whether this home holds the seed of `account_id` (ADR-0035).
    fn holds_seed(&self, account_id: &str) -> Result<bool, KupferdrahtError> {
        account_keys::has_seed(&self.keystore, self.home.home_id(), account_id)
    }

    /// The account id last marked active, if any.
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` on a registry read failure.
    pub async fn active_account(&self) -> Result<Option<String>, KupferdrahtError> {
        self.global.active_account().await
    }

    /// The id of this home (Step 93, ADR-0025): 32 lowercase hex characters
    /// persisted in `base/home.id`, created once under the home lock. Every
    /// keystore id of this home and its login cookie label carry it.
    #[must_use]
    pub fn home_id(&self) -> &str {
        self.home.home_id()
    }

    /// The Wire login cookie label of this home, `kupferdraht-<home-id>`
    /// (Step 93, ADR-0025): the `(user, label)` slot Wire keeps one refresh
    /// cookie in — per home, so two homes of one user never evict each other.
    #[must_use]
    pub fn cookie_label(&self) -> String {
        self.home.cookie_label()
    }

    /// Phase 1 of login: authenticate against `server` and return a
    /// [`PendingLogin`]. Call [`PendingLogin::register_device`] to register
    /// the device, write the registry row (marking the account active), and
    /// obtain a ready, resumable `Client`.
    ///
    /// The split lets a consumer recover from Wire's per-user device cap
    /// (`TooManyDevices` from `register_device`) without re-authenticating —
    /// the returned handle frees a slot and retries on the captured session.
    ///
    /// # Errors
    /// Authentication failures (`InvalidCredentials`, `SecondFactorRequired`,
    /// …) surface here; device-cap / registration failures surface from
    /// `register_device`.
    pub async fn login(
        &self,
        server: ServerConfig,
        email: String,
        password: String,
        second_factor_code: Option<String>,
        device_label: Option<String>,
    ) -> Result<Arc<PendingLogin>, KupferdrahtError> {
        let mut builder = self.client_builder(Some(server.clone()));
        if let Some(label) = device_label {
            builder = builder.device_label(label);
        }
        let inner = builder.login(email, password, second_factor_code).await?;
        let pending = PendingLogin::new(inner, server, self.global.clone());
        #[cfg(any(target_arch = "wasm32", test))]
        let pending = self.attach_first_setup(pending).await?;
        Ok(Arc::new(pending))
    }

    /// SSO (Step 17β) — validate `code` against `server` and return the
    /// interactive initiate URL the consumer opens in a browser. The
    /// `zuid` cookie from the post-`IdP` redirect then feeds `finalize_sso`
    /// (17b) to complete login.
    ///
    /// # Errors
    /// `InvalidInput` for an unknown code; `BackendRejected` / `Network`
    /// from the code-existence probe.
    pub async fn initiate_sso(
        &self,
        server: ServerConfig,
        code: &str,
        success_redirect: &str,
        error_redirect: &str,
    ) -> Result<String, KupferdrahtError> {
        self.client_builder(Some(server))
            .initiate_sso(code, success_redirect, error_redirect)
            .await
    }

    /// SSO email discovery (Step 17β) — resolve the backend's default SSO code
    /// (`GET /sso/settings`) for `email` and return the interactive initiate
    /// URL. Wire v14 has no per-domain discovery, so the team default is used;
    /// multi-IdP backends must use [`initiate_sso`](Self::initiate_sso).
    ///
    /// # Errors
    /// `InvalidInput` for an empty email or a backend with no default SSO
    /// code; otherwise whatever `ClientBuilder::initiate_sso_by_email` surfaces.
    pub async fn initiate_sso_by_email(
        &self,
        server: ServerConfig,
        email: &str,
        success_redirect: &str,
        error_redirect: &str,
    ) -> Result<String, KupferdrahtError> {
        self.client_builder(Some(server))
            .initiate_sso_by_email(email, success_redirect, error_redirect)
            .await
    }

    /// SSO finalize (Step 17β) — complete login from the `zuid` cookie handed
    /// back by Wire's `/sso/finalize-login` redirect, returning a
    /// [`PendingLogin`] whose `register_device` finishes login (and writes the
    /// registry row) exactly as the password flow.
    ///
    /// # Errors
    /// `InvalidCredentials` / `SessionExpired` for a revoked cookie; otherwise
    /// whatever `ClientBuilder::finalize_sso` surfaces.
    pub async fn finalize_sso(
        &self,
        server: ServerConfig,
        sso_cookie: &str,
        device_label: Option<String>,
    ) -> Result<Arc<PendingLogin>, KupferdrahtError> {
        let mut builder = self.client_builder(Some(server.clone()));
        if let Some(label) = device_label {
            builder = builder.device_label(label);
        }
        let inner = builder.finalize_sso(sso_cookie).await?;
        let pending = PendingLogin::new(inner, server, self.global.clone());
        #[cfg(any(target_arch = "wasm32", test))]
        let pending = self.attach_first_setup(pending).await?;
        Ok(Arc::new(pending))
    }

    /// Create a fresh Wire account on `server`, returning a
    /// [`PendingAccount`] to drive through activation with the emailed
    /// code. `register` alone writes nothing to the device registry —
    /// the account becomes listed, active, and resumable when
    /// [`PendingAccount::activate`] completes (same registry + session
    /// writes as [`Kupferdraht::login`]).
    ///
    /// # Errors
    /// Whatever `ClientBuilder::register` surfaces.
    pub async fn register(
        &self,
        server: ServerConfig,
        name: String,
        email: String,
        password: String,
    ) -> Result<PendingAccount, KupferdrahtError> {
        let pending = self
            .client_builder(Some(server.clone()))
            .register(name, email, password)
            .await?;
        Ok(PendingAccount::new(pending, server, self.global.clone()))
    }

    /// Resume `account_id` from disk (no Wire login). The server config
    /// is read from the per-account store; the registry `account_id` is
    /// validated against what is on disk.
    ///
    /// # Errors
    /// `KupferdrahtError::InvalidInput` if the account is not registered or
    /// the resumed account does not match; otherwise whatever
    /// `ClientBuilder::resume` surfaces.
    pub async fn resume(&self, account_id: &str) -> Result<Arc<Client>, KupferdrahtError> {
        let known = self
            .global
            .list_accounts()
            .await?
            .into_iter()
            .any(|a| a.account_id == account_id);
        if !known {
            return Err(KupferdrahtError::InvalidInput(format!(
                "unknown account: {account_id}"
            )));
        }
        if self.home.account_keys() == AccountKeySource::Seed && !self.holds_seed(account_id)? {
            return Err(KupferdrahtError::RecoveryDataRequired(
                account_id.to_owned(),
            ));
        }
        // Step 39: one live `Client` per account. Hold the cache lock across the
        // build so concurrent resumes for the same account collapse to a single
        // rebind instead of racing the `zuid`. A still-live Client (its sync loop
        // holds an `Arc`) is reused as-is; a torn-down one (`Weak` dangling) is
        // rebuilt.
        let mut cache = self.clients.lock().await;
        if let Some(existing) = cache.get(account_id).and_then(Weak::upgrade) {
            self.global.set_active_account(Some(account_id)).await?;
            return Ok(existing);
        }
        let client = self
            .client_builder(None)
            .account(account_id)
            .resume()
            .await?;
        if client.user_id().map(UserId::as_str) != Some(account_id) {
            return Err(KupferdrahtError::InvalidInput(format!(
                "resumed account {} does not match requested {account_id}",
                client.user_id().map_or("?", UserId::as_str)
            )));
        }
        self.global.set_active_account(Some(account_id)).await?;
        cache.insert(account_id.to_string(), Arc::downgrade(&client));
        Ok(client)
    }

    /// Snapshot the manager config used to build the per-account
    /// `ClientBuilder` (shared by `login`, `register`, and `resume`).
    pub(crate) fn config(&self) -> ManagerConfig {
        ManagerConfig {
            home: self.home.clone(),
            keystore: self.keystore.clone(),
            provider: self.provider.clone(),
            permit_http: self.permit_http,
            #[cfg(any(target_arch = "wasm32", test))]
            recovery_services: self.recovery_services.clone(),
        }
    }

    /// What a client's recovery calls need of this manager (Step 112).
    #[cfg(any(target_arch = "wasm32", test))]
    pub(crate) fn recovery_context(&self) -> crate::recovery::account::RecoveryContext {
        crate::recovery::account::RecoveryContext {
            keystore: self.keystore.clone(),
            home_id: self.home.home_id().to_owned(),
            services: self.recovery_services.clone(),
        }
    }

    fn client_builder(&self, server: Option<ServerConfig>) -> ClientBuilder {
        build_client_builder(&self.config(), server)
    }
}

/// Write the account's registry row and mark it active. Shared by
/// [`Kupferdraht::login`] and [`PendingAccount::activate`].
async fn register_account(
    global: &Arc<dyn GlobalStore>,
    client: &Arc<Client>,
    server: String,
) -> Result<(), KupferdrahtError> {
    let account = client.account();
    let display = account.display_name();
    let entry = AccountEntry {
        account_id: account.user_id().as_str().to_string(),
        server,
        handle: account.handle().map(str::to_string),
        display_name: (!display.is_empty()).then(|| display.to_string()),
        last_active_ms: crate::time::now_ms(),
    };
    global.upsert_account(&entry).await?;
    global.set_active_account(Some(&entry.account_id)).await
}

/// Builder for [`Kupferdraht`] — the way to inject a custom [`StorageProvider`]
/// (e.g. a server-side backend for cloud bots). `base_dir` and `keystore`
/// are required; `storage_provider` defaults to the shipped `SQLite` one;
/// `base_dir` is still needed for `mls.db`.
#[derive(Default)]
pub struct KupferdrahtBuilder {
    base_dir: Option<PathBuf>,
    keystore: Option<Arc<dyn SecureKeyStore>>,
    storage_provider: Option<Arc<dyn StorageProvider>>,
    permit_http: bool,
    account_keys: AccountKeySource,
    #[cfg(any(target_arch = "wasm32", test))]
    recovery_services: Option<crate::recovery::flow::RecoveryServices>,
}

impl KupferdrahtBuilder {
    /// Where the browser's recovery data is sealed and opened (Step 112).
    #[cfg(any(target_arch = "wasm32", test))]
    #[must_use]
    pub(crate) fn recovery_services(
        mut self,
        services: crate::recovery::flow::RecoveryServices,
    ) -> Self {
        self.recovery_services = Some(services);
        self
    }

    /// The persistence root (required). Hosts the default `global.db` and
    /// the per-account `accounts/<id>/` directories.
    #[must_use]
    pub fn base_dir(mut self, base_dir: PathBuf) -> Self {
        self.base_dir = Some(base_dir);
        self
    }

    /// The platform keystore (required). The only place the manager hands
    /// out the keystore; the default provider self-fetches its keys.
    #[must_use]
    pub fn keystore(mut self, keystore: Arc<dyn SecureKeyStore>) -> Self {
        self.keystore = Some(keystore);
        self
    }

    /// Override the storage backend (global + per-account stores). When
    /// unset, the shipped `SQLite` provider rooted at `base_dir` is used.
    #[must_use]
    pub fn storage_provider(mut self, provider: Arc<dyn StorageProvider>) -> Self {
        self.storage_provider = Some(provider);
        self
    }

    /// Permit plain-HTTP server URLs (local dev / tests).
    #[must_use]
    pub fn permit_http(mut self) -> Self {
        self.permit_http = true;
        self
    }

    /// Where a new key of an account comes from (ADR-0035): `Seed` in the
    /// browser, `Random` — the default — natively.
    #[cfg(any(target_arch = "wasm32", test))]
    #[must_use]
    pub(crate) fn account_keys(mut self, source: AccountKeySource) -> Self {
        self.account_keys = source;
        self
    }

    /// Build the [`Kupferdraht`] manager, opening the device-level registry via
    /// the (default or custom) provider.
    ///
    /// # Errors
    /// `KupferdrahtError::InvalidInput` if `base_dir` / `keystore` are missing;
    /// `KupferdrahtError::Storage` if the registry cannot be opened.
    pub fn build(self) -> Result<Kupferdraht, KupferdrahtError> {
        let base_dir = self.base_dir.ok_or_else(|| {
            KupferdrahtError::InvalidInput("Kupferdraht::builder requires base_dir".into())
        })?;
        let keystore = self.keystore.ok_or_else(|| {
            KupferdrahtError::InvalidInput("Kupferdraht::builder requires keystore".into())
        })?;
        // Step 93: resolve the home first — the directory, and its id
        // (created under the home lock on a first open) — so the default
        // provider keys everything under this home's ids.
        let home = KupferdrahtHome::open(base_dir)?.with_account_keys(self.account_keys);
        let provider: Arc<dyn StorageProvider> = self.storage_provider.unwrap_or_else(|| {
            Arc::new(SqliteStorageProvider::new(home.clone(), keystore.clone()))
        });
        let global = provider.open_global()?;
        Ok(Kupferdraht {
            home,
            keystore,
            provider,
            global,
            permit_http: self.permit_http,
            clients: tokio::sync::Mutex::new(HashMap::new()),
            #[cfg(any(target_arch = "wasm32", test))]
            recovery_services: self.recovery_services,
        })
    }
}

/// Persist the just-logged-in `client`'s session into its (SDK-opened)
/// store so a later [`Kupferdraht::resume`] can reconstruct it without a Wire
/// login. A no-op when the client has no store (in-memory clients).
/// Shared by [`Kupferdraht::login`] and [`PendingAccount::activate`].
async fn persist_session(
    client: &Arc<Client>,
    server: &ServerConfig,
) -> Result<(), KupferdrahtError> {
    use base64::Engine as _;

    let Some(store) = client.store() else {
        return Ok(());
    };
    let device_id = client.device_id().unwrap_or_default().to_string();
    let account = client.account();
    let user_id = account.user_id().as_str().to_string();

    store
        .save_session(&SessionRecord {
            user_id: user_id.clone(),
            device_id: device_id.clone(),
            access_token: client.access_token().unwrap_or_default(),
            refresh_token: client.refresh_token().unwrap_or_default(),
            // Step 93: the label this session was logged in with, so a
            // session still on a pre-Step-93 label is diagnosable.
            cookie_label: client.cookie_label().map(str::to_owned),
            api_version: client.api_version(),
            access_token_issued_at_ms: crate::time::now_ms(),
        })
        .await?;
    store
        .save_device(&DeviceRecord {
            user_id: user_id.clone(),
            client_id: device_id.clone(),
            mls_public_key: Some(
                base64::engine::general_purpose::STANDARD.encode(client.mls_signature_public_key()),
            ),
        })
        .await?;
    store
        .save_self_user(&SelfUserRecord {
            id: user_id,
            handle: account.handle().map(str::to_string),
            name: account.display_name().to_string(),
            email: account.email().map(str::to_string),
        })
        .await?;
    store
        .save_server_config(&ServerConfigRecord {
            api_base_url: server.api_base_url.clone(),
            websocket_url: server.websocket_url.clone(),
            title: server.title.clone(),
        })
        .await?;
    store
        .save_backend_mls_key(&BackendMlsKey {
            purpose: "removal".to_string(),
            signature_scheme: "ecdsa_secp256r1_sha256".to_string(),
            public_key: client.backend_removal_key_ecdsa_p256().to_vec(),
        })
        .await?;
    Ok(())
}
