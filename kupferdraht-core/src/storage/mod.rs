// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

// Crash-safe file replacement (Step 77 / Step 93). `std::fs` on every
// target: the dev `FileKeyStore` is compiled everywhere; the browser's own
// files live in the OPFS pool.
#[cfg(any(target_arch = "wasm32", test))]
mod account_files;
pub(crate) mod account_keys;
mod atomic_file;
#[cfg(any(target_arch = "wasm32", test))]
pub(crate) mod remote;
// The browser's OPFS storage install (Step 91, ADR-0022) — wasm-only; the
// SQLite backend below is the store on every target.
#[cfg(target_arch = "wasm32")]
mod browser;
pub mod db_key;
mod durable_keys;
mod engine;
mod global;
mod home;
// The home id (Step 93, ADR-0025) — `base/home.id`.
mod home_id;
// The advisory cross-process lock (Step 85) — native only.
#[cfg(not(target_arch = "wasm32"))]
mod lock;
pub(crate) mod page;
mod records;
mod schema;
mod sqlite;

#[cfg(target_arch = "wasm32")]
pub use browser::{
    BROWSER_HOME_ID_ENTRY, BROWSER_INSTANCE_LEASE, BrowserEviction, browser_database_vfs,
    browser_storage_evicted, database_taken_over, open_browser_storage, prepare_browser_database,
    wait_for_browser_eviction,
};
pub use engine::StorageEngineInfo;
pub use global::{AccountEntry, GlobalStore, open_default_global_store};
pub use records::{
    AttachmentRecord, BackendMlsKey, DeliveryReceiptRecord, DeviceRecord, ExpiredMessageRecord,
    FeatureConfigEntry, MentionRecord, MessageEditRecord, ReactionRecord, ReadReceiptRecord,
    RecoveryStateRecord, RoomMemberRecord, RoomRecord, SelfUserRecord, ServerConfigRecord,
    SessionRecord, Store, SyncStateRecord, TaskRecord, TimelineEventRecord, UserPropertyEntry,
};
pub use sqlite::SqliteStore;

#[cfg(any(target_arch = "wasm32", test))]
pub(crate) use account_files::{delete_account_databases, delete_leftover_account_databases};
pub(crate) use atomic_file::write_atomically;
#[cfg(target_arch = "wasm32")]
pub(crate) use browser::{
    check_database, claim_account_slot, delete_account_slot, evicted_by_takeover,
    label_account_slot,
};
pub(crate) use durable_keys::ensure_account_keys_durable;
#[cfg(target_arch = "wasm32")]
pub(crate) use durable_keys::ensure_global_key_durable;
#[cfg(any(target_arch = "wasm32", test))]
pub(crate) use durable_keys::store_seed_durably;
pub(crate) use global::open_global_store_in;
pub(crate) use home::{KupferdrahtHome, is_path_safe_id};
pub(crate) use home_id::ephemeral_cookie_label;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use lock::ProcessLock;
#[cfg(target_arch = "wasm32")]
pub(crate) use remote::health::StorageState;
#[cfg(target_arch = "wasm32")]
pub(crate) use remote::server::{
    account_on_storage_server, account_storage_state, attach_account_client,
    prepare_remote_account, storage_server_configured, subscribe_storage_state,
    unsubscribe_storage_state, use_storage_server,
};
#[cfg(target_arch = "wasm32")]
pub(crate) use remote::wire_session::WireSession;

use std::path::Path;
#[cfg(not(target_arch = "wasm32"))]
use std::path::PathBuf;
use std::sync::Arc;

use rusqlite::Connection;
use zeroize::Zeroizing;

use crate::error::KupferdrahtError;
use crate::keystore::key_id::KeyId;
use crate::keystore::{SecureKeyStore, key_id, load_or_create_key};

/// How long a connection waits for a concurrent writer before surfacing
/// `SQLITE_BUSY` (Step 85, open.md §69). Two processes on one home write
/// the same files; a write transaction lasts milliseconds, a migration
/// run or a large timeline batch at most a few seconds. Native only, like
/// the pragma that applies it.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) const BUSY_TIMEOUT_MS: u32 = 5_000;

/// The multi-process pragmas every native connection carries (Step 85):
/// `busy_timeout` so a second process's write waits instead of failing
/// `database is locked`, and `synchronous = FULL` made explicit — a
/// rotated `zuid` or an MLS epoch advance that fails to reach disk after
/// a power loss costs a re-login or a group desync, which beats the write
/// throughput `NORMAL` would buy. `rusqlite` 0.37 already sets a 5 s busy
/// timeout on every open (`inner_connection.rs`); the pragma pins the
/// contract here, where the open paths are, instead of relying on a
/// dependency default. Native only, like the WAL pragma: the browser's
/// OPFS pool is single-instance by ADR-0023.
///
/// # Errors
/// `KupferdrahtError::Storage` if a pragma fails.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn set_multi_process_pragmas(conn: &Connection) -> Result<(), KupferdrahtError> {
    conn.pragma_update(None, "busy_timeout", BUSY_TIMEOUT_MS)
        .map_err(|e| KupferdrahtError::Storage(format!("set busy_timeout: {e}")))?;
    conn.pragma_update(None, "synchronous", "FULL")
        .map_err(|e| KupferdrahtError::Storage(format!("set synchronous=FULL: {e}")))?;
    // Postcondition on our own mutation: the engine reports what was set.
    let busy: i64 = conn
        .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
        .map_err(|e| KupferdrahtError::Storage(format!("read busy_timeout: {e}")))?;
    debug_assert_eq!(
        busy,
        i64::from(BUSY_TIMEOUT_MS),
        "busy_timeout pragma took effect"
    );
    Ok(())
}

/// Which advisory lock guards a first-time key creation (Step 85).
#[derive(Clone, Copy, Debug)]
pub(crate) enum KeyLock<'a> {
    /// `base/kupferdraht.lock` — the device-registry key (`global-db`).
    Home,
    /// `base/accounts/<id>/kupferdraht.lock` — the account's own keys.
    Account(&'a str),
}

impl KeyLock<'_> {
    /// What the lock guards, for logs and the `AccountLocked` payload.
    fn scope(self, home: &KupferdrahtHome) -> String {
        match self {
            Self::Home => format!("home {}", home.base_dir().display()),
            Self::Account(account_id) => format!("account {account_id}"),
        }
    }

    /// The lock file that is taken — the layout's single source of names.
    #[cfg(not(target_arch = "wasm32"))]
    fn lock_path(self, home: &KupferdrahtHome) -> PathBuf {
        match self {
            Self::Home => home.home_lock(),
            Self::Account(account_id) => home.account_lock(account_id),
        }
    }
}

/// [`load_or_create_key`] made atomic across processes (Step 85, review
/// S18): the plain get-then-set lets two first initialisations of one
/// account each generate a key, the second `set_secret` overwrites, and
/// the database created under the first key is lost.
///
/// Fast path first, without the lock: an existing key is the common case
/// and must never contend — two processes on one home stay possible.
/// Only when the key is absent is the directory's advisory lock taken
/// (fail-fast: `AccountLocked` when another process is inside), and the
/// keystore is **re-read under the lock** before generating, so the
/// loser of the race adopts the winner's key on its retry.
///
/// Step 93 (ADR-0025): `key` carries the home id, so two *different*
/// homes on one keystore never contend for one entry (the residual
/// Step 85 could not close); inside the critical section a keystore-wide
/// entry an earlier build stored is adopted for `database` when that
/// file already exists ([`adopt_or_create_key`]).
///
/// wasm: the plain path — one instance per origin (ADR-0023).
///
/// # Errors
/// `KupferdrahtError::AccountLocked` when another process holds the lock;
/// `KupferdrahtError::KeyUnavailable` when `database` exists but the keystore
/// has no key for it (ADR-0029 — nothing is written);
/// `KupferdrahtError::Storage` / `KupferdrahtError::Generic` from the directory,
/// the lock file or the keystore.
pub(crate) fn load_or_create_key_locked(
    home: &KupferdrahtHome,
    lock: KeyLock<'_>,
    keystore: &Arc<dyn SecureKeyStore>,
    key: &KeyId,
    database: &Path,
) -> Result<db_key::DbKey, KupferdrahtError> {
    debug_assert!(
        key.namespaced().starts_with("kupferdraht."),
        "keystore ids carry the home id"
    );
    if let Some(existing) = crate::keystore::load_key(keystore, key.namespaced())? {
        return Ok(existing);
    }
    let scope = lock.scope(home);
    tracing::debug!(%scope, id = key.namespaced(), "first-time database key creation");
    #[cfg(not(target_arch = "wasm32"))]
    let _lock = {
        let path = lock.lock_path(home);
        create_parent_dir(&path)?;
        ProcessLock::try_acquire(&path, &scope)?
    };
    // Re-read under the lock: the loser of a race adopts the winner's key.
    if let Some(existing) = crate::keystore::load_key(keystore, key.namespaced())? {
        return Ok(existing);
    }
    // ADR-0035: a `Seed` home derives an account's key — the same key for an
    // existing database, so ADR-0029's refusal below does not apply to it.
    if let Some(derived) = account_keys::derive_missing_account_key(home, keystore, key)? {
        return Ok(derived);
    }
    adopt_or_create_key(keystore, key, database)
}

/// Inside the locked critical section, with no home-scoped key present:
/// for an **existing** database, adopt the keystore-wide entry an earlier
/// build stored under the legacy id, else refuse — the database was
/// written under a key the keystore does not return, and a new key would
/// replace it for good (ADR-0029). A key is generated only for a database
/// that does not exist yet (a fresh database always gets a fresh key,
/// ADR-0025 H3). Adoption is one `set_secret` of the namespaced id; the
/// legacy entry is never deleted, because no home knows whether a sibling
/// home on the same keystore still needs it.
///
/// # Errors
/// `KupferdrahtError::KeyUnavailable` for an existing database without a key
/// (nothing is written); keystore / filesystem errors otherwise.
fn adopt_or_create_key(
    keystore: &Arc<dyn SecureKeyStore>,
    key: &KeyId,
    database: &Path,
) -> Result<db_key::DbKey, KupferdrahtError> {
    if !database_exists(database)? {
        return load_or_create_key(keystore, key.namespaced());
    }
    adopt_legacy_key(keystore, key, database)
}

/// The existing-database half of [`adopt_or_create_key`]: adopt the
/// legacy entry, or refuse with `KeyUnavailable` — never generate.
///
/// # Errors
/// `KupferdrahtError::KeyUnavailable` when no legacy entry exists either;
/// keystore errors otherwise.
fn adopt_legacy_key(
    keystore: &Arc<dyn SecureKeyStore>,
    key: &KeyId,
    database: &Path,
) -> Result<db_key::DbKey, KupferdrahtError> {
    debug_assert_ne!(
        key.legacy(),
        key.namespaced(),
        "the legacy id is the pre-ADR-0025 shape, never the namespaced one"
    );
    let Some(legacy) = crate::keystore::load_key(keystore, key.legacy())? else {
        return Err(KupferdrahtError::KeyUnavailable(format!(
            "{} (keystore id {}) — if the keystore is still locked, retry after \
             unlocking; if the key is gone for good, {}",
            database.display(),
            key.namespaced(),
            key_unavailable_hint(database)
        )));
    };
    tracing::info!(
        id = key.namespaced(),
        legacy = key.legacy(),
        database = %database.display(),
        "adopting the keystore-wide key of an existing database under its home-scoped id"
    );
    keystore.set_secret(
        key.namespaced().to_owned(),
        legacy.to_keystore_bytes().to_vec(),
    )?;
    Ok(legacy)
}

/// The way out of a `KeyUnavailable` for `database`, by file name: the
/// device's account registry (`global.db`, `KupferdrahtHome::global_db`) lives
/// in the home itself, so "remove the account directory" would point at
/// the wrong place; the account databases (`kupferdraht.db`, `mls.db`) live in
/// the account directory.
fn key_unavailable_hint(database: &Path) -> &'static str {
    if database.file_name().is_some_and(|name| name == "global.db") {
        "remove the home's global.db (the device's account registry — the accounts' \
         own databases and keys stay in place) and log in again"
    } else {
        "remove the account directory and log in again as a new device"
    }
}

/// Whether the database file a key opens already exists — a file on
/// native, a name in the OPFS pool on wasm. Runtime data: an unreadable
/// path is an error, never a guess.
///
/// # Errors
/// `KupferdrahtError::Storage` when the file's metadata cannot be read (other
/// than "not found") or the pool cannot be queried (wasm).
fn database_exists(database: &Path) -> Result<bool, KupferdrahtError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        match std::fs::metadata(database) {
            Ok(metadata) => Ok(metadata.is_file()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(KupferdrahtError::Storage(format!(
                "stat {}: {e}",
                database.display()
            ))),
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        browser::browser_database_exists(database)
    }
}

/// Open a connection on the database at `path`: natively the file, in the
/// browser through the database's prepared VFS (Step 111a) — the one open
/// every store and `mls.db` go through.
///
/// # Errors
/// `KupferdrahtError::Storage` when the open fails; in the browser also
/// `HeldElsewhere`, `StorageFull` and `Evicted`.
pub(crate) fn open_connection(path: &Path) -> Result<rusqlite::Connection, KupferdrahtError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        rusqlite::Connection::open(path)
            .map_err(|e| KupferdrahtError::Storage(format!("open {}: {e}", path.display())))
    }
    #[cfg(target_arch = "wasm32")]
    {
        browser::open_browser_connection(path)
    }
}

/// Prepare an account's `kupferdraht.db` and `mls.db` in the browser before they
/// are checked or opened (Step 111a). On the storage server a Wire session
/// prepared them already ([`remote::server::prepare_remote_account`],
/// Step 111b): they must be.
///
/// # Errors
/// As [`prepare_browser_database`]; `KupferdrahtError::Storage` for server
/// databases no session prepared.
#[cfg(target_arch = "wasm32")]
pub(crate) async fn prepare_account_databases(
    home: &KupferdrahtHome,
    account_id: &str,
) -> Result<(), KupferdrahtError> {
    if remote::server::storage_server_configured() {
        return remote::server::ensure_remote_account_prepared(home, account_id);
    }
    prepare_browser_database(&home.account_db(account_id)).await?;
    prepare_browser_database(&home.account_mls(account_id)).await
}

/// Create the directory a lock file lives in (the account directory may
/// not exist yet on a first login).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn create_parent_dir(path: &Path) -> Result<(), KupferdrahtError> {
    let Some(dir) = path.parent() else {
        return Err(KupferdrahtError::Storage(format!(
            "{} has no parent directory",
            path.display()
        )));
    };
    std::fs::create_dir_all(dir)
        .map_err(|e| KupferdrahtError::Storage(format!("mkdir {}: {e}", dir.display())))
}

/// Supertrait bound for the storage traits ([`Store`], [`GlobalStore`]).
///
/// On native it is `Send + Sync` (the stores are shared across a multi-thread
/// tokio runtime). On wasm it relaxes to nothing: the browser is
/// single-threaded, and a custom `Store` over browser APIs would hold
/// `!Send` handles/futures — requiring `Send + Sync` would make such a
/// store impossible. See docs/plan/archive/45-* (Step 45 / W1).
#[cfg(not(target_arch = "wasm32"))]
pub trait MaybeSendSync: Send + Sync {}
#[cfg(not(target_arch = "wasm32"))]
impl<T: Send + Sync + ?Sized> MaybeSendSync for T {}
#[cfg(target_arch = "wasm32")]
pub trait MaybeSendSync {}
#[cfg(target_arch = "wasm32")]
impl<T: ?Sized> MaybeSendSync for T {}

/// Key `conn` with a per-database [`DbKey`] (a raw 32-byte key) via
/// `SQLite3MC`'s `chacha20` scheme (ChaCha20-Poly1305, ADR-0021). Used by
/// `SqliteStore::open` (`kupferdraht.db`), the global store, and the openmls
/// provider opening (`mls.db`) — each with its **own** key. Raw-key mode
/// keeps any KDF out of the trust path; there is no salt.
///
/// Pragma order is `SQLite3MC`'s (the inverse of `SQLCipher`'s, observations
/// §75): cipher parameters BEFORE the key — `cipher = 'chacha20'` (explicit,
/// though it is the compile-time default), `plaintext_header_size = 32`
/// (keeps the `SQLite` header readable so iOS can identify a WAL-mode DB in
/// a shared container), then `key`. The key is then **verified by a first
/// read** of `sqlite_master`: a wrong key surfaces here as a recoverable
/// `KupferdrahtError::Storage`, never later mid-migration and never as a panic
/// (the key is runtime data, not a code invariant). Hex material lives in
/// `Zeroizing` buffers.
///
/// # Errors
/// `KupferdrahtError::Storage` if a pragma fails or the key does not open the
/// file.
pub(crate) fn apply_whole_db_key(
    conn: &Connection,
    key: &db_key::DbKey,
) -> Result<(), crate::error::KupferdrahtError> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut key_hex: Zeroizing<String> = Zeroizing::new(String::with_capacity(db_key::KEY_LEN * 2));
    for &b in key.key() {
        key_hex.push(HEX[(b >> 4) as usize] as char);
        key_hex.push(HEX[(b & 0x0f) as usize] as char);
    }
    // Code invariant (the hex encoder), not runtime data.
    debug_assert_eq!(key_hex.len(), db_key::KEY_LEN * 2);
    let stmt: Zeroizing<String> = Zeroizing::new(format!(
        "PRAGMA cipher = 'chacha20'; \
         PRAGMA plaintext_header_size = 32; \
         PRAGMA key = \"x'{}'\";",
        *key_hex
    ));
    conn.execute_batch(&stmt)
        .map_err(|e| crate::error::KupferdrahtError::Storage(format!("apply db key: {e}")))?;
    // First read: on an existing file the wrong key fails here
    // (`file is not a database`); on a fresh file it counts zero rows.
    conn.query_row("SELECT count(*) FROM sqlite_master", [], |row| {
        row.get::<_, i64>(0)
    })
    .map_err(|e| {
        crate::error::KupferdrahtError::Storage(format!("verify db key (first read): {e}"))
    })?;
    Ok(())
}

/// Open the SDK's default app-data store for `account_id`, **ready to
/// use**: it materializes `base_dir/accounts/<id>/kupferdraht.db`, fetches the
/// per-account key from `keystore` itself, and keys the database. The
/// concrete store owns its encryption; returned behind the [`Store`]
/// trait so consumers never name the `rusqlite`-backed type. Paths come
/// from [`KupferdrahtHome`], the single source of the on-disk layout — on
/// wasm the same layout names files in the OPFS pool that
/// [`install_browser_storage`] set up (Step 91, ADR-0022); there is no
/// directory to create. Resolving the home reads (on a first open:
/// creates) `base_dir/home.id`, whose id the keystore id carries
/// (Step 93, ADR-0025).
///
/// # Errors
/// `KupferdrahtError::Storage` if the directory, the home id or the database
/// cannot be opened, `KupferdrahtError::AccountLocked` while another process
/// is inside the home's or the account's critical section,
/// `KupferdrahtError::KeyUnavailable` if the database exists but the keystore
/// returns no key for it (ADR-0029), or `KupferdrahtError::Generic` if the
/// keystore fails.
pub fn open_default_store(
    base_dir: &Path,
    keystore: &Arc<dyn SecureKeyStore>,
    account_id: &str,
) -> Result<Arc<dyn Store>, KupferdrahtError> {
    // Refuse before resolving the home: a refused id must leave nothing
    // behind, not even the home directory (Step 80's pin).
    refuse_path_unsafe_account_id(account_id)?;
    let home = KupferdrahtHome::open(base_dir.to_path_buf())?;
    open_store_in(&home, keystore, account_id)
}

/// Step 80: the account id is runtime data (login identity from the
/// server, registry rows) and becomes a path component — refuse
/// path-unsafe ids at the materialization sites, regardless of which
/// constructor built the id upstream.
fn refuse_path_unsafe_account_id(account_id: &str) -> Result<(), KupferdrahtError> {
    if is_path_safe_id(account_id) {
        Ok(())
    } else {
        Err(KupferdrahtError::InvalidInput(format!(
            "open_default_store: refusing path-unsafe account id {account_id:?}"
        )))
    }
}

/// [`open_default_store`] on an already-resolved home — the default
/// [`StorageProvider`] and the manager come through here.
///
/// # Errors
/// As [`open_default_store`].
pub(crate) fn open_store_in(
    home: &KupferdrahtHome,
    keystore: &Arc<dyn SecureKeyStore>,
    account_id: &str,
) -> Result<Arc<dyn Store>, KupferdrahtError> {
    refuse_path_unsafe_account_id(account_id)?;
    #[cfg(not(target_arch = "wasm32"))]
    {
        let dir = home.account_dir(account_id);
        std::fs::create_dir_all(&dir)
            .map_err(|e| KupferdrahtError::Storage(format!("mkdir {}: {e}", dir.display())))?;
    }
    let database = home.account_db(account_id);
    let key = load_or_create_key_locked(
        home,
        KeyLock::Account(account_id),
        keystore,
        &key_id::account_db(home.home_id(), account_id),
        &database,
    )?;
    Ok(Arc::new(SqliteStore::open(&database, &key)?))
}

/// How the SDK obtains its stores. The consumer either lets the SDK use
/// the shipped default (`SQLite` under a `base_dir`) or injects their own
/// implementation — e.g. a server-side backend for cloud bots. The
/// provider opens **ready** stores; the concrete store does its own
/// encryption (the SDK never assumes a key-based scheme exists). `mls.db`
/// is opened through a crate-internal seam (`open_mls_provider`), not this
/// public trait — see `docs/plan/archive/61-mls-storage-pluggable.md`.
pub trait StorageProvider: Send + Sync {
    /// Open the device-level account registry, ready to use.
    ///
    /// # Errors
    /// Backend-defined; the default surfaces `KupferdrahtError::Storage` /
    /// `KupferdrahtError::Generic`.
    fn open_global(&self) -> Result<Arc<dyn GlobalStore>, KupferdrahtError>;

    /// Open the per-account app-data store, ready to use.
    ///
    /// # Errors
    /// Backend-defined; the default surfaces `KupferdrahtError::Storage` /
    /// `KupferdrahtError::Generic`.
    fn open_store(&self, account_id: &str) -> Result<Arc<dyn Store>, KupferdrahtError>;
}

/// The shipped default [`StorageProvider`]: `SQLite` under `base_dir`,
/// keyed from `keystore`. The keystore is a dependency of **this default
/// implementation only** — the trait and custom providers are
/// keystore-free.
pub(crate) struct SqliteStorageProvider {
    /// The resolved home (Step 93): its id is read once, here, and every
    /// store this provider opens keys under it.
    home: KupferdrahtHome,
    keystore: Arc<dyn SecureKeyStore>,
}

impl SqliteStorageProvider {
    pub(crate) fn new(home: KupferdrahtHome, keystore: Arc<dyn SecureKeyStore>) -> Self {
        Self { home, keystore }
    }
}

impl StorageProvider for SqliteStorageProvider {
    fn open_global(&self) -> Result<Arc<dyn GlobalStore>, KupferdrahtError> {
        open_global_store_in(&self.home, &self.keystore)
    }

    fn open_store(&self, account_id: &str) -> Result<Arc<dyn Store>, KupferdrahtError> {
        open_store_in(&self.home, &self.keystore, account_id)
    }
}

#[cfg(test)]
mod seed_keys_tests;

/// The error of a refused open of `database` in the browser (Step 111a,
/// ADR-0035): `SQLITE_BUSY` — another instance holds it — is
/// `HeldElsewhere`, `SQLITE_FULL` is `StorageFull`, anything else stays a
/// storage error.
#[cfg(any(target_arch = "wasm32", test))]
pub(crate) fn browser_open_error(database: &Path, error: &rusqlite::Error) -> KupferdrahtError {
    let name = database.display().to_string();
    match error.sqlite_error_code() {
        Some(rusqlite::ErrorCode::DatabaseBusy) => KupferdrahtError::HeldElsewhere(name),
        Some(rusqlite::ErrorCode::DiskFull) => KupferdrahtError::StorageFull(name),
        Some(rusqlite::ErrorCode::CannotOpen) => KupferdrahtError::NothingStored(name),
        Some(rusqlite::ErrorCode::AuthorizationForStatementDenied) => {
            KupferdrahtError::StorageAccessDenied(name)
        }
        // Step 120 (ADR-0040): another device holds the account's slot.
        Some(rusqlite::ErrorCode::PermissionDenied) => KupferdrahtError::DeviceRetired(name),
        _ => KupferdrahtError::Storage(format!("open {name}: {error}")),
    }
}

#[cfg(test)]
mod browser_open_error_tests {
    use std::path::Path;

    use super::browser_open_error;
    use crate::error::KupferdrahtError;

    fn failure(code: rusqlite::ErrorCode, extended: i32) -> rusqlite::Error {
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error {
                code,
                extended_code: extended,
            },
            None,
        )
    }

    const DB: &str = "kupferdraht/accounts/a@b/kupferdraht.db";

    /// Step 120 (ADR-0040): another device holds the account's slot.
    #[test]
    fn a_database_another_device_holds_is_device_retired() {
        let e = browser_open_error(
            Path::new(DB),
            &failure(rusqlite::ErrorCode::PermissionDenied, 3),
        );
        assert!(
            matches!(&e, KupferdrahtError::DeviceRetired(db) if db.contains("kupferdraht.db")),
            "{e:?}"
        );
    }

    #[test]
    fn a_busy_database_is_held_elsewhere() {
        let e = browser_open_error(
            Path::new(DB),
            &failure(rusqlite::ErrorCode::DatabaseBusy, 5),
        );
        assert!(
            matches!(&e, KupferdrahtError::HeldElsewhere(db) if db.contains("kupferdraht.db")),
            "{e:?}"
        );
    }

    #[test]
    fn a_full_disk_is_storage_full() {
        let e = browser_open_error(Path::new(DB), &failure(rusqlite::ErrorCode::DiskFull, 13));
        assert!(matches!(e, KupferdrahtError::StorageFull(_)), "{e:?}");
    }

    /// Step 111b: `resume` opens without creating — a database the server
    /// no longer has is `NothingStored`.
    #[test]
    fn a_missing_database_is_nothing_stored() {
        let e = browser_open_error(Path::new(DB), &failure(rusqlite::ErrorCode::CannotOpen, 14));
        assert!(matches!(e, KupferdrahtError::NothingStored(_)), "{e:?}");
    }

    /// Step 111b: the storage server rejected the token.
    #[test]
    fn a_rejected_token_is_no_storage_access() {
        let e = browser_open_error(
            Path::new(DB),
            &failure(rusqlite::ErrorCode::AuthorizationForStatementDenied, 23),
        );
        assert!(
            matches!(e, KupferdrahtError::StorageAccessDenied(_)),
            "{e:?}"
        );
    }

    #[test]
    fn anything_else_stays_a_storage_error() {
        let e = browser_open_error(
            Path::new(DB),
            &failure(rusqlite::ErrorCode::NotADatabase, 26),
        );
        assert!(matches!(e, KupferdrahtError::Storage(_)), "{e:?}");
    }
}

#[cfg(test)]
mod key_unavailable_hint_tests {
    use std::path::Path;

    use super::key_unavailable_hint;

    /// Step 96 review: the registry is not in an account directory.
    #[test]
    fn the_registry_and_the_account_databases_get_their_own_way_out() {
        let registry = key_unavailable_hint(Path::new("/home/global.db"));
        assert!(registry.contains("global.db"), "{registry}");
        assert!(!registry.contains("account directory"), "{registry}");
        for account_db in [
            "/home/accounts/a@b/kupferdraht.db",
            "/home/accounts/a@b/mls.db",
        ] {
            let hint = key_unavailable_hint(Path::new(account_db));
            assert!(hint.contains("account directory"), "{account_db}: {hint}");
        }
    }
}
