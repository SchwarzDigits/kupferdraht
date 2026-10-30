// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Browser binding for the login / session lifecycle — the real W10 surface
//! (parallel to `ffi::manager`), not test scaffolding.
//!
//! Mirrors the native `ffi` wrapper-object shape: each stateful core type gets a
//! `#[wasm_bindgen]` wrapper holding an `Arc<Core>`; `async` core methods become
//! `Promise`-returning `async fn`s; `KupferdrahtError` maps to a thrown JS `Error`
//! whose `name` is the variant (Step 110, ADR-0035).
//!
//! The vertical exposed here — `Kupferdraht.open → login → PendingLogin.register_device
//! → Client` — exercises the whole wasm stack end to end: fetch (W5), the `zuid`
//! browser cookie jar (W6), the persistent `IndexedDb` keystore (W2), and the
//! `SQLite3MC` store — on the storage server (ADR-0035), or over OPFS in the
//! local mode (Step 91, ADR-0022).
//!
//! Binding-layer lint posture: errors surface to JS as thrown exceptions (the
//! `.d.ts` is the contract, not rustdoc `# Errors`), and entry points naturally
//! take wide argument lists — so `missing_errors_doc`/`too_many_arguments` are
//! allowed here, as on the native `ffi` surface.
#![allow(clippy::missing_errors_doc, clippy::too_many_arguments)]

use std::path::PathBuf;
use std::sync::Arc;

use wasm_bindgen::prelude::*;

use zeroize::Zeroizing;

use crate::client::ServerConfig;
use crate::error::KupferdrahtError;
use crate::keystore::IndexedDbKeyStore;
use crate::manager::{KeysState, RegisterOptions};
use crate::recovery::account::{RecoveryContext, RecoveryState};
use crate::recovery::flow::RecoveryServices;
use crate::storage::account_keys::AccountKeySource;
use crate::storage::remote::wire_session::TokenExchange;

/// Map a core error into a thrown JS `Error`: the message as before, the
/// `name` its `KupferdrahtError` variant — JavaScript branches on it as Kotlin and
/// Swift do on the typed error (Step 110, ADR-0035).
#[must_use]
pub fn js_error(e: &KupferdrahtError) -> JsValue {
    to_js(e)
}

/// [`js_error`] under the name the binding's methods use.
fn to_js(e: &KupferdrahtError) -> JsValue {
    // Step 119: an operation that failed because another browser took the
    // databases over reports the takeover, not its symptom (a disk I/O error).
    let evicted = KupferdrahtError::Evicted;
    let e = if crate::storage::evicted_by_takeover() {
        &evicted
    } else {
        e
    };
    let error = named_error(e.variant_name(), &e.to_string());
    set_error_detail(&error, e);
    error
}

/// The detail an error carries beyond its name (ADR-0035): the `reason` of
/// `InvalidRecoveryData`, the `attemptsRemaining` of `WrongPin`, the
/// `retryAfter` (seconds) of `TooManyAttempts` and `RateLimited` (Step
/// 122) — each only when known.
fn set_error_detail(error: &JsValue, e: &KupferdrahtError) {
    let (key, value) = match e {
        KupferdrahtError::InvalidRecoveryData { reason } => ("reason", JsValue::from_str(reason)),
        KupferdrahtError::WrongPin {
            attempts_remaining: Some(attempts),
        } => ("attemptsRemaining", JsValue::from(*attempts)),
        KupferdrahtError::TooManyAttempts {
            retry_after_secs: Some(secs),
        }
        | KupferdrahtError::RateLimited {
            retry_after_secs: Some(secs),
        } => (
            "retryAfter",
            JsValue::from(u32::try_from(*secs).unwrap_or(u32::MAX)),
        ),
        _ => return,
    };
    // Setting a property on a fresh `Error` object cannot fail.
    let _ = js_sys::Reflect::set(error, &JsValue::from_str(key), &value);
}

/// A JS `Error` with `name` — for the binding's own failures, named like the
/// `KupferdrahtError` variant they stand for (`InvalidInput`, `Encoding`).
fn named_error(name: &str, message: &str) -> JsValue {
    let error = js_sys::Error::new(message);
    error.set_name(name);
    error.into()
}

/// Device-level entry point — the browser flavor of [`crate::manager::Kupferdraht`].
/// Owns the `IndexedDb`-backed keystore and the `SQLite3MC` storage (on the
/// storage server, or in OPFS in the local mode); open once at app start, **inside a dedicated Web Worker**, and
/// only one per origin at a time (ADR-0023).
#[wasm_bindgen]
pub struct Kupferdraht {
    inner: Arc<crate::manager::Kupferdraht>,
}

/// The options object `Kupferdraht.open` takes (`{ permitHttp?, takeOver?,
/// storage? }`; `undefined` means all defaults).
#[derive(Default, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct OpenOptions {
    /// Allow plain-HTTP server URLs (same-origin dev proxies only).
    permit_http: bool,
    /// Steal the origin's instance lease from another tab (ADR-0023).
    take_over: bool,
    /// Where the databases live (ADR-0035); `None` is the local mode.
    storage: Option<StorageOptions>,
    /// Step 117: the least severe level `onLog` receives (default
    /// `"warn"`); without `onLog` it changes nothing.
    log_level: Option<String>,
}

/// The options object `PendingLogin.registerDevice` takes (`{ newSetup?,
/// takeOver? }`).
#[derive(Default, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct RegisterDeviceOptions {
    /// A first setup of this browser for the account (ADR-0035).
    new_setup: bool,
    /// Take the account's databases over from another browser that holds
    /// them (Step 111c).
    take_over: bool,
}

/// The options object `Kupferdraht.resume` and `PendingLogin.recover` take
/// (`{ takeOver? }`).
#[derive(Default, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct TakeOverOptions {
    /// Take the account's databases over from another browser that holds
    /// them (Step 111c).
    take_over: bool,
}

/// The options object `Client.logout` takes (`{ purgeDevice?, password?,
/// purgeLocal? }`).
#[derive(Default, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct LogoutOptions {
    /// Delete this device at the backend too (needs the password) and, in
    /// the server mode, its databases there.
    purge_device: bool,
    password: Option<String>,
    /// Forget the account in this browser (Step 114).
    purge_local: bool,
}

/// Parse the options object `what` takes; `undefined` / `null` mean all
/// defaults.
fn parse_options<T: Default + serde::de::DeserializeOwned>(
    options: &JsValue,
    what: &str,
) -> Result<T, JsValue> {
    if options.is_undefined() || options.is_null() {
        return Ok(T::default());
    }
    serde_wasm_bindgen::from_value(options.clone())
        .map_err(|e| named_error("InvalidInput", &format!("{what} options: {e}")))
}

/// The browser home's base: every database is named under it
/// (`kupferdraht/global.db`, `kupferdraht/accounts/<id>/kupferdraht.db`, `…/mls.db`); the
/// local mode keeps each as `sqlite-remote-vfs-local/<home-id>/<name>`,
/// the storage server under the account's storage-login key.
const BASE_DIR: &str = "kupferdraht";

/// Where `Kupferdraht.open` keeps the databases: `{ mode: "local" }` (the
/// default) or `{ mode: "server", url, tokenExchangeUrl, pinServiceUrl? }`
/// (Step 111b; the PIN service, Step 112).
#[derive(serde::Deserialize)]
#[serde(tag = "mode", rename_all = "camelCase")]
enum StorageOptions {
    /// Only in this browser, in `IndexedDB` (the VFS's local mode).
    Local,
    /// The account databases on a storage server; `global.db` stays local.
    #[serde(rename_all = "camelCase")]
    Server {
        /// The storage server's WebSocket URL (`wss://…/v1/ws`).
        url: String,
        /// The token exchange's base URL (`https://…`).
        token_exchange_url: String,
        /// The PIN service's base URL (`https://…`) — recovery data is
        /// sealed and opened with it (Step 112); without it the recovery
        /// calls refuse with `InvalidInput`.
        #[serde(default)]
        pin_service_url: Option<String>,
    },
}

/// The recovery services of the server mode with a PIN service (Step 112;
/// the slot query beside the storage WebSocket since Step 120).
///
/// # Errors
/// `InvalidInput` for a storage URL that is not the storage WebSocket's
/// (`…/v1/ws`).
fn recovery_services(
    storage: Option<&StorageOptions>,
    permit_http: bool,
) -> Result<Option<RecoveryServices>, KupferdrahtError> {
    let Some(StorageOptions::Server {
        url,
        token_exchange_url,
        pin_service_url: Some(pin_service_url),
    }) = storage
    else {
        return Ok(None);
    };
    Ok(Some(RecoveryServices {
        exchange: TokenExchange {
            url: token_exchange_url.clone(),
            permit_http,
        },
        pin_service_url: pin_service_url.clone(),
        slot_url: crate::storage::remote::slot::slot_url(url)?,
    }))
}

/// Step 117: the app's `onLog` function in the `Kupferdraht.open` options, if
/// any (a function is not part of the serde-parsed options).
fn on_log_option(options: &JsValue) -> Option<js_sys::Function> {
    if options.is_undefined() || options.is_null() {
        return None;
    }
    js_sys::Reflect::get(options, &JsValue::from_str("onLog"))
        .ok()
        .and_then(|value| value.dyn_into::<js_sys::Function>().ok())
}

/// Step 117: hand Kupferdraht's log events at `level` (default `"warn"`) to the
/// app's `onLog` — Kupferdraht writes to no console itself.
fn set_up_logging(on_log: js_sys::Function, level: Option<&str>) -> Result<(), JsValue> {
    let level = match level {
        Some(name) => super::logging::parse_level(name).map_err(|e| to_js(&e))?,
        None => crate::logging::LogLevel::Warn,
    };
    let listener = Arc::new(super::logging::JsLogListener::new(on_log));
    crate::logging::init_logging(level, listener).map_err(|e| to_js(&e))
}

/// Call `callback` with the reason once this instance stops (ADR-0023,
/// ADR-0035).
fn report_eviction(callback: js_sys::Function) {
    wasm_bindgen_futures::spawn_local(async move {
        let reason = crate::storage::wait_for_browser_eviction().await;
        if let Err(e) = callback.call1(&JsValue::NULL, &JsValue::from_str(reason.as_str())) {
            web_sys::console::error_1(&e);
        }
    });
}

#[wasm_bindgen]
impl Kupferdraht {
    /// Open the device manager against the origin's storage: the `IndexedDb` +
    /// `WebCrypto` keystore and the `SQLite3MC` databases on sqlite-remote-vfs
    /// (Step 111a, ADR-0035; the local mode keeps them in `IndexedDB`).
    /// **Requires a dedicated Web Worker in a cross-origin isolated page**
    /// (`Cross-Origin-Opener-Policy: same-origin`,
    /// `Cross-Origin-Embedder-Policy: require-corp`): the storage blocks on a
    /// `SharedArrayBuffer`; the call fails closed otherwise, naming the
    /// requirement (no in-memory fallback). **One instance per origin**
    /// (Step 92, ADR-0023): if another tab holds the origin's lease the call
    /// fails closed ("already open in another tab"); `options.takeOver`
    /// steals the lease and, in the local mode, each database's lock
    /// instead — the other tab's instance is evicted (its
    /// `onEvicted("tab")` fires). It never takes another browser's
    /// databases on the storage server: that is `resume` / `registerDevice`
    /// with `{ takeOver: true }` (Step 111c).
    ///
    /// `options` is `{ permitHttp?: boolean, takeOver?: boolean, storage?:
    /// { mode: "local" } | { mode: "server", url: string, tokenExchangeUrl:
    /// string, pinServiceUrl?: string }, logLevel?: "error" | "warn" |
    /// "info" | "debug" | "trace", onLog?: (record) => void }` (or
    /// `undefined`). Step 117: with `onLog`, Kupferdraht hands its log events
    /// at `logLevel` (default `"warn"`) or above to it as `{ level, target,
    /// message }`, in this worker; Kupferdraht writes to no console itself, and
    /// without `onLog` it stays silent. `permitHttp` allows plain-HTTP server
    /// URLs — needed when the app talks to a Wire backend through a
    /// same-origin dev proxy on `http://localhost`; production uses HTTPS.
    /// The server mode (Step 111b) keeps each account's databases on the
    /// storage server at `url` (`wss://…/v1/ws`), with storage tokens from
    /// the token exchange at `tokenExchangeUrl`; recovery data (Step 112) is
    /// sealed and opened with the PIN service at `pinServiceUrl`; all come
    /// from the app's configuration. `onEvicted(reason)` (optional) fires once when this
    /// instance stops — `"tab"`: another tab took over; `"device"`: another
    /// browser or device took the account's databases on the storage
    /// server over; `"access"`: the token exchange no longer admits the
    /// user to the storage server; every further store operation fails as
    /// `Evicted`, and the embedder should terminate this worker.
    ///
    /// The keystore is pre-opened (async) so the synchronous `SecureKeyStore`
    /// reads the rest of the engine makes are served from memory (W2); it
    /// holds the home id, the namespace of the databases.
    pub async fn open(
        options: JsValue,
        on_evicted: Option<js_sys::Function>,
    ) -> Result<Kupferdraht, JsValue> {
        // Route Rust panics to the browser console (once) so failures show a
        // real message instead of an opaque `RuntimeError: unreachable`.
        console_error_panic_hook::set_once();
        let on_log = on_log_option(&options);
        let options: OpenOptions = parse_options(&options, "Kupferdraht.open")?;
        if let Some(on_log) = on_log {
            set_up_logging(on_log, options.log_level.as_deref())?;
        }
        let recovery = recovery_services(options.storage.as_ref(), options.permit_http)
            .map_err(|e| to_js(&e))?;
        let keystore: Arc<dyn crate::keystore::SecureKeyStore> =
            Arc::new(IndexedDbKeyStore::open().await.map_err(|e| to_js(&e))?);
        crate::storage::open_browser_storage(&keystore, options.take_over)
            .await
            .map_err(|e| to_js(&e))?;
        if let Some(StorageOptions::Server {
            url,
            token_exchange_url,
            ..
        }) = options.storage
        {
            crate::storage::use_storage_server(url, token_exchange_url, options.permit_http)
                .map_err(|e| to_js(&e))?;
        }
        if let Some(callback) = on_evicted {
            report_eviction(callback);
        }
        // Step 104: the registry key exists durably before `build` creates
        // `global.db` with it (the keystore persists in the background).
        let home = crate::storage::KupferdrahtHome::open(PathBuf::from(BASE_DIR))
            .map_err(|e| to_js(&e))?;
        crate::storage::prepare_browser_database(&home.global_db())
            .await
            .map_err(|e| to_js(&e))?;
        crate::storage::ensure_global_key_durable(&home, &keystore)
            .await
            .map_err(|e| to_js(&e))?;
        // ADR-0035: an account's keys come from its seed in the browser.
        let mut builder = crate::manager::Kupferdraht::builder()
            .base_dir(PathBuf::from(BASE_DIR))
            .keystore(keystore)
            .account_keys(AccountKeySource::Seed);
        if options.permit_http {
            builder = builder.permit_http();
        }
        if let Some(services) = recovery {
            builder = builder.recovery_services(services);
        }
        let inner = builder.build().map_err(|e| to_js(&e))?;
        Ok(Self {
            inner: Arc::new(inner),
        })
    }

    /// Whether this instance stopped — another tab or device took over, or
    /// the storage no longer admits the user (ADR-0023, ADR-0035). Once
    /// `true` every store operation fails; terminate this worker.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn evicted(&self) -> bool {
        crate::storage::browser_storage_evicted()
    }

    /// The id of this home (Step 93, ADR-0025) — `home.id` in the OPFS pool,
    /// 32 lowercase hex; every keystore id and the login cookie label carry
    /// it (one origin = one home, so it is stable across reloads).
    #[wasm_bindgen(getter, js_name = homeId)]
    #[must_use]
    pub fn home_id(&self) -> String {
        self.inner.home_id().to_owned()
    }

    /// The Wire login cookie label of this home, `kupferdraht-<home-id>` (Step 93).
    #[wasm_bindgen(getter, js_name = cookieLabel)]
    #[must_use]
    pub fn cookie_label(&self) -> String {
        self.inner.cookie_label()
    }

    /// Phase 1 of login: authenticate against a Wire server and return a
    /// [`PendingLogin`]. Call [`PendingLogin::register_device`] to finish.
    ///
    /// `websocket_url`/`title` complete the Wire [`ServerConfig`] (Wire pairs an
    /// HTTPS API host with a separate WS host); pass empty strings if unused for
    /// this flow. `second_factor_code`/`device_label` are optional.
    #[wasm_bindgen(js_name = login)]
    pub async fn login(
        &self,
        api_base_url: String,
        websocket_url: String,
        title: String,
        email: String,
        password: String,
        second_factor_code: Option<String>,
        device_label: Option<String>,
    ) -> Result<PendingLogin, JsValue> {
        let server = ServerConfig {
            api_base_url,
            websocket_url,
            title,
        };
        let inner = self
            .inner
            .login(server, email, password, second_factor_code, device_label)
            .await
            .map_err(|e| to_js(&e))?;
        Ok(PendingLogin {
            inner,
            manager: Arc::clone(&self.inner),
        })
    }

    /// SSO (Step 17β) — build the interactive initiate URL for `code`, validated
    /// against the Wire server at `api_base_url`. Open the returned URL in a
    /// browser; the post-`IdP` redirect's `zuid` cookie feeds `finalize_sso` (17b).
    #[wasm_bindgen(js_name = initiateSso)]
    pub async fn initiate_sso(
        &self,
        api_base_url: String,
        code: String,
        success_redirect: String,
        error_redirect: String,
    ) -> Result<String, JsValue> {
        let server = ServerConfig {
            api_base_url,
            websocket_url: String::new(),
            title: String::new(),
        };
        self.inner
            .initiate_sso(server, &code, &success_redirect, &error_redirect)
            .await
            .map_err(|e| to_js(&e))
    }

    /// SSO email discovery (Step 17β) — resolve the backend's default SSO code
    /// (`GET /sso/settings`) and return the interactive initiate URL. Wire v14
    /// has no per-domain discovery, so the team default is used; multi-IdP
    /// backends must use `initiateSso` with an explicit code.
    #[wasm_bindgen(js_name = initiateSsoByEmail)]
    pub async fn initiate_sso_by_email(
        &self,
        api_base_url: String,
        email: String,
        success_redirect: String,
        error_redirect: String,
    ) -> Result<String, JsValue> {
        let server = ServerConfig {
            api_base_url,
            websocket_url: String::new(),
            title: String::new(),
        };
        self.inner
            .initiate_sso_by_email(server, &email, &success_redirect, &error_redirect)
            .await
            .map_err(|e| to_js(&e))
    }

    /// SSO finalize (Step 17β) — complete login from the `zuid` cookie handed
    /// back by Wire's `/sso/finalize-login` redirect, returning a
    /// [`PendingLogin`] to drive through `registerDevice`. `websocket_url` /
    /// `title` complete the `ServerConfig` (pass empty strings if unused).
    #[wasm_bindgen(js_name = finalizeSso)]
    pub async fn finalize_sso(
        &self,
        api_base_url: String,
        websocket_url: String,
        title: String,
        sso_cookie: String,
        device_label: Option<String>,
    ) -> Result<PendingLogin, JsValue> {
        let server = ServerConfig {
            api_base_url,
            websocket_url,
            title,
        };
        let inner = self
            .inner
            .finalize_sso(server, &sso_cookie, device_label)
            .await
            .map_err(|e| to_js(&e))?;
        Ok(PendingLogin {
            inner,
            manager: Arc::clone(&self.inner),
        })
    }

    /// Resume a previously registered account from its stored session (no
    /// Wire login). In the server mode (Step 111b) the browser's login
    /// cookie renews the session first; the call throws `SessionExpired`
    /// without a valid cookie, `NothingStored` when the storage server no
    /// longer has the account's databases — log in again either way —
    /// `StorageAccessDenied` or `ServiceUnavailable` from the storage.
    /// `options` is `{ takeOver?: boolean }` (Step 111c): a database another
    /// browser holds throws `HeldElsewhere`; ask the user, then call again
    /// with `takeOver: true` — the other browser's instance stops
    /// (`onEvicted("device")` there).
    #[wasm_bindgen(js_name = resume)]
    pub async fn resume(&self, account_id: String, options: JsValue) -> Result<Client, JsValue> {
        let options: TakeOverOptions = parse_options(&options, "resume")?;
        let inner = self
            .inner
            .resume_with(&account_id, options.take_over)
            .await
            .map_err(|e| to_js(&e))?;
        Ok(Client {
            inner,
            manager: Arc::clone(&self.inner),
        })
    }

    /// Whether `pin` may become a recovery PIN (ADR-0035): `"ok"`,
    /// `"tooShort"` or `"tooCommon"`. A PIN has at least 6 characters,
    /// counted as characters after Unicode NFC, not as bytes. It may be
    /// digits only, and it may be longer and hold letters, digits and other
    /// characters; there is no upper limit a user would reach.
    /// `"tooCommon"` refuses a short list of trivial PINs such as `123456`
    /// or `000000`. So offer a text field that takes any characters — not a
    /// keypad for exactly six digits; digits first, with a hint that more
    /// is allowed.
    #[wasm_bindgen(js_name = validateNewPin)]
    #[must_use]
    pub fn validate_new_pin(&self, pin: String) -> String {
        let pin = Zeroizing::new(pin);
        crate::recovery::pin::validate_new_pin(&pin)
            .as_str()
            .to_owned()
    }

    /// The device account registry (ids the app can `resume`), as a JS array of
    /// `{ accountId, server, handle, displayName, lastActiveMs }`.
    #[wasm_bindgen(js_name = accounts)]
    pub async fn accounts(&self) -> Result<JsValue, JsValue> {
        let entries = self.inner.accounts().await.map_err(|e| to_js(&e))?;
        let view: Vec<AccountView> = entries.into_iter().map(AccountView::from).collect();
        serde_wasm_bindgen::to_value(&view).map_err(|e| named_error("Encoding", &e.to_string()))
    }
}

/// Plain, JS-serializable view of `DeviceInfo` (Step 98a). Field for field
/// the core record, so a browser consumer sees what Kotlin and Swift see.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct DeviceInfoView {
    device_id: String,
    class: Option<String>,
    label: Option<String>,
    model: Option<String>,
    registered_at: Option<String>,
    last_active: Option<String>,
    is_self: bool,
}

impl From<crate::client::DeviceInfo> for DeviceInfoView {
    fn from(d: crate::client::DeviceInfo) -> Self {
        Self {
            device_id: d.device_id,
            class: d.class,
            label: d.label,
            model: d.model,
            registered_at: d.registered_at,
            last_active: d.last_active,
            is_self: d.is_self,
        }
    }
}

/// Shared by `Client.devices` and `PendingLogin.devices`.
fn devices_to_js(devices: Vec<crate::client::DeviceInfo>) -> Result<JsValue, JsValue> {
    let view: Vec<DeviceInfoView> = devices.into_iter().map(DeviceInfoView::from).collect();
    serde_wasm_bindgen::to_value(&view).map_err(|e| named_error("Encoding", &e.to_string()))
}

/// Plain, JS-serializable view of `StorageEngineInfo`.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct StorageEngineView {
    engine: String,
    version: String,
    cipher: String,
    plaintext_header_size: u32,
    /// Step 122: `"server"` for databases on the storage server, else
    /// `"local"`.
    location: &'static str,
}

/// Plain, JS-serializable view of a device-registry account entry.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct AccountView {
    account_id: String,
    server: String,
    handle: Option<String>,
    display_name: Option<String>,
    last_active_ms: i64,
}

impl From<crate::storage::AccountEntry> for AccountView {
    fn from(e: crate::storage::AccountEntry) -> Self {
        Self {
            account_id: e.account_id,
            server: e.server,
            handle: e.handle,
            display_name: e.display_name,
            last_active_ms: e.last_active_ms,
        }
    }
}

/// Post-authentication handle — the browser flavor of
/// [`crate::manager::PendingLogin`].
#[wasm_bindgen]
pub struct PendingLogin {
    inner: Arc<crate::manager::PendingLogin>,
    /// Handed to the client it completes into (Steps 112, 114).
    manager: Arc<crate::manager::Kupferdraht>,
}

#[wasm_bindgen]
impl PendingLogin {
    /// The authenticated user's qualified Wire id.
    #[wasm_bindgen(getter, js_name = userId)]
    #[must_use]
    pub fn user_id(&self) -> String {
        self.inner.user_id().to_string()
    }

    /// The email the session was authenticated under.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn email(&self) -> String {
        self.inner.email().to_string()
    }

    /// Whether this browser holds the account's keys (ADR-0035): `"present"`
    /// — `registerDevice()` takes the native path — or `"missing"` — restore
    /// recovery data, or set up this browser with
    /// `registerDevice({ newSetup: true })`.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn keys(&self) -> String {
        match self.inner.keys() {
            KeysState::Present => "present",
            KeysState::Missing => "missing",
        }
        .to_owned()
    }

    /// With `keys` `"missing"`: whether the account has a registered browser
    /// device, so recovery data exists somewhere and would bring it back —
    /// a first setup adds another device instead. A hint, not a guarantee.
    #[wasm_bindgen(getter, js_name = recoveryExpected)]
    #[must_use]
    pub fn recovery_expected(&self) -> bool {
        self.inner.recovery_expected()
    }

    /// Phase 2: register this device and obtain a ready, resumable [`Client`].
    /// Also writes the account registry row and persists the session.
    /// It opens the account's databases — the login opened none (Step
    /// 111c). `options` is `{ newSetup?: boolean, takeOver?: boolean }`:
    /// with `keys` `"missing"` only a first setup (`newSetup: true`)
    /// proceeds, else it throws `RecoveryDataRequired`; a database another
    /// browser holds throws `HeldElsewhere` — ask, then call again with
    /// `takeOver: true`. A retry after `TooManyDevices` is the same call.
    #[wasm_bindgen(js_name = registerDevice)]
    pub async fn register_device(&self, options: JsValue) -> Result<Client, JsValue> {
        let options: RegisterDeviceOptions = parse_options(&options, "registerDevice")?;
        let inner = self
            .inner
            .register_device_with(RegisterOptions {
                new_setup: options.new_setup,
                take_over: options.take_over,
            })
            .await
            .map_err(|e| to_js(&e))?;
        Ok(Client {
            inner,
            manager: Arc::clone(&self.manager),
        })
    }

    /// With `keys` `"missing"`: restore the account from recovery data
    /// (ADR-0035) — open `data` with `pin` for this account at the PIN
    /// service, keep the account's keys in this browser, then register as
    /// `registerDevice` does. `options` is `{ takeOver?: boolean }`. A
    /// failure before the keys are recovered (`WrongPin` with
    /// `attemptsRemaining`, `TooManyAttempts` with `retryAfter`,
    /// `InvalidRecoveryData` with `reason`, `ServiceUnavailable`) leaves
    /// nothing; this login stays usable. After it the keys stay (`keys`
    /// becomes `"present"`): on `HeldElsewhere` continue with
    /// `registerDevice({ takeOver: true })` — no second PIN attempt. With
    /// `keys` `"present"` or without a `pinServiceUrl` it throws
    /// `InvalidInput`.
    #[wasm_bindgen(js_name = recover)]
    pub async fn recover(
        &self,
        data: Vec<u8>,
        pin: String,
        options: JsValue,
    ) -> Result<Client, JsValue> {
        let options: TakeOverOptions = parse_options(&options, "recover")?;
        let pin = Zeroizing::new(pin);
        let inner = self
            .inner
            .recover(
                &data,
                &pin,
                RegisterOptions {
                    new_setup: false,
                    take_over: options.take_over,
                },
            )
            .await
            .map_err(|e| to_js(&e))?;
        Ok(Client {
            inner,
            manager: Arc::clone(&self.manager),
        })
    }

    /// Abandon the login: what a first setup of it created — the account's
    /// keys and databases in this browser — is removed again; keys it
    /// recovered are forgotten here (the databases are the account's).
    #[wasm_bindgen(js_name = cancel)]
    pub fn cancel(&self) -> Result<(), JsValue> {
        self.inner.cancel().map_err(|e| to_js(&e))
    }

    /// The account's registered devices (Step 98a) — the first half of
    /// device-limit recovery. On `TooManyDevices` this handle stays valid,
    /// so the caller lists the devices here, frees a slot with
    /// [`Self::delete_device`], and calls `registerDevice` again on the same
    /// handle; no second login.
    #[wasm_bindgen(js_name = devices)]
    pub async fn devices(&self) -> Result<JsValue, JsValue> {
        devices_to_js(self.inner.devices().await.map_err(|e| to_js(&e))?)
    }

    /// Delete a device by its Wire `client_id` to free a slot (Step 98a).
    /// No password: phase 1 already authenticated this handle.
    #[wasm_bindgen(js_name = deleteDevice)]
    pub async fn delete_device(&self, device_id: String) -> Result<(), JsValue> {
        self.inner
            .delete_device(&device_id)
            .await
            .map_err(|e| to_js(&e))
    }
}

/// A logged-in, resumable client — the browser flavor of
/// [`crate::client::Client`]. Slice 1 exposes identity accessors; sync + send
/// land in later slices.
#[wasm_bindgen]
pub struct Client {
    inner: Arc<crate::client::Client>,
    /// The manager: what `recovery()` and a purging `logout` need of it
    /// (Steps 112, 114).
    manager: Arc<crate::manager::Kupferdraht>,
}

#[wasm_bindgen]
impl Client {
    /// The logged-in user's qualified Wire id, or `undefined` before identity
    /// is established.
    #[wasm_bindgen(getter, js_name = userId)]
    #[must_use]
    pub fn user_id(&self) -> Option<String> {
        self.inner.user_id().map(|u| u.as_str().to_string())
    }

    /// The account's email address, or `undefined` when the account has
    /// none on record (Step 122).
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn email(&self) -> Option<String> {
        self.inner.account().email().map(str::to_owned)
    }

    /// The account's handle, or `undefined` when the account has none on
    /// record (Step 125) — as the session knows it from its login, like
    /// `displayName`: a `setHandle` in this session does not change it. A
    /// consumer needs no `GET /self` of its own for it.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn handle(&self) -> Option<String> {
        self.inner.account().handle().map(str::to_owned)
    }

    /// Whether sync runs on push (Step 122): `"connected"` while the sync
    /// loop has an open socket, `"polling"` while it runs without one,
    /// `"stopped"` without a running loop (`startSync`).
    #[wasm_bindgen(js_name = pushState)]
    #[must_use]
    pub fn push_state(&self) -> String {
        self.inner
            .user_id()
            .map_or("stopped", |user| {
                super::push_state::push_state(user.as_str())
            })
            .to_owned()
    }

    /// The account's display name (empty string if unset).
    #[wasm_bindgen(getter, js_name = displayName)]
    #[must_use]
    pub fn display_name(&self) -> String {
        self.inner.account().display_name().to_string()
    }

    /// Whether this login's device replaced the one recorded for the
    /// account, because the backend no longer had it: older messages of
    /// that device are gone (ADR-0035).
    #[wasm_bindgen(getter, js_name = deviceReplaced)]
    #[must_use]
    pub fn device_replaced(&self) -> bool {
        self.inner.device_replaced()
    }

    /// Whether the account's storage takes operations (Step 113, ADR-0038):
    /// `"online"`, or `"reconnecting"` while the storage server is
    /// unreachable — operations fail with `ServiceUnavailable` meanwhile,
    /// and the storage heals by itself once the server answers again.
    /// Always `"online"` in the local mode. `startSync`'s `onStorage` reports
    /// each change.
    #[wasm_bindgen(getter, js_name = storageState)]
    #[must_use]
    pub fn storage_state(&self) -> String {
        self.inner
            .user_id()
            .map_or(crate::storage::StorageState::Online, |user| {
                crate::storage::account_storage_state(user.as_str())
            })
            .as_str()
            .to_owned()
    }

    /// What the app-data store's whole-DB encryption engine reports about
    /// itself, read off the live connection (Step 90 / 91): `{ engine,
    /// version, cipher, plaintextHeaderSize, location }`, or `undefined` for
    /// a backend without such an engine. `location` (Step 122) is
    /// `"server"` for databases on the storage server, else `"local"`. The
    /// browser flavor of the native `Client::storage_engine` (FFI-mirrored
    /// on the `Client` object).
    #[wasm_bindgen(js_name = storageEngine)]
    pub async fn storage_engine(&self) -> Result<JsValue, JsValue> {
        let info = self.inner.storage_engine().await.map_err(|e| to_js(&e))?;
        let on_server = self
            .inner
            .user_id()
            .is_some_and(|user| crate::storage::account_on_storage_server(user.as_str()));
        let view = info.map(|i| StorageEngineView {
            engine: i.engine,
            version: i.version,
            cipher: i.cipher,
            plaintext_header_size: i.plaintext_header_size,
            location: if on_server { "server" } else { "local" },
        });
        serde_wasm_bindgen::to_value(&view).map_err(|e| named_error("Encoding", &e.to_string()))
    }

    /// Start a background sync loop (W4) that pushes each incoming message to
    /// `onMessage` (`{ roomId, senderUserId, messageId, body, replyToId,
    /// receivedAtMs }`). `onTyping` (optional) fires per typing event
    /// (`{ roomId, typingUsers }`); `onExpired` (optional) fires once if the
    /// session's refresh token dies; `onSync` (optional, Step 94) fires after
    /// every successful pass with `{ roomsUpdated, eventsDispatched }` — the
    /// signal a thin tab re-pulls the room list on; `onDevicesChanged`
    /// (optional, Step 95d) fires per room whose MLS device set changed
    /// with `{ roomId, added, removed }` (devices as in `roomDevices`);
    /// `onStorage` (optional, Step 113) fires with `"reconnecting"` /
    /// `"online"` when the account's storage state changes (see
    /// `storageState`). Returns a handle whose `.stop()` ends them all.
    #[wasm_bindgen(js_name = startSync)]
    #[must_use]
    pub fn start_sync(
        &self,
        on_message: js_sys::Function,
        on_typing: Option<js_sys::Function>,
        on_expired: Option<js_sys::Function>,
        on_sync: Option<js_sys::Function>,
        on_devices_changed: Option<js_sys::Function>,
        on_storage: Option<js_sys::Function>,
    ) -> super::SyncHandle {
        super::live_sync::start(
            self.inner.clone(),
            super::live_sync::Callbacks {
                message: on_message,
                typing: on_typing,
                expired: on_expired,
                sync: on_sync,
                devices_changed: on_devices_changed,
                storage: on_storage,
            },
        )
    }

    /// The devices of `roomId` (Step 95d) as `{ userId, deviceId,
    /// credential, thumbprint }[]` — one per MLS leaf; `thumbprint` is
    /// Wire's "MLS Thumbprint".
    #[wasm_bindgen(js_name = roomDevices)]
    pub async fn room_devices(&self, room_id: String) -> Result<JsValue, JsValue> {
        let room = self
            .inner
            .room(&room_id)
            .await
            .ok_or_else(|| named_error("InvalidInput", &format!("unknown room: {room_id}")))?;
        let view: Vec<super::devices::DeviceView> = room
            .devices()
            .await
            .map_err(|e| to_js(&e))?
            .iter()
            .map(super::devices::DeviceView::from)
            .collect();
        serde_wasm_bindgen::to_value(&view).map_err(|e| named_error("Encoding", &e.to_string()))
    }

    /// This device's MLS thumbprint (Step 95d) — compare it with the
    /// "MLS Thumbprint" an official Wire app shows for this device.
    #[wasm_bindgen(js_name = ownDeviceThumbprint)]
    pub fn own_device_thumbprint(&self) -> Result<String, JsValue> {
        self.inner.own_device_thumbprint().map_err(|e| to_js(&e))
    }

    /// Notify the room that this user started typing (W4). The receiver gets it
    /// via `startSync`'s `onTyping` callback.
    #[wasm_bindgen(js_name = startTyping)]
    pub async fn start_typing(&self, room_id: String) -> Result<(), JsValue> {
        self.inner
            .send_typing_started(&room_id)
            .await
            .map_err(|e| to_js(&e))
    }

    /// Notify the room that this user stopped typing (W4).
    #[wasm_bindgen(js_name = stopTyping)]
    pub async fn stop_typing(&self, room_id: String) -> Result<(), JsValue> {
        self.inner
            .send_typing_stopped(&room_id)
            .await
            .map_err(|e| to_js(&e))
    }

    /// Run one sync pass — drain notifications + refresh the conversations list,
    /// applying everything to the account's store. Returns
    /// `{ roomsUpdated, eventsDispatched }`.
    #[wasm_bindgen(js_name = syncOnce)]
    pub async fn sync_once(&self) -> Result<JsValue, JsValue> {
        let summary = self
            .inner
            .sync_once(crate::sync::SyncSettings::default())
            .await
            .map_err(|e| to_js(&e))?;
        let view = SyncSummaryView {
            rooms_updated: summary.rooms_updated,
            events_dispatched: summary.events_dispatched,
        };
        serde_wasm_bindgen::to_value(&view).map_err(|e| named_error("Encoding", &e.to_string()))
    }

    /// The rooms known to this client, read from the store, as
    /// `{ roomId, name }[]`. After a `syncOnce` these are backed by the
    /// account's `SQLite3MC` store, so they survive a reload.
    #[wasm_bindgen(js_name = rooms)]
    pub async fn rooms(&self) -> Result<JsValue, JsValue> {
        let view: Vec<RoomView> = self
            .inner
            .rooms()
            .await
            .iter()
            .map(|r| RoomView {
                room_id: r.id(),
                name: r.name(),
            })
            .collect();
        serde_wasm_bindgen::to_value(&view).map_err(|e| named_error("Encoding", &e.to_string()))
    }

    /// Send a text message to `roomId`, joining the room's MLS group first if
    /// this client hasn't yet (W3). Returns `{ messageId, sentAtMs }`.
    #[wasm_bindgen(js_name = sendText)]
    pub async fn send_text(&self, room_id: String, body: String) -> Result<JsValue, JsValue> {
        let timeline = self.timeline_for(&room_id).await?;
        let handle = timeline
            .send_text_auto_join(body)
            .await
            .map_err(|e| to_js(&e))?;
        message_handle_to_js(&handle)
    }

    /// The (locally stored, already-decrypted) messages of `roomId`, oldest
    /// first, newest `limit` items. Projects each `TimelineItem` to a plain JS
    /// object; non-text kinds carry `body: null` + a `kind` label.
    #[wasm_bindgen(js_name = messages)]
    pub async fn messages(
        &self,
        room_id: String,
        limit: Option<u32>,
        before: Option<String>,
    ) -> Result<JsValue, JsValue> {
        let room = self
            .inner
            .room(&room_id)
            .await
            .ok_or_else(|| named_error("InvalidInput", &format!("unknown room: {room_id}")))?;
        let mut opts = crate::timeline::MessagesOptions::all();
        if let Some(n) = limit {
            opts = opts.with_limit(n);
        }
        // Step 98c: the cursor. Without it a browser can only ever read the
        // newest page of a conversation, however long it is.
        opts.before = before;
        let items = room.messages(&opts).await.map_err(|e| to_js(&e))?;
        let me = self.inner.user_id().map(ToString::to_string);
        let view: Vec<TimelineItemView> = items
            .iter()
            .map(|it| TimelineItemView::project(it, me.as_deref()))
            .collect();
        serde_wasm_bindgen::to_value(&view).map_err(|e| named_error("Encoding", &e.to_string()))
    }

    /// Send a knock ("ping") to `roomId` (Step 98c). `hotKnock` marks a
    /// repeat nudge. Returns `{ messageId, sentAtMs }`.
    #[wasm_bindgen(js_name = sendKnock)]
    pub async fn send_knock(&self, room_id: String, hot_knock: bool) -> Result<JsValue, JsValue> {
        let timeline = self.timeline_for(&room_id).await?;
        let handle = timeline
            .send_knock(hot_knock)
            .await
            .map_err(|e| to_js(&e))?;
        message_handle_to_js(&handle)
    }

    /// Share a map point in `roomId` (Step 98c). `name` is an optional label,
    /// `zoom` an optional map zoom level. Returns `{ messageId, sentAtMs }`.
    #[wasm_bindgen(js_name = sendLocation)]
    pub async fn send_location(
        &self,
        room_id: String,
        latitude: f32,
        longitude: f32,
        name: Option<String>,
        zoom: Option<i32>,
    ) -> Result<JsValue, JsValue> {
        let timeline = self.timeline_for(&room_id).await?;
        let handle = timeline
            .send_location(latitude, longitude, name, zoom)
            .await
            .map_err(|e| to_js(&e))?;
        message_handle_to_js(&handle)
    }

    /// Send a text reply to `parentId` in `roomId`. Returns `{ messageId, sentAtMs }`.
    #[wasm_bindgen(js_name = sendReply)]
    pub async fn send_reply(
        &self,
        room_id: String,
        parent_id: String,
        body: String,
    ) -> Result<JsValue, JsValue> {
        let timeline = self.timeline_for(&room_id).await?;
        let handle = timeline
            .send_reply(parent_id, body)
            .await
            .map_err(|e| to_js(&e))?;
        message_handle_to_js(&handle)
    }

    /// Toggle an emoji reaction on `itemId` in `roomId` (adds if absent, removes
    /// if already reacted by this user). Returns `{ messageId, sentAtMs }`.
    #[wasm_bindgen(js_name = toggleReaction)]
    pub async fn toggle_reaction(
        &self,
        room_id: String,
        item_id: String,
        emoji: String,
    ) -> Result<JsValue, JsValue> {
        let timeline = self.timeline_for(&room_id).await?;
        let handle = timeline
            .toggle_reaction(item_id, emoji)
            .await
            .map_err(|e| to_js(&e))?;
        message_handle_to_js(&handle)
    }

    /// Edit `itemId`'s text in `roomId`. Returns `{ messageId, sentAtMs }` (of the
    /// edit event).
    #[wasm_bindgen(js_name = editMessage)]
    pub async fn edit_message(
        &self,
        room_id: String,
        item_id: String,
        new_body: String,
    ) -> Result<JsValue, JsValue> {
        let timeline = self.timeline_for(&room_id).await?;
        let handle = timeline
            .edit(item_id, new_body)
            .await
            .map_err(|e| to_js(&e))?;
        message_handle_to_js(&handle)
    }

    /// Delete (redact) `itemId` in `roomId` for everyone.
    #[wasm_bindgen(js_name = redact)]
    pub async fn redact(&self, room_id: String, item_id: String) -> Result<JsValue, JsValue> {
        let timeline = self.timeline_for(&room_id).await?;
        let handle = timeline.redact(&item_id).await.map_err(|e| to_js(&e))?;
        message_handle_to_js(&handle)
    }

    /// Mark `itemId` (and everything before it) as read in `roomId`.
    #[wasm_bindgen(js_name = markAsRead)]
    pub async fn mark_as_read(&self, room_id: String, item_id: String) -> Result<(), JsValue> {
        let timeline = self.timeline_for(&room_id).await?;
        timeline
            .mark_as_read(item_id)
            .await
            .map_err(|e| to_js(&e))?;
        Ok(())
    }

    /// Create a new group conversation named `name`. Returns `{ roomId, name }`.
    #[wasm_bindgen(js_name = createRoom)]
    pub async fn create_room(&self, name: String) -> Result<JsValue, JsValue> {
        let room = self.inner.create_room(&name).await.map_err(|e| to_js(&e))?;
        room_to_js(&room)
    }

    /// Open (or reuse) a 1:1 conversation with the qualified `peer` user id.
    /// Returns `{ roomId, name }`.
    #[wasm_bindgen(js_name = createDm)]
    pub async fn create_dm(&self, peer: String) -> Result<JsValue, JsValue> {
        let room = self.inner.create_dm(&peer).await.map_err(|e| to_js(&e))?;
        room_to_js(&room)
    }

    /// The members of `roomId` as `{ userId, isSelf, displayName, handle }[]`.
    #[wasm_bindgen(js_name = members)]
    pub async fn members(&self, room_id: String) -> Result<JsValue, JsValue> {
        let room = self
            .inner
            .room(&room_id)
            .await
            .ok_or_else(|| named_error("InvalidInput", &format!("unknown room: {room_id}")))?;
        let view: Vec<MemberView> = room
            .members()
            .await
            .map_err(|e| to_js(&e))?
            .iter()
            .map(MemberView::from)
            .collect();
        serde_wasm_bindgen::to_value(&view).map_err(|e| named_error("Encoding", &e.to_string()))
    }

    /// Add `userIds` (qualified ids) to `roomId`'s MLS group. Returns the
    /// refreshed `{ roomId, name }`.
    #[wasm_bindgen(js_name = invite)]
    pub async fn invite(&self, room_id: String, user_ids: Vec<String>) -> Result<JsValue, JsValue> {
        let room = self
            .inner
            .invite_members(&room_id, &user_ids)
            .await
            .map_err(|e| to_js(&e))?;
        room_to_js(&room)
    }

    /// Remove `userIds` from `roomId`'s MLS group.
    #[wasm_bindgen(js_name = kick)]
    pub async fn kick(&self, room_id: String, user_ids: Vec<String>) -> Result<JsValue, JsValue> {
        let room = self
            .inner
            .kick_members(&room_id, &user_ids)
            .await
            .map_err(|e| to_js(&e))?;
        room_to_js(&room)
    }

    /// Rename `roomId` (Step 98b). Returns the updated room.
    #[wasm_bindgen(js_name = renameRoom)]
    pub async fn rename_room(&self, room_id: String, name: String) -> Result<JsValue, JsValue> {
        let room = self
            .inner
            .rename_room(&room_id, &name)
            .await
            .map_err(|e| to_js(&e))?;
        room_to_js(&room)
    }

    /// Delete `roomId` for everyone (Step 98b) — team-admin only. A room
    /// that is already gone counts as success.
    #[wasm_bindgen(js_name = deleteRoom)]
    pub async fn delete_room(&self, room_id: String) -> Result<(), JsValue> {
        self.inner
            .delete_room(&room_id)
            .await
            .map_err(|e| to_js(&e))
    }

    /// `roomId`'s access modes and roles (Step 98b), as `{modes, roles}`.
    #[wasm_bindgen(js_name = roomAccess)]
    pub async fn room_access(&self, room_id: String) -> Result<JsValue, JsValue> {
        let room = self
            .inner
            .room(&room_id)
            .await
            .ok_or_else(|| named_error("InvalidInput", &format!("unknown room: {room_id}")))?;
        let access = room.access().await.map_err(|e| to_js(&e))?;
        let view = RoomAccessView {
            modes: access.modes,
            roles: access.roles,
        };
        serde_wasm_bindgen::to_value(&view).map_err(|e| named_error("Encoding", &e.to_string()))
    }

    /// Set `roomId`'s access modes and roles (Step 98b).
    #[wasm_bindgen(js_name = setRoomAccess)]
    pub async fn set_room_access(
        &self,
        room_id: String,
        modes: Vec<String>,
        roles: Vec<String>,
    ) -> Result<JsValue, JsValue> {
        let room = self
            .inner
            .set_room_access(&room_id, &modes, &roles)
            .await
            .map_err(|e| to_js(&e))?;
        room_to_js(&room)
    }

    /// Open `roomId` to guest-link joins (Step 98b).
    ///
    /// **The prerequisite for [`Self::create_join_link`].** A Kupferdraht-created
    /// conversation defaults to `access=["invite"]`, against which Wire
    /// answers `403 access-denied` to a code request. Delegates to the
    /// core's `Room::open_to_guests` (Step 8b), the one place that owns the
    /// canonical guest configuration.
    #[wasm_bindgen(js_name = allowGuests)]
    pub async fn allow_guests(&self, room_id: String) -> Result<JsValue, JsValue> {
        let room = self
            .inner
            .room(&room_id)
            .await
            .ok_or_else(|| named_error("InvalidInput", &format!("unknown room: {room_id}")))?;
        let room = room.open_to_guests().await.map_err(|e| to_js(&e))?;
        room_to_js(&room)
    }

    /// Issue (or fetch) `roomId`'s reusable guest link (Step 98b).
    /// `password` protects the link; omit it for an open one.
    ///
    /// Needs the room to permit codes — call [`Self::allow_guests`] once
    /// first, or Wire answers `403 access-denied`.
    #[wasm_bindgen(js_name = createJoinLink)]
    pub async fn create_join_link(
        &self,
        room_id: String,
        password: Option<String>,
    ) -> Result<JsValue, JsValue> {
        let link = self
            .inner
            .create_join_link(&room_id, password.as_deref())
            .await
            .map_err(|e| to_js(&e))?;
        let view = JoinLinkView {
            url: link.url,
            password_protected: link.password_protected,
        };
        serde_wasm_bindgen::to_value(&view).map_err(|e| named_error("Encoding", &e.to_string()))
    }

    /// Revoke `roomId`'s guest link (Step 98b).
    #[wasm_bindgen(js_name = revokeJoinLink)]
    pub async fn revoke_join_link(&self, room_id: String) -> Result<(), JsValue> {
        self.inner
            .revoke_join_link(&room_id)
            .await
            .map_err(|e| to_js(&e))
    }

    /// Redeem a guest link and join the conversation it names (Step 98b;
    /// Step 122: no domain — the link names the conversation).
    ///
    /// `code` and `key` come from the link URL's query. This is an MLS
    /// external commit, not a REST call — which is why a browser could never
    /// stand in for it.
    #[wasm_bindgen(js_name = joinViaLink)]
    pub async fn join_via_link(
        &self,
        code: String,
        key: String,
        password: Option<String>,
    ) -> Result<JsValue, JsValue> {
        let room = self
            .inner
            .join_via_link(code, key, password)
            .await
            .map_err(|e| to_js(&e))?;
        room_to_js(&room)
    }

    /// Leave `roomId` (server-driven MLS remove + local group discard).
    #[wasm_bindgen(js_name = leave)]
    pub async fn leave(&self, room_id: String) -> Result<(), JsValue> {
        self.inner.leave_room(&room_id).await.map_err(|e| to_js(&e))
    }

    /// Set the user's display name (Step 98a).
    #[wasm_bindgen(js_name = setDisplayName)]
    pub async fn set_display_name(&self, name: String) -> Result<(), JsValue> {
        self.inner
            .set_display_name(&name)
            .await
            .map_err(|e| to_js(&e))
    }

    /// Set the user's globally unique `@handle` (Step 98a).
    #[wasm_bindgen(js_name = setHandle)]
    pub async fn set_handle(&self, handle: String) -> Result<(), JsValue> {
        self.inner.set_handle(&handle).await.map_err(|e| to_js(&e))
    }

    /// Request a change of the account email (Step 98a). Wire sends a
    /// verification mail; the address only changes once it is confirmed.
    #[wasm_bindgen(js_name = updateEmail)]
    pub async fn update_email(&self, email: String) -> Result<(), JsValue> {
        self.inner.update_email(&email).await.map_err(|e| to_js(&e))
    }

    /// The account's registered devices (Step 98a).
    #[wasm_bindgen(js_name = devices)]
    pub async fn devices(&self) -> Result<JsValue, JsValue> {
        devices_to_js(self.inner.devices().await.map_err(|e| to_js(&e))?)
    }

    /// Delete a device that is not this one (Step 98a); Wire requires the
    /// account password to re-authenticate.
    #[wasm_bindgen(js_name = deleteDevice)]
    pub async fn delete_device(&self, device_id: String, password: String) -> Result<(), JsValue> {
        self.inner
            .delete_device(&device_id, &password)
            .await
            .map_err(|e| to_js(&e))
    }

    /// The team's server-administered feature configs (Step 98d), as
    /// `{name, status, configJson, lastSyncedAtMs}[]`. Read from the local
    /// store, refreshed at login and by events — no network call here.
    #[wasm_bindgen(js_name = featureConfigs)]
    pub async fn feature_configs(&self) -> Result<JsValue, JsValue> {
        let entries = self.inner.feature_configs().await.map_err(|e| to_js(&e))?;
        let view: Vec<FeatureConfigView> = entries
            .into_iter()
            .map(|e| FeatureConfigView {
                name: e.name,
                status: e.status,
                config_json: e.config_json,
                last_synced_at_ms: e.last_synced_at_ms,
            })
            .collect();
        serde_wasm_bindgen::to_value(&view).map_err(|e| named_error("Encoding", &e.to_string()))
    }

    /// The team's file-sharing policy (Step 98d). `policy` is
    /// `"disabled" | "enabled"`.
    #[wasm_bindgen(js_name = fileSharing)]
    pub async fn file_sharing(&self) -> Result<JsValue, JsValue> {
        use crate::client::FileSharingPolicy as P;
        let view = match self.inner.file_sharing().await.map_err(|e| to_js(&e))? {
            P::Disabled => FileSharingView { policy: "disabled" },
            P::Enabled => FileSharingView { policy: "enabled" },
        };
        serde_wasm_bindgen::to_value(&view).map_err(|e| named_error("Encoding", &e.to_string()))
    }

    /// The account's recovery data (ADR-0035): its state, the first data,
    /// a new PIN, the local PIN check.
    #[wasm_bindgen(js_name = recovery)]
    #[must_use]
    pub fn recovery(&self) -> Recovery {
        Recovery {
            client: self.inner.clone(),
            context: self.manager.recovery_context(),
        }
    }

    /// Revoke the session server-side (`POST /access/logout`); the browser
    /// cookie jar's `zuid` is invalidated by Wire. `options` is
    /// `{ purgeDevice?: boolean, password?: string, purgeLocal?: boolean }`
    /// (ADR-0035): without options the account's keys and data stay, so the
    /// next login needs no recovery data; `purgeDevice` also deletes this
    /// device at the backend (the password is required) and, in the server
    /// mode, its databases there; `purgeLocal` forgets the account in this
    /// browser — its keys, local databases and registry entry (recovery data
    /// still restores it). Both together leave a browser for good. The
    /// backend comes first: a refused password changes nothing here. After
    /// a purge the client is closed — drop it.
    #[wasm_bindgen(js_name = logout)]
    pub async fn logout(&self, options: JsValue) -> Result<(), JsValue> {
        let options: LogoutOptions = parse_options(&options, "logout")?;
        let password = options.password.map(Zeroizing::new);
        let purge = crate::manager::PurgeOptions {
            purge_device: options.purge_device,
            purge_local: options.purge_local,
        };
        self.manager
            .logout_client(&self.inner, purge, password.as_ref().map(|p| p.as_str()))
            .await
            .map_err(|e| to_js(&e))
    }

    /// Send `bytes` as an encrypted file/image asset to `roomId` (W7, in-memory).
    /// Returns `{ messageId, sentAtMs }`.
    #[wasm_bindgen(js_name = sendFile)]
    pub async fn send_file(
        &self,
        room_id: String,
        bytes: Vec<u8>,
        filename: String,
        mime_type: String,
    ) -> Result<JsValue, JsValue> {
        let handle = self
            .inner
            .send_bytes(&room_id, &bytes, &filename, &mime_type)
            .await
            .map_err(|e| to_js(&e))?;
        message_handle_to_js(&handle)
    }

    /// Download + decrypt the asset attached to `itemId` in `roomId`, returning
    /// the plaintext bytes (W7). Verifies SHA-256 before returning.
    #[wasm_bindgen(js_name = downloadAsset)]
    pub async fn download_asset(
        &self,
        room_id: String,
        item_id: String,
    ) -> Result<Vec<u8>, JsValue> {
        let room = self
            .inner
            .room(&room_id)
            .await
            .ok_or_else(|| named_error("InvalidInput", &format!("unknown room: {room_id}")))?;
        let items = room
            .messages(&crate::timeline::MessagesOptions::all())
            .await
            .map_err(|e| to_js(&e))?;
        let summary = items
            .iter()
            .find_map(|it| match &it.kind {
                crate::timeline::TimelineItemKind::Asset(s) if it.item_id == item_id => {
                    Some(s.as_ref())
                }
                _ => None,
            })
            .ok_or_else(|| named_error("InvalidInput", &format!("no asset for item {item_id}")))?;
        summary.download().await.map_err(|e| to_js(&e))
    }
}

impl Client {
    /// Resolve a room's `Timeline`, erroring if the room is unknown.
    async fn timeline_for(
        &self,
        room_id: &str,
    ) -> Result<std::sync::Arc<crate::timeline::Timeline>, JsValue> {
        let room = self
            .inner
            .room(room_id)
            .await
            .ok_or_else(|| named_error("InvalidInput", &format!("unknown room: {room_id}")))?;
        room.timeline().map_err(|e| to_js(&e))
    }
}

/// Marshal a `MessageHandle` to `{ messageId, sentAtMs }`.
fn message_handle_to_js(handle: &crate::timeline::MessageHandle) -> Result<JsValue, JsValue> {
    let view = MessageHandleView {
        message_id: handle.message_id.clone(),
        sent_at_ms: handle.sent_at_ms,
    };
    serde_wasm_bindgen::to_value(&view).map_err(|e| named_error("Encoding", &e.to_string()))
}

/// Marshal a `Room` to `{ roomId, name }`.
fn room_to_js(room: &crate::rooms::Room) -> Result<JsValue, JsValue> {
    let view = RoomView {
        room_id: room.id(),
        name: room.name(),
    };
    serde_wasm_bindgen::to_value(&view).map_err(|e| named_error("Encoding", &e.to_string()))
}

/// Plain, JS-serializable view of `RoomAccess` (Step 98b).
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct RoomAccessView {
    modes: Vec<String>,
    roles: Vec<String>,
}

/// Plain, JS-serializable view of `JoinLink` (Step 98b). `passwordProtected`
/// is carried so a consumer can tell an open link from a protected one.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct JoinLinkView {
    url: String,
    password_protected: bool,
}

/// Plain, JS-serializable view of a server-administered feature config
/// (Step 98d). `config_json` stays raw: Wire's per-feature shapes differ,
/// and reshaping them here would age badly.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct FeatureConfigView {
    name: String,
    status: Option<String>,
    config_json: String,
    last_synced_at_ms: i64,
}

/// Plain, JS-serializable view of `FileSharingPolicy` (Step 98d): an
/// object with a `policy` string rather than a bare boolean, so a later
/// field can join without changing the shape.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct FileSharingView {
    policy: &'static str,
}

/// Plain, JS-serializable view of a room member.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct MemberView {
    user_id: String,
    is_self: bool,
    display_name: Option<String>,
    handle: Option<String>,
    /// `"administrator"` | `"member"` (Step 72a — same neutral
    /// vocabulary the native FFI mirrors via `RoomMemberRole`).
    role: String,
}

impl From<&crate::rooms::RoomMember> for MemberView {
    fn from(m: &crate::rooms::RoomMember) -> Self {
        Self {
            user_id: m.user_id.clone(),
            is_self: m.is_self,
            display_name: m.display_name.clone(),
            handle: m.handle.clone(),
            role: m.role.as_db_str().to_string(),
        }
    }
}

/// Plain, JS-serializable view of a `sync_once` outcome.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SyncSummaryView {
    rooms_updated: u32,
    events_dispatched: u32,
}

/// Plain, JS-serializable view of a room.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct RoomView {
    room_id: String,
    name: String,
}

/// Plain, JS-serializable view of a `MessageHandle`.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct MessageHandleView {
    message_id: String,
    sent_at_ms: i64,
}

/// Plain, JS-serializable projection of a `TimelineItem` — like `ffi`'s twin,
/// this strips the non-serializable boxed asset payload. Text bodies come
/// through directly; other kinds carry `body: None` + a `kind` label.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct TimelineItemView {
    item_id: String,
    sender_user_id: String,
    sent_at_ms: i64,
    kind: &'static str,
    body: Option<String>,
    reply_to: Option<String>,
    reactions: Vec<ReactionView>,
    edited: bool,
    redacted: bool,
    /// Present for `kind == "asset"` — the file metadata; download the bytes via
    /// `Client.downloadAsset(roomId, itemId)`.
    asset: Option<AssetMeta>,
    /// Step 89: set only for a self-deleting message (ms since Unix epoch).
    expires_at_ms: Option<i64>,
    /// Step 98c: `"sending" | "submitted" | "received" | "failed"` — whether
    /// the item was sent locally and how far it got, or arrived via sync.
    /// Without it a UI cannot tell a sent message from a failed one.
    delivery_status: &'static str,
    /// Present for `kind == "knock"` — whether it was a repeat nudge.
    hot_knock: Option<bool>,
    /// Present for `kind == "location"` — the shared map point.
    location: Option<LocationView>,
}

/// Plain, JS-serializable view of a `Location` timeline item (Step 98c).
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct LocationView {
    latitude: f32,
    longitude: f32,
    name: Option<String>,
    zoom: Option<i32>,
}

/// An emoji reaction group on a timeline item.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ReactionView {
    emoji: String,
    count: u32,
    /// Step 98c: whether the local user is among this group's senders. The
    /// core aggregates `{emoji, senders}`; a bare count cannot express it.
    is_self: bool,
}

/// File metadata for an asset timeline item.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct AssetMeta {
    filename: Option<String>,
    mime: String,
    size: f64,
}

impl TimelineItemView {
    /// Project a core item for JS. `me` is the local user's qualified id,
    /// needed for `ReactionView::is_self`; `None` degrades that flag to
    /// `false` rather than guessing.
    #[allow(clippy::cast_precision_loss)]
    fn project(it: &crate::timeline::TimelineItem, me: Option<&str>) -> Self {
        use crate::timeline::TimelineItemKind as K;
        let mut asset = None;
        let (kind, body, reply_to) = match &it.kind {
            K::Text { body, reply_to, .. } => ("text", Some(body.clone()), reply_to.clone()),
            K::Redacted => ("redacted", None, None),
            K::Asset(s) => {
                asset = Some(AssetMeta {
                    filename: s.filename.clone(),
                    mime: s.mime_type.clone(),
                    size: s.size_bytes as f64,
                });
                ("asset", None, None)
            }
            K::Knock { .. } => ("knock", None, None),
            K::Location { .. } => ("location", None, None),
            // Task (Step 60a) and Case (Step 72b) render as bare kind
            // markers in the JS view for now — the in-flight WASM
            // rework owns richer projections. Their absence kept the
            // wasm target from compiling at all (gap-B.25).
            K::Task(_) => ("task", None, None),
            K::Case(_) => ("case", None, None),
            // Task-list marker (Step 75, ADR-0020): a bare kind marker like
            // Task/Case; the folded room flag is `Room::is_task_list()`.
            // Its absence kept the wasm target from compiling (gap-B.28).
            K::TaskListMarker { .. } => ("task_list_marker", None, None),
        };
        // Step 98c: the two variants the projection used to flatten away.
        let (hot_knock, location) = knock_and_location(&it.kind);
        let reactions = it
            .reactions
            .iter()
            .map(|r| ReactionView {
                emoji: r.emoji.clone(),
                count: u32::try_from(r.senders.len()).unwrap_or(u32::MAX),
                is_self: me.is_some_and(|me| r.senders.iter().any(|s| s == me)),
            })
            .collect();
        Self {
            item_id: it.item_id.clone(),
            sender_user_id: it.sender_user_id.clone(),
            sent_at_ms: it.sent_at_ms,
            kind,
            body,
            reply_to,
            reactions,
            edited: it.edited_at_ms.is_some(),
            redacted: it.redacted,
            asset,
            expires_at_ms: it.expires_at_ms,
            hot_knock,
            location,
            delivery_status: delivery_status_label(it.delivery_status),
        }
    }
}

/// The payload of a knock or a location item (Step 98c); `(None, None)`
/// for every other kind.
fn knock_and_location(
    kind: &crate::timeline::TimelineItemKind,
) -> (Option<bool>, Option<LocationView>) {
    use crate::timeline::TimelineItemKind as K;
    match kind {
        K::Knock { hot_knock } => (Some(*hot_knock), None),
        K::Location {
            latitude,
            longitude,
            name,
            zoom,
        } => (
            None,
            Some(LocationView {
                latitude: *latitude,
                longitude: *longitude,
                name: name.clone(),
                zoom: *zoom,
            }),
        ),
        _ => (None, None),
    }
}

/// The JS label of a `DeliveryStatus` (Step 98c).
fn delivery_status_label(status: crate::timeline::DeliveryStatus) -> &'static str {
    use crate::timeline::DeliveryStatus as D;
    match status {
        D::Sending => "sending",
        D::Submitted => "submitted",
        D::Received => "received",
        D::Failed => "failed",
    }
}

/// An account's recovery data (Step 112, ADR-0035) — from
/// `Client.recovery()`. The data is opaque bytes the embedder keeps (a
/// download, a file); the seed never leaves Kupferdraht. In the local mode, or
/// without a `pinServiceUrl`, `state()` is disabled and the rest throws
/// `InvalidInput`.
#[wasm_bindgen]
pub struct Recovery {
    client: Arc<crate::client::Client>,
    context: RecoveryContext,
}

/// Plain, JS-serializable view of the recovery state.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct RecoveryStateView {
    state: &'static str,
    /// Milliseconds since the epoch.
    #[serde(skip_serializing_if = "Option::is_none")]
    exported_at: Option<i64>,
}

#[wasm_bindgen]
impl Recovery {
    /// `{ state: "disabled" }` or `{ state: "enabled", exportedAt }` — when
    /// the current recovery data was made (milliseconds since the epoch).
    #[wasm_bindgen(js_name = state)]
    pub async fn state(&self) -> Result<JsValue, JsValue> {
        let view = match crate::recovery::account::recovery_state(&self.client)
            .await
            .map_err(|e| to_js(&e))?
        {
            RecoveryState::Disabled => RecoveryStateView {
                state: "disabled",
                exported_at: None,
            },
            RecoveryState::Enabled { exported_at_ms } => RecoveryStateView {
                state: "enabled",
                exported_at: Some(exported_at_ms),
            },
        };
        serde_wasm_bindgen::to_value(&view).map_err(|e| named_error("Encoding", &e.to_string()))
    }

    /// The first recovery data (a `Uint8Array`), sealed with `pin` — only
    /// while disabled; the PIN must pass `Kupferdraht.validateNewPin`. Costs one
    /// attempt at the PIN service.
    #[wasm_bindgen(js_name = enable)]
    pub async fn enable(&self, pin: String) -> Result<Vec<u8>, JsValue> {
        let pin = Zeroizing::new(pin);
        crate::recovery::account::enable_recovery(&self.client, &self.context, &pin)
            .await
            .map_err(|e| to_js(&e))
    }

    /// New recovery data under `newPin` — with the same PIN twice, a new
    /// copy of lost data. `currentPin` is checked locally: another one
    /// throws `WrongPin` (no attempt spent). Earlier data still opens with
    /// its own PIN: replace the copies the user kept.
    #[wasm_bindgen(js_name = changePin)]
    pub async fn change_pin(
        &self,
        current_pin: String,
        new_pin: String,
    ) -> Result<Vec<u8>, JsValue> {
        let current_pin = Zeroizing::new(current_pin);
        let new_pin = Zeroizing::new(new_pin);
        crate::recovery::account::change_recovery_pin(
            &self.client,
            &self.context,
            &current_pin,
            &new_pin,
        )
        .await
        .map_err(|e| to_js(&e))
    }

    /// Whether `pin` is the recovery PIN — checked locally, no attempt
    /// spent (a reminder to keep it in mind).
    #[wasm_bindgen(js_name = verifyPin)]
    pub async fn verify_pin(&self, pin: String) -> Result<bool, JsValue> {
        let pin = Zeroizing::new(pin);
        crate::recovery::account::verify_recovery_pin(&self.client, &self.context, &pin)
            .await
            .map_err(|e| to_js(&e))
    }
}
