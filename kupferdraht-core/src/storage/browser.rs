// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Browser storage (Step 91, ADR-0022; on sqlite-remote-vfs since Step 111,
//! ADR-0035). **wasm-only.**
//!
//! The browser runs the same `SQLite3MC` store as native; what differs is
//! where the bytes live. Each database is opened through its own
//! registration of `sqlite-remote-vfs` — the VFS allows one database per
//! registration and one connection per database — with `SQLite3MC`'s
//! encrypting VFS above it (`RemoteVfs::encrypted_name`). In the VFS's
//! **local mode** (Step 111a) every database is an `IndexedDB` database
//! named `sqlite-remote-vfs-local/<home-id>/<path>`, one Web Lock per
//! database; in the **server mode** (Step 111b, `remote::server`) the
//! account databases live on a storage server, `global.db` stays local.
//! A database is *prepared* — its VFS registered, an async step — before
//! the synchronous open, which names that VFS explicitly.
//!
//! The VFS blocks with `Atomics.wait` on a `SharedArrayBuffer`: it needs a
//! **dedicated worker** in a **cross-origin isolated** page. Both are
//! checked before anything else, and both fail closed with a recoverable
//! `KupferdrahtError::Storage` naming the requirement — never a silent in-memory
//! fallback (ADR-0022 decision 3).
//!
//! **The home id** is the keystore entry [`BROWSER_HOME_ID_ENTRY`]: it lives
//! exactly as long as the keys whose ids carry it (Step 111a). It is the
//! namespace of the local databases.
//!
//! **One active instance per origin (Step 92, ADR-0023).** Before any
//! database is touched, the worker takes the origin-scoped Web Locks lease
//! [`BROWSER_INSTANCE_LEASE`]; held by another tab → the open fails closed
//! ("already open in another tab"), or a takeover steals it — and then
//! takes over each database's lock too (`Config::takeover`). The lease is
//! kept by a request whose callback never settles, so it lives exactly as
//! long as this worker; when another tab steals it, that request rejects
//! with `AbortError` — the eviction: the instance flips to evicted with the
//! reason [`BrowserEviction::Tab`], every store operation fails as
//! `KupferdrahtError::Evicted` from then on, and [`wait_for_browser_eviction`]
//! resolves so the binding can tell the embedder. The binding to
//! `navigator.locks` is a hand-written extern block: no `unsafe`, no
//! `web_sys_unstable_apis`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use rusqlite::{Connection, OpenFlags};
use sqlite_remote_vfs::{Config, Failure, RemoteVfs};
use tokio::sync::watch;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

use crate::error::KupferdrahtError;
use crate::keystore::SecureKeyStore;

/// The origin-scoped Web Locks lease one active Kupferdraht instance holds
/// (ADR-0023). Web Locks are origin-scoped already, so the name carries
/// no origin.
pub const BROWSER_INSTANCE_LEASE: &str = "kupferdraht:instance";

/// The keystore entry holding the browser's home id (Step 111a): it lives
/// exactly as long as the keys whose ids carry it.
pub const BROWSER_HOME_ID_ENTRY: &str = "kupferdraht.home-id";

/// The prefix the VFS's local mode gives every `IndexedDB` database.
const LOCAL_DATABASE_PREFIX: &str = "sqlite-remote-vfs-local";

/// Why this instance stopped (ADR-0035): passed to the embedder's
/// `onEvicted`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrowserEviction {
    /// Another tab of this origin took the instance over (ADR-0023).
    Tab,
    /// Another instance — another browser or device — took the account's
    /// databases on the storage server over (Step 111b).
    Device,
    /// The token exchange no longer admits the user to the storage server
    /// (Step 111b).
    Access,
}

impl BrowserEviction {
    /// The reason as the browser API spells it.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tab => "tab",
            Self::Device => "device",
            Self::Access => "access",
        }
    }
}

#[wasm_bindgen]
extern "C" {
    /// `navigator.locks.request(name, options, callback)` — the Web Locks
    /// API, bound by hand (web-sys keeps it behind `web_sys_unstable_apis`).
    /// Resolves in both `Window` and worker scopes. `catch`: a browser
    /// without `navigator.locks` throws, which surfaces as `Err`.
    #[wasm_bindgen(js_namespace = ["navigator", "locks"], js_name = request, catch)]
    fn locks_request(
        name: &str,
        options: &JsValue,
        callback: &js_sys::Function,
    ) -> Result<js_sys::Promise, JsValue>;
}

/// The worker's lease state (single-threaded: a `thread_local`).
struct LeaseState {
    /// This worker holds [`BROWSER_INSTANCE_LEASE`].
    held: bool,
    /// Why the instance stopped; `Some` once it is dead.
    evicted: Option<BrowserEviction>,
    /// Broadcast of `evicted` for [`wait_for_browser_eviction`].
    eviction: watch::Sender<Option<BrowserEviction>>,
}

/// A prepared database: its VFS, whether it exists, whether its open may
/// create it, and — on the storage server — what its VFS was registered
/// for.
struct PreparedDatabase {
    vfs: RemoteVfs,
    exists: bool,
    create: bool,
    server: Option<ServerRegistration>,
    /// The account a server database belongs to: its health hears the
    /// database's outages (Step 113).
    account: Option<String>,
}

/// What a database's VFS on the storage server was registered for: the
/// public key of the storage-login key it logs in with, and whether its
/// open takes the database over from another instance (Step 111c: the
/// takeover of the call that opens the account).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ServerRegistration {
    pub(super) owner: [u8; 32],
    pub(super) take_over: bool,
}

/// The opened storage: the namespace, the takeover flag, the prepared
/// databases keyed by their path, and the number of VFS registrations —
/// each needs a name of its own for the rest of the worker's life.
#[derive(Default)]
struct Storage {
    home_id: Option<String>,
    take_over: bool,
    databases: HashMap<String, PreparedDatabase>,
    registrations: usize,
}

thread_local! {
    static LEASE: RefCell<LeaseState> = RefCell::new(LeaseState {
        held: false,
        evicted: None,
        eviction: watch::channel(None).0,
    });
    static STORAGE: RefCell<Storage> = RefCell::new(Storage::default());
}

/// Whether this worker's instance was stopped. Once `true` it stays `true`:
/// every store operation fails as `Evicted`.
#[must_use]
pub fn browser_storage_evicted() -> bool {
    LEASE.with(|l| l.borrow().evicted.is_some())
}

/// Resolve once this instance has stopped, with the reason.
pub async fn wait_for_browser_eviction() -> BrowserEviction {
    let mut rx = LEASE.with(|l| l.borrow().eviction.subscribe());
    loop {
        if let Some(reason) = *rx.borrow_and_update() {
            return reason;
        }
        if rx.changed().await.is_err() {
            return BrowserEviction::Tab;
        }
    }
}

/// Stop the instance for `reason` — the first reason stands.
pub(super) fn mark_evicted(reason: BrowserEviction) {
    LEASE.with(|l| {
        let mut state = l.borrow_mut();
        if state.evicted.is_some() {
            return;
        }
        state.held = false;
        state.evicted = Some(reason);
        let _ = state.eviction.send(Some(reason));
    });
    debug_assert!(browser_storage_evicted(), "the eviction is recorded");
}

/// Step 119: the listener sqlite-remote-vfs calls when another instance of
/// the account took the server database `database` over — the instance stops
/// at once with the reason `"device"`, not at its next write. Called once per
/// database; the first call stands. Public for the browser suite.
pub fn database_taken_over(database: &str) {
    tracing::warn!(
        database,
        "another browser took the account's databases over; stopping"
    );
    mark_evicted(BrowserEviction::Device);
}

/// Step 119: whether this instance stopped — or a server database reports a
/// takeover no event announced (another server instance took it over: the
/// client learns at its next request). Then the instance is marked evicted,
/// and an error the caller saw is the takeover's symptom.
pub(crate) fn evicted_by_takeover() -> bool {
    if browser_storage_evicted() {
        return true;
    }
    let taken_over = STORAGE.with(|s| {
        s.borrow()
            .databases
            .values()
            .any(|db| db.server.is_some() && matches!(db.vfs.failure(), Some(Failure::TakenOver)))
    });
    if taken_over {
        mark_evicted(BrowserEviction::Device);
    }
    taken_over
}

pub(super) fn refuse_if_evicted() -> Result<(), KupferdrahtError> {
    if browser_storage_evicted() {
        Err(KupferdrahtError::Evicted)
    } else {
        Ok(())
    }
}

fn lock_options(if_available: bool, steal: bool) -> Result<JsValue, KupferdrahtError> {
    let options = js_sys::Object::new();
    for (key, value) in [
        ("mode", JsValue::from_str("exclusive")),
        ("ifAvailable", JsValue::from_bool(if_available)),
        ("steal", JsValue::from_bool(steal)),
    ] {
        js_sys::Reflect::set(&options, &JsValue::from_str(key), &value)
            .map_err(|e| KupferdrahtError::Storage(format!("browser lease options: {e:?}")))?;
    }
    Ok(options.into())
}

/// Request [`BROWSER_INSTANCE_LEASE`] and keep it: the callback returns a
/// promise that never settles, so the lock lives as long as this worker.
/// `ifAvailable` (the normal open) hands the callback `null` when another
/// tab holds it → `Ok(false)`; `steal` (takeover) always grants. The outer
/// request promise is awaited in a background task: it rejects with
/// `AbortError` when a later takeover steals the lease — the eviction.
async fn acquire_lease(steal: bool) -> Result<bool, KupferdrahtError> {
    let (granted_tx, granted_rx) = tokio::sync::oneshot::channel::<bool>();
    let granted_tx = RefCell::new(Some(granted_tx));
    let keep = Closure::once_into_js(move |lock: JsValue| -> JsValue {
        let granted = !lock.is_null() && !lock.is_undefined();
        if let Some(tx) = granted_tx.borrow_mut().take() {
            let _ = tx.send(granted);
        }
        if granted {
            js_sys::Promise::new(&mut |_resolve, _reject| {}).into()
        } else {
            JsValue::UNDEFINED
        }
    });
    let request = locks_request(
        BROWSER_INSTANCE_LEASE,
        &lock_options(!steal, steal)?,
        keep.unchecked_ref(),
    )
    .map_err(|e| {
        KupferdrahtError::Storage(format!(
            "browser lease: navigator.locks is unavailable in this browser ({e:?})"
        ))
    })?;
    wasm_bindgen_futures::spawn_local(async move {
        // Held leases keep this pending; a steal rejects it (AbortError).
        if JsFuture::from(request).await.is_err() {
            mark_evicted(BrowserEviction::Tab);
        }
    });
    let granted = granted_rx.await.map_err(|_| {
        KupferdrahtError::Storage("browser lease: request callback never ran".into())
    })?;
    if granted {
        LEASE.with(|l| l.borrow_mut().held = true);
    }
    Ok(granted)
}

/// The context checks, before any lease is taken: a dedicated worker (the
/// VFS blocks, which the main thread may not), in a cross-origin isolated
/// page (the VFS needs a `SharedArrayBuffer`).
///
/// # Errors
/// `KupferdrahtError::Storage` naming the missing requirement.
fn ensure_worker_context() -> Result<(), KupferdrahtError> {
    let global = js_sys::global();
    if global
        .dyn_ref::<web_sys::DedicatedWorkerGlobalScope>()
        .is_none()
    {
        return Err(KupferdrahtError::Storage(
            "browser storage needs a dedicated worker: the storage blocks while it waits for \
             IndexedDB or the server, which the main thread may not — run Kupferdraht inside a Web \
             Worker"
                .into(),
        ));
    }
    let isolated = js_sys::Reflect::get(&global, &JsValue::from_str("crossOriginIsolated"))
        .ok()
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if !isolated {
        return Err(KupferdrahtError::Storage(
            "browser storage needs a cross-origin isolated page (it uses a SharedArrayBuffer): \
             serve the app with `Cross-Origin-Opener-Policy: same-origin` and \
             `Cross-Origin-Embedder-Policy: require-corp`"
                .into(),
        ));
    }
    Ok(())
}

/// Open the browser's storage: check that the page is cross-origin
/// isolated and that this is a dedicated worker, take the origin's
/// instance lease (or take it over), and resolve the home id from
/// `keystore` — created and stored durably on a first open. The home id is
/// the namespace of the local databases. Returns the home id. Idempotent
/// within the lease holder.
///
/// # Errors
/// `KupferdrahtError::Storage` without cross-origin isolation or outside a
/// dedicated worker, when another tab holds the lease, or when the home id
/// cannot be read or stored; `KupferdrahtError::Evicted` after an eviction.
pub async fn open_browser_storage(
    keystore: &Arc<dyn SecureKeyStore>,
    take_over: bool,
) -> Result<String, KupferdrahtError> {
    ensure_worker_context()?;
    refuse_if_evicted()?;
    if !LEASE.with(|l| l.borrow().held) && !acquire_lease(take_over).await? {
        return Err(KupferdrahtError::Storage(
            "browser storage: Kupferdraht is already open in another tab of this origin — take \
             over with Kupferdraht.open({ takeOver: true }) or close that tab"
                .into(),
        ));
    }
    let home_id = resolve_home_id(keystore).await?;
    STORAGE.with(|s| -> Result<(), KupferdrahtError> {
        let mut storage = s.borrow_mut();
        match &storage.home_id {
            Some(open) if *open != home_id => Err(KupferdrahtError::Storage(format!(
                "browser storage: already open for home {open}, the keystore names {home_id}"
            ))),
            _ => {
                storage.home_id = Some(home_id.clone());
                storage.take_over |= take_over;
                Ok(())
            }
        }
    })?;
    Ok(home_id)
}

/// The home id in `keystore`, or a new one stored durably.
async fn resolve_home_id(keystore: &Arc<dyn SecureKeyStore>) -> Result<String, KupferdrahtError> {
    if let Some(bytes) = keystore.get_secret(BROWSER_HOME_ID_ENTRY.to_owned())? {
        let id = String::from_utf8(bytes)
            .ok()
            .filter(|id| super::home_id::is_home_id(id))
            .ok_or_else(|| {
                KupferdrahtError::Storage(format!(
                    "browser storage: the keystore entry {BROWSER_HOME_ID_ENTRY} is not a home \
                     id; it is never rewritten — a new id would orphan every key of this home"
                ))
            })?;
        return Ok(id);
    }
    let id = super::home_id::generate_home_id();
    keystore.set_secret(BROWSER_HOME_ID_ENTRY.to_owned(), id.clone().into_bytes())?;
    if let Err(e) = crate::keystore::flush_pending_writes().await {
        let _ = keystore.delete_secret(BROWSER_HOME_ID_ENTRY.to_owned());
        return Err(e);
    }
    tracing::info!(home_id = %id, "browser home id created");
    Ok(id)
}

/// The home id of the opened storage (Step 111a: `KupferdrahtHome`'s id in the
/// browser).
///
/// # Errors
/// `KupferdrahtError::Storage` before [`open_browser_storage`].
pub(crate) fn browser_home_id() -> Result<String, KupferdrahtError> {
    STORAGE
        .with(|s| s.borrow().home_id.clone())
        .ok_or_else(not_opened)
}

fn not_opened() -> KupferdrahtError {
    KupferdrahtError::Storage("browser storage: not opened — open Kupferdraht first".into())
}

fn database_key(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Register the VFS of the database at `path` — once per worker — so a
/// store can open it. Call before opening it.
///
/// # Errors
/// `KupferdrahtError::Storage` before [`open_browser_storage`], or when the VFS
/// cannot be registered; `KupferdrahtError::Evicted` after an eviction.
pub async fn prepare_browser_database(path: &Path) -> Result<(), KupferdrahtError> {
    refuse_if_evicted()?;
    let key = database_key(path);
    let (home_id, take_over) = STORAGE.with(|s| {
        let storage = s.borrow();
        (storage.home_id.clone(), storage.take_over)
    });
    let home_id = home_id.ok_or_else(not_opened)?;
    if STORAGE.with(|s| s.borrow().databases.contains_key(&key)) {
        return Ok(());
    }
    let exists = local_database_names()
        .await?
        .contains(&format!("{LOCAL_DATABASE_PREFIX}/{home_id}/{key}"));
    let mut config = Config::local(home_id);
    config.takeover = take_over;
    let vfs = RemoteVfs::register_async(&registration_name("local"), config)
        .await
        .map_err(|e| KupferdrahtError::Storage(format!("browser storage: prepare {key}: {e}")))?;
    STORAGE.with(|s| {
        s.borrow_mut().databases.insert(
            key.clone(),
            PreparedDatabase {
                vfs,
                exists,
                create: true,
                server: None,
                account: None,
            },
        );
    });
    debug_assert!(
        browser_database_vfs(path).is_some(),
        "the database is prepared"
    );
    Ok(())
}

/// A VFS name no registration of this worker used: SQLite keeps every
/// registration for the life of the worker.
pub(super) fn registration_name(kind: &str) -> String {
    STORAGE.with(|s| {
        let mut storage = s.borrow_mut();
        storage.registrations += 1;
        format!("kupferdraht-{kind}-{}", storage.registrations)
    })
}

/// Record a database on the storage server, prepared with `vfs` for
/// `registration` — replacing a registration for another key or another
/// takeover. Whether it exists is unknown before its open: it counts as
/// existing, so no key is ever generated over it (ADR-0029).
pub(super) fn insert_server_database(
    path: &Path,
    vfs: RemoteVfs,
    registration: ServerRegistration,
    account_id: &str,
) {
    STORAGE.with(|s| {
        s.borrow_mut().databases.insert(
            database_key(path),
            PreparedDatabase {
                vfs,
                exists: true,
                create: false,
                server: Some(registration),
                account: Some(account_id.to_owned()),
            },
        );
    });
    debug_assert_eq!(
        server_registration(path),
        Some(registration),
        "the database is prepared"
    );
}

/// What a prepared server database's VFS was registered for; `None` for a
/// local or an unprepared database.
pub(super) fn server_registration(path: &Path) -> Option<ServerRegistration> {
    STORAGE.with(|s| {
        s.borrow()
            .databases
            .get(&database_key(path))
            .and_then(|db| db.server)
    })
}

/// Whether the open of the prepared database at `path` may create it.
pub(super) fn set_database_create(path: &Path, create: bool) {
    STORAGE.with(|s| {
        if let Some(db) = s.borrow_mut().databases.get_mut(&database_key(path)) {
            db.create = create;
        }
    });
}

/// The names of the origin's `IndexedDB` databases (`indexedDB.databases()`,
/// called by reflection: web-sys keeps it behind its unstable APIs).
async fn local_database_names() -> Result<Vec<String>, KupferdrahtError> {
    let unavailable =
        |e: JsValue| KupferdrahtError::Storage(format!("browser storage: indexedDB: {e:?}"));
    let factory = js_sys::Reflect::get(&js_sys::global(), &JsValue::from_str("indexedDB"))
        .map_err(unavailable)?;
    let databases: js_sys::Function =
        js_sys::Reflect::get(&factory, &JsValue::from_str("databases"))
            .map_err(unavailable)?
            .dyn_into()
            .map_err(unavailable)?;
    let promise: js_sys::Promise = databases
        .call0(&factory)
        .map_err(unavailable)?
        .dyn_into()
        .map_err(unavailable)?;
    let list = JsFuture::from(promise).await.map_err(unavailable)?;
    Ok(js_sys::Array::from(&list)
        .iter()
        .filter_map(|info| {
            js_sys::Reflect::get(&info, &JsValue::from_str("name"))
                .ok()
                .and_then(|n| n.as_string())
        })
        .collect())
}

/// The encrypting VFS of a prepared database, to open a raw connection on
/// it; `None` when the database was not prepared.
#[must_use]
pub fn browser_database_vfs(path: &Path) -> Option<String> {
    STORAGE.with(|s| {
        s.borrow()
            .databases
            .get(&database_key(path))
            .map(|db| db.vfs.encrypted_name())
    })
}

/// Open a connection on the prepared database at `path` through its VFS
/// — created when missing, unless it was prepared without creating.
///
/// # Errors
/// `KupferdrahtError::Evicted` after an eviction; `KupferdrahtError::Storage` when
/// the database was not prepared; `HeldElsewhere` / `StorageFull` /
/// `NothingStored` / `StorageAccessDenied` / `Storage` from the open
/// ([`super::browser_open_error`]).
pub(crate) fn open_browser_connection(path: &Path) -> Result<Connection, KupferdrahtError> {
    refuse_if_evicted()?;
    let (vfs, create) = STORAGE
        .with(|s| {
            s.borrow()
                .databases
                .get(&database_key(path))
                .map(|db| (db.vfs.encrypted_name(), db.create))
        })
        .ok_or_else(|| {
            KupferdrahtError::Storage(format!(
                "browser storage: {} was not prepared before its open",
                path.display()
            ))
        })?;
    let flags = if create {
        OpenFlags::default()
    } else {
        OpenFlags::default().difference(OpenFlags::SQLITE_OPEN_CREATE)
    };
    let conn = Connection::open_with_flags_and_vfs(path, flags, vfs.as_str())
        .map_err(|e| super::browser_open_error(path, &e))?;
    STORAGE.with(|s| {
        if let Some(db) = s.borrow_mut().databases.get_mut(&database_key(path)) {
            db.exists = true;
        }
    });
    Ok(conn)
}

/// Before an operation on the database at `path` (Step 111b, Step 113,
/// ADR-0038), by the failure its VFS reported. Taken over: by another tab
/// for a local database (its lease went too), by another instance for one
/// on the storage server — the instance stops (`onEvicted("device")`). The
/// token refused: `StorageAccessDenied`. The server unreachable: `probe` —
/// a cheap read on the caller's connection — lets the VFS heal the
/// database, or it fails at once; the account's health hears either way.
///
/// # Errors
/// `KupferdrahtError::Evicted` after an eviction or a takeover;
/// `StorageAccessDenied` for a refused token; `ServiceUnavailable` while
/// the storage server is unreachable.
pub(crate) fn check_database(
    path: &Path,
    probe: impl FnOnce() -> bool,
) -> Result<(), KupferdrahtError> {
    refuse_if_evicted()?;
    let reachable = match database_failure(path) {
        Some((Failure::TakenOver, on_server)) => {
            mark_evicted(if on_server {
                BrowserEviction::Device
            } else {
                BrowserEviction::Tab
            });
            return Err(KupferdrahtError::Evicted);
        }
        Some((Failure::Denied, _)) => {
            return Err(KupferdrahtError::StorageAccessDenied(format!(
                "{}: the storage server refused the storage token",
                path.display()
            )));
        }
        Some((Failure::Unreachable, _)) => probe() && database_failure(path).is_none(),
        _ => true,
    };
    if let Some(account) = database_account(path) {
        super::remote::server::report_database_reachable(&account, path, reachable);
    }
    if reachable {
        return Ok(());
    }
    Err(KupferdrahtError::ServiceUnavailable(format!(
        "{}: the storage server is unreachable — the storage reconnects by itself",
        path.display()
    )))
}

/// The failure the VFS of the database at `path` reported, and whether the
/// database is on the storage server.
fn database_failure(path: &Path) -> Option<(Failure, bool)> {
    STORAGE.with(|s| {
        s.borrow()
            .databases
            .get(&database_key(path))
            .and_then(|db| {
                db.vfs
                    .failure()
                    .map(|failure| (failure, db.server.is_some()))
            })
    })
}

/// The account a server database belongs to.
fn database_account(path: &Path) -> Option<String> {
    STORAGE.with(|s| {
        s.borrow()
            .databases
            .get(&database_key(path))
            .and_then(|db| db.account.clone())
    })
}

/// Whether the prepared database at `path` exists.
///
/// # Errors
/// `KupferdrahtError::Storage` when the database was not prepared.
pub(crate) fn browser_database_exists(path: &Path) -> Result<bool, KupferdrahtError> {
    STORAGE
        .with(|s| {
            s.borrow()
                .databases
                .get(&database_key(path))
                .map(|db| db.exists)
        })
        .ok_or_else(|| {
            KupferdrahtError::Storage(format!(
                "browser storage: {} was not prepared before its existence check",
                path.display()
            ))
        })
}

/// Delete the prepared database at `path` through its VFS — it must not be
/// open. A missing database is not an error.
///
/// # Errors
/// `KupferdrahtError::Evicted` after an eviction; `KupferdrahtError::Storage` when
/// the database was not prepared or the VFS refuses the deletion.
/// Step 120 (ADR-0040): claim the account's storage slot for this device's
/// key, through the VFS of the server database at `path` — another device
/// holding it is replaced and its databases deleted. The replaced device's
/// label, when it had one.
///
/// # Errors
/// `InvalidInput` for a database not on the storage server;
/// `StorageAccessDenied` for a refused token; `ServiceUnavailable`
/// otherwise.
pub(crate) fn claim_account_slot(path: &Path) -> Result<Option<String>, KupferdrahtError> {
    let slot = with_server_vfs(path, "claim the slot", RemoteVfs::claim_slot)?;
    Ok(slot.replaced_label)
}

/// Step 120 (ADR-0040): label the account's storage slot with this
/// device's Wire client id — the slot this key holds.
///
/// # Errors
/// `DeviceRetired` when another device holds the slot; as
/// [`claim_account_slot`] otherwise.
pub(crate) fn label_account_slot(path: &Path, client_id: &str) -> Result<(), KupferdrahtError> {
    with_server_vfs(path, "label the slot", |vfs| vfs.set_slot_label(client_id)).map(|_| ())
}

/// Step 120 (ADR-0040): release the account's storage slot and delete
/// every database of this device's key on the server. No database may be
/// open on the VFS.
///
/// # Errors
/// As [`label_account_slot`].
pub(crate) fn delete_account_slot(path: &Path) -> Result<(), KupferdrahtError> {
    with_server_vfs(path, "delete the slot", RemoteVfs::delete_slot)
}

/// Run `call` on the VFS of the server database at `path`, mapping the
/// slot errors.
fn with_server_vfs<T>(
    path: &Path,
    what: &str,
    call: impl FnOnce(&RemoteVfs) -> Result<T, sqlite_remote_vfs::Error>,
) -> Result<T, KupferdrahtError> {
    refuse_if_evicted()?;
    let name = path.display().to_string();
    STORAGE.with(|s| {
        let storage = s.borrow();
        let db = storage
            .databases
            .get(&database_key(path))
            .filter(|db| db.server.is_some())
            .ok_or_else(|| {
                KupferdrahtError::InvalidInput(format!(
                    "{name}: not a database on the storage server"
                ))
            })?;
        call(&db.vfs).map_err(|e| {
            let message = format!("{name}: {what}: {e}");
            if e.is_slot_taken() {
                KupferdrahtError::DeviceRetired(message)
            } else if e.is_access_denied() {
                KupferdrahtError::StorageAccessDenied(message)
            } else {
                KupferdrahtError::ServiceUnavailable(message)
            }
        })
    })
}

pub(crate) fn delete_browser_database(path: &Path) -> Result<(), KupferdrahtError> {
    refuse_if_evicted()?;
    let key = database_key(path);
    STORAGE.with(|s| {
        let mut storage = s.borrow_mut();
        let db = storage.databases.get_mut(&key).ok_or_else(|| {
            KupferdrahtError::Storage(format!(
                "browser storage: {key} was not prepared before its deletion"
            ))
        })?;
        db.vfs.delete_database(&key).map_err(|e| {
            KupferdrahtError::Storage(format!("browser storage: delete {key}: {e}"))
        })?;
        db.exists = false;
        Ok(())
    })
}
