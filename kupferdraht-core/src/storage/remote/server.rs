// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! The browser's server mode (Step 111b, ADR-0035): an account's
//! `kupferdraht.db` and `mls.db` on a `sqlite-remote-server`. **wasm-only.**
//!
//! `Kupferdraht.open({ storage: { mode: "server", … } })` configures the
//! storage server and the token exchange ([`use_storage_server`]); nothing
//! is built in. Before an account's databases open, a Wire session buys a
//! storage token for the account's storage-login key
//! ([`prepare_remote_account`]), and each database registers its VFS with
//! a signer over that key — never the key — and a token source reading
//! the cached token. The token is renewed in the background at half its
//! lifetime, with the client's own session once the client exists
//! ([`attach_account_client`]); a renewal the exchange refuses (`403`)
//! stops the instance: `onEvicted("access")`. `global.db` and the home id
//! stay local.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Weak};
use std::time::Duration;

use sqlite_remote_vfs::{Algorithm, Cache, Config, RemoteVfs, Server, Signer, Store, TokenSource};
use zeroize::Zeroizing;

use super::health::{ObserverId, StorageHealth, StorageState};
use super::instance_id::storage_instance_id;
use super::login_key::{StorageLoginKey, account_storage_login_key};
use super::token_cache::StorageTokenCache;
use super::token_exchange::StorageToken;
use super::wire_session::{TokenExchange, WireSession, buy_storage_token};
use crate::error::KupferdrahtError;
use crate::keystore::SecureKeyStore;
use crate::storage::KupferdrahtHome;
use crate::storage::browser::{self, BrowserEviction, ServerRegistration};
use crate::wire::http::TokenRefresh;

/// How long before its expiry a token is no longer handed to the VFS: a
/// login must not race the expiry.
const EXPIRY_MARGIN: Duration = Duration::from_secs(30);

/// The wait after a renewal that failed for a passing reason.
const RETRY_AFTER_FAILURE: Duration = Duration::from_secs(60);

/// The configured storage server and token exchange.
struct ServerSettings {
    url: String,
    exchange: TokenExchange,
}

/// An account's server session: its signer, the VFS instance id of its
/// databases, the cached storage token, the Wire session that renews it,
/// and whether its databases take operations (Step 113).
struct RemoteAccount {
    public_key: [u8; 32],
    instance_id: [u8; 16],
    health: StorageHealth,
    signer: Arc<LoginSigner>,
    cache: Arc<StorageTokenCache>,
    session: RefCell<Rc<WireSession>>,
    renewing: Cell<bool>,
}

thread_local! {
    static SERVER: RefCell<Option<Rc<ServerSettings>>> = const { RefCell::new(None) };
    static ACCOUNTS: RefCell<HashMap<String, Rc<RemoteAccount>>> = RefCell::new(HashMap::new());
}

/// The signer the VFS logs in with: the storage-login key behind it.
struct LoginSigner(StorageLoginKey);

impl Signer for LoginSigner {
    fn algorithm(&self) -> Algorithm {
        Algorithm::Ed25519
    }

    fn public_key(&self) -> Vec<u8> {
        self.0.public_key().to_vec()
    }

    fn sign(&self, message: &[u8]) -> Vec<u8> {
        self.0.sign(message).to_vec()
    }
}

impl TokenSource for StorageTokenCache {
    fn token(&self) -> Result<Zeroizing<String>, String> {
        self.current(crate::time::now_ms())
    }
}

/// Keep the account databases on the storage server at `url` (`wss://`),
/// with storage tokens from the token exchange at `token_exchange_url`
/// (`https://`); plain `ws://` / `http://` only with `permit_http`.
///
/// # Errors
/// `InvalidInput` for an address of the wrong scheme; `Storage` when the
/// storage is already configured with other addresses.
pub(crate) fn use_storage_server(
    url: String,
    token_exchange_url: String,
    permit_http: bool,
) -> Result<(), KupferdrahtError> {
    refuse_insecure(&url, "wss://", "ws://", permit_http, "storage server")?;
    refuse_insecure(
        &token_exchange_url,
        "https://",
        "http://",
        permit_http,
        "token exchange",
    )?;
    SERVER.with(|s| {
        let mut server = s.borrow_mut();
        if let Some(current) = server.as_ref() {
            if current.url != url || current.exchange.url != token_exchange_url {
                return Err(KupferdrahtError::Storage(format!(
                    "browser storage: already on the storage server {}",
                    current.url
                )));
            }
            return Ok(());
        }
        *server = Some(Rc::new(ServerSettings {
            url,
            exchange: TokenExchange {
                url: token_exchange_url,
                permit_http,
            },
        }));
        Ok(())
    })?;
    debug_assert!(storage_server_configured(), "the server mode is on");
    Ok(())
}

fn refuse_insecure(
    address: &str,
    secure: &str,
    plain: &str,
    permit_http: bool,
    what: &str,
) -> Result<(), KupferdrahtError> {
    if address.starts_with(secure) || (permit_http && address.starts_with(plain)) {
        return Ok(());
    }
    Err(KupferdrahtError::InvalidInput(format!(
        "{what} {address}: needs {secure} (plain {plain} only with permitHttp)"
    )))
}

/// Whether the account databases live on a storage server.
pub(crate) fn storage_server_configured() -> bool {
    SERVER.with(|s| s.borrow().is_some())
}

fn server_settings() -> Result<Rc<ServerSettings>, KupferdrahtError> {
    SERVER
        .with(|s| s.borrow().clone())
        .ok_or_else(|| KupferdrahtError::Storage("browser storage: no storage server".into()))
}

/// Make the databases of `account_id` ready to open on the storage server:
/// buy a storage token with `session`, register each database's VFS with
/// the account's signer (once per key and takeover), and keep the token
/// renewed. With `create` a missing database is created on its open,
/// without it the open fails as `NothingStored`. With `take_over` the open
/// takes a database another instance holds over (that instance stops:
/// `onEvicted("device")`), without it such an open fails as
/// `HeldElsewhere` — the takeover is the opening call's (Step 111c,
/// ADR-0035), never `Kupferdraht.open`'s. Nothing in the local mode.
///
/// # Errors
/// `RecoveryDataRequired` without the account's seed; the token purchase's
/// errors (`StorageAccessDenied`, `ServiceUnavailable`, `RateLimited`,
/// `Unauthorized`); `StorageAccessDenied` / `ServiceUnavailable` from a
/// registration; `Evicted` after an eviction.
pub(crate) async fn prepare_remote_account(
    home: &KupferdrahtHome,
    keystore: &Arc<dyn SecureKeyStore>,
    account_id: &str,
    session: WireSession,
    create: bool,
    take_over: bool,
) -> Result<(), KupferdrahtError> {
    if !storage_server_configured() {
        return Ok(());
    }
    browser::refuse_if_evicted()?;
    let settings = server_settings()?;
    let account = remote_account(
        home.home_id(),
        account_id,
        account_storage_login_key(home, keystore, account_id)?,
    );
    *account.session.borrow_mut() = Rc::new(session);
    let session = Rc::clone(&account.session.borrow());
    let token = buy_storage_token(&settings.exchange, &session, &account.public_key).await?;
    let renew_after = token.renew_after();
    store_token(&account, token);
    let registration = ServerRegistration {
        owner: account.public_key,
        take_over,
    };
    for path in [home.account_db(account_id), home.account_mls(account_id)] {
        register_database(&settings, account_id, &account, &path, registration).await?;
        browser::set_database_create(&path, create);
    }
    keep_token_renewed(Rc::clone(&account), renew_after);
    debug_assert!(
        ensure_remote_account_prepared(home, account_id).is_ok(),
        "both databases are registered for the account's key"
    );
    Ok(())
}

/// The account's server session for `key`: the known one, or a new one
/// when there is none or it has another key (a new seed).
fn remote_account(home_id: &str, account_id: &str, key: StorageLoginKey) -> Rc<RemoteAccount> {
    let public_key = key.public_key();
    ACCOUNTS.with(|a| {
        let mut accounts = a.borrow_mut();
        if let Some(known) = accounts.get(account_id)
            && known.public_key == public_key
        {
            return Rc::clone(known);
        }
        let account = Rc::new(RemoteAccount {
            public_key,
            instance_id: storage_instance_id(home_id, account_id),
            health: StorageHealth::new(),
            signer: Arc::new(LoginSigner(key)),
            cache: Arc::new(StorageTokenCache::default()),
            session: RefCell::new(Rc::new(WireSession::Login(Zeroizing::default()))),
            renewing: Cell::new(false),
        });
        accounts.insert(account_id.to_owned(), Rc::clone(&account));
        account
    })
}

/// Register the VFS of the database at `path` for `registration`, unless
/// it is registered for it already.
async fn register_database(
    settings: &ServerSettings,
    account_id: &str,
    account: &RemoteAccount,
    path: &std::path::Path,
    registration: ServerRegistration,
) -> Result<(), KupferdrahtError> {
    debug_assert_eq!(
        registration.owner, account.public_key,
        "the account's own key"
    );
    if browser::server_registration(path) == Some(registration) {
        return Ok(());
    }
    let mut server = Server::new(settings.url.clone(), account.signer.clone());
    server.token = Some(account.cache.clone());
    server.cache = Cache::Browser;
    // Step 111d: stable per home and account — a reload takes its own
    // lease back at once; one home runs one instance (ADR-0023).
    server.instance_id = Some(account.instance_id);
    // Step 113 (ADR-0038): the VFS heals an unreachable database itself, so a
    // commit that meets an outage gives up after 3 s instead of 10.
    server.reconnect_timeout = Duration::from_secs(3);
    // Step 119: another instance of the account taking the database over
    // stops this one at once — not at its next write.
    server.on_takeover = Some(Arc::new(browser::database_taken_over));
    let mut config = Config::new(Store::Server(server));
    config.takeover = registration.take_over;
    let vfs = RemoteVfs::register_async(&browser::registration_name("server"), config)
        .await
        .map_err(|e| registration_error(path, &e))?;
    browser::insert_server_database(path, vfs, registration, account_id);
    Ok(())
}

fn registration_error(
    path: &std::path::Path,
    error: &sqlite_remote_vfs::Error,
) -> KupferdrahtError {
    let what = format!("storage server: {}: {error}", path.display());
    if error.is_access_denied() {
        KupferdrahtError::StorageAccessDenied(what)
    } else {
        KupferdrahtError::ServiceUnavailable(what)
    }
}

fn store_token(account: &RemoteAccount, token: StorageToken) {
    debug_assert!(!token.token.is_empty(), "the exchange issues a token");
    let lifetime = token.expires_in.saturating_sub(EXPIRY_MARGIN);
    let expires_at = crate::time::now_ms()
        .saturating_add(i64::try_from(lifetime.as_millis()).unwrap_or(i64::MAX));
    account.cache.set(token.token, expires_at);
}

/// From now on the client's own session renews the storage token of
/// `account_id` — the login's token it was bought with expires.
pub(crate) fn attach_account_client(account_id: &str, client: Weak<dyn TokenRefresh>) {
    let Some(account) = known_account(account_id) else {
        return;
    };
    *account.session.borrow_mut() = Rc::new(WireSession::Client(client));
    debug_assert!(
        matches!(**account.session.borrow(), WireSession::Client(_)),
        "the client's session renews the token"
    );
}

/// Start the account's renewal unless it runs.
fn keep_token_renewed(account: Rc<RemoteAccount>, first_wait: Duration) {
    if account.renewing.replace(true) {
        return;
    }
    debug_assert!(account.renewing.get(), "one renewal task per account");
    wasm_bindgen_futures::spawn_local(async move {
        renew_until_stopped(&account, first_wait).await;
        account.renewing.set(false);
    });
}

/// Renew the storage token at half its lifetime until the instance stops,
/// the session is gone, or the exchange refuses the user — which stops the
/// instance (`onEvicted("access")`).
async fn renew_until_stopped(account: &RemoteAccount, mut wait: Duration) {
    loop {
        crate::time::sleep(wait).await;
        if crate::storage::browser_storage_evicted() {
            return;
        }
        let Ok(settings) = server_settings() else {
            return;
        };
        let session = Rc::clone(&account.session.borrow());
        wait = match buy_storage_token(&settings.exchange, &session, &account.public_key).await {
            Ok(token) => {
                let next = token.renew_after();
                store_token(account, token);
                next
            }
            Err(KupferdrahtError::StorageAccessDenied(reason)) => {
                tracing::warn!(%reason, "storage token refused: the instance stops");
                browser::mark_evicted(BrowserEviction::Access);
                return;
            }
            Err(KupferdrahtError::SessionExpired) => {
                tracing::info!("storage token renewal ends with the session");
                return;
            }
            Err(KupferdrahtError::RateLimited { retry_after_secs }) => {
                Duration::from_secs(retry_after_secs.unwrap_or(0)).max(RETRY_AFTER_FAILURE)
            }
            Err(e) => {
                tracing::warn!(error = %e, "storage token renewal failed; retrying");
                RETRY_AFTER_FAILURE
            }
        };
    }
}

/// Whether the database at `path` of `account_id` took its last operation
/// (Step 113): the account's health — and its `onStorage` observers — hear
/// each database.
pub(crate) fn report_database_reachable(account_id: &str, path: &std::path::Path, reachable: bool) {
    if let Some(account) = known_account(account_id) {
        account
            .health
            .set_reachable(&path.to_string_lossy(), reachable);
    }
}

/// Whether the databases of `account_id` take operations — `online` in the
/// local mode and for an account the storage server has not seen.
pub(crate) fn account_storage_state(account_id: &str) -> StorageState {
    known_account(account_id).map_or(StorageState::Online, |a| a.health.state())
}

/// Call `observer` on every change of `account_id`'s storage state; `None`
/// when the account has no databases on the storage server.
pub(crate) fn subscribe_storage_state(
    account_id: &str,
    observer: Box<dyn Fn(StorageState)>,
) -> Option<ObserverId> {
    known_account(account_id).map(|account| account.health.subscribe(observer))
}

/// Stop calling the storage-state observer `id` of `account_id`.
pub(crate) fn unsubscribe_storage_state(account_id: &str, id: ObserverId) {
    if let Some(account) = known_account(account_id) {
        account.health.unsubscribe(id);
    }
}

/// Whether the databases of `account_id` are on the storage server (Step
/// 122 — `storageEngine().location`).
pub(crate) fn account_on_storage_server(account_id: &str) -> bool {
    known_account(account_id).is_some()
}

fn known_account(account_id: &str) -> Option<Rc<RemoteAccount>> {
    ACCOUNTS.with(|a| a.borrow().get(account_id).cloned())
}

/// Whether the databases of `account_id` were prepared on the storage
/// server, as they must be before an open in the server mode.
///
/// # Errors
/// `Storage` when they were not.
pub(crate) fn ensure_remote_account_prepared(
    home: &KupferdrahtHome,
    account_id: &str,
) -> Result<(), KupferdrahtError> {
    let prepared = [home.account_db(account_id), home.account_mls(account_id)]
        .iter()
        .all(|path| browser::server_registration(path).is_some());
    if prepared {
        return Ok(());
    }
    Err(KupferdrahtError::Storage(format!(
        "browser storage: the databases of {account_id} are on the storage server and need a \
         session first — log in, or resume through Kupferdraht"
    )))
}
