// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `KupferdrahtProvider` — Kupferdraht's openmls provider: stateless [`RustCrypto`]
//! (crypto + randomness) plus the `mls.db` connection, so signing keys,
//! `KeyPackage` private halves and per-group epoch state survive a
//! process restart (PLAN.md Step 1c.5).
//!
//! The storage backend is a [`SqliteStorageProvider`] over a whole-DB-
//! encrypted connection (`openmls_sqlite_storage`) on **every** target:
//! production callers open a file-backed `rusqlite::Connection` — a file
//! under `base_dir` on native, a file in the OPFS pool in the browser
//! (Step 91, ADR-0022) — and tests use `Connection::open_in_memory()`.
//!
//! **One transaction per MLS operation (Step 97c).** openmls writes one
//! record per statement — a commit merge is about a dozen — and expects
//! the application to make an operation atomic. Every operation
//! therefore runs through [`KupferdrahtProvider::with_write_tx`] (`BEGIN
//! IMMEDIATE … COMMIT`, rolled back on error or unwind) or
//! [`KupferdrahtProvider::with_read_tx`] (a consistent snapshot), and openmls
//! sees a [`TxProvider`] whose storage borrows that connection. The
//! write lock also serialises operations across processes and `Client`s
//! (`busy_timeout`, ADR-0024).
//!
//! **A notification window (Step 97e, ADR-0032).** The drain keeps one
//! write transaction open around an inbound notification — decrypt,
//! persist its data to `kupferdraht.db`, then commit here. Inside a window
//! only the inbound operations (`with_inbound_tx`) may write; every other
//! write fails closed, so an MLS write that did not wait for the
//! `Client`'s write gate can never be rolled back with a notification.

use std::cell::Cell;
use std::fmt;

use openmls_rust_crypto::RustCrypto;
use openmls_sqlite_storage::{Codec, Connection, SqliteStorageProvider};
use openmls_traits::OpenMlsProvider;
use rusqlite::{Transaction, TransactionBehavior};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::KupferdrahtError;

/// The openmls storage backend of one operation: a view of the
/// provider's connection (see module docs).
pub(crate) type Storage<'a> = SqliteStorageProvider<JsonCodec, &'a Connection>;

/// JSON codec for `SqliteStorageProvider`. openmls serializes its
/// persistent records through this codec; JSON is a safe default with
/// no version-stability guarantee from openmls itself — that is the
/// `STORAGE_PROVIDER_VERSION` contract's responsibility, not ours.
#[derive(Default)]
pub struct JsonCodec;

impl Codec for JsonCodec {
    type Error = serde_json::Error;

    fn to_vec<T: Serialize>(value: &T) -> Result<Vec<u8>, Self::Error> {
        serde_json::to_vec(value)
    }

    fn from_slice<T: DeserializeOwned>(slice: &[u8]) -> Result<T, Self::Error> {
        serde_json::from_slice(slice)
    }
}

/// `RustCrypto` plus the owned `mls.db` connection.
///
/// Owns the underlying `rusqlite::Connection` so the storage lifetime is
/// tied to the provider's lifetime — dropping the provider releases the
/// file handle (native) or the pool slot (browser) cleanly.
pub struct KupferdrahtProvider {
    crypto: RustCrypto,
    conn: Connection,
    /// Whether the running top-level transaction may write — a nested
    /// write inside a read snapshot is a programming error.
    write_open: Cell<bool>,
    /// A notification window is open (Step 97e).
    window_open: Cell<bool>,
    /// An inbound operation runs inside the open window right now.
    window_op: Cell<bool>,
    /// The database's path in the browser, whose VFS failure every
    /// operation checks first (Step 113, like `SqliteStore`).
    #[cfg(target_arch = "wasm32")]
    path: Option<std::path::PathBuf>,
}

/// The openmls provider of one MLS operation: `RustCrypto` plus a
/// storage view whose writes all land in the operation's transaction.
pub(crate) struct TxProvider<'a> {
    crypto: &'a RustCrypto,
    storage: Storage<'a>,
}

impl fmt::Debug for KupferdrahtProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KupferdrahtProvider")
            .field("storage", &"<store>")
            .finish_non_exhaustive()
    }
}

impl KupferdrahtProvider {
    /// Wrap a `rusqlite::Connection` and run the `openmls_sqlite_storage`
    /// migrations against it. Idempotent — a second call on a
    /// connection that has already been migrated is a no-op.
    ///
    /// # Errors
    /// Returns `KupferdrahtError::Storage` if migrations fail.
    pub fn open(mut conn: Connection) -> Result<Self, KupferdrahtError> {
        SqliteStorageProvider::<JsonCodec, &mut Connection>::new(&mut conn)
            .run_migrations()
            .map_err(|e| KupferdrahtError::Storage(format!("openmls migrations: {e}")))?;
        Ok(Self {
            crypto: RustCrypto::default(),
            conn,
            write_open: Cell::new(false),
            window_open: Cell::new(false),
            window_op: Cell::new(false),
            #[cfg(target_arch = "wasm32")]
            path: None,
        })
    }

    /// Check the VFS failure of the database at `path` before every
    /// operation (Step 113): the browser's `mls.db`.
    #[cfg(target_arch = "wasm32")]
    pub(crate) fn watch_database(&mut self, path: std::path::PathBuf) {
        self.path = Some(path);
    }

    /// Before an operation (Step 113, ADR-0038): the database's VFS
    /// failure decides — a storage outage fails at once, unless the VFS has
    /// healed the database by the time a cheap read reaches it.
    #[cfg(target_arch = "wasm32")]
    fn check_storage(&self) -> Result<(), KupferdrahtError> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        crate::storage::check_database(path, || {
            self.conn
                .query_row("SELECT count(*) FROM sqlite_master", [], |row| {
                    row.get::<_, i64>(0)
                })
                .is_ok()
        })
    }

    /// Close the `mls.db` connection (Step 114: a purge deletes it next).
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` when `SQLite` cannot close it.
    #[cfg(any(target_arch = "wasm32", test))]
    pub(crate) fn close(self) -> Result<(), KupferdrahtError> {
        self.conn
            .close()
            .map_err(|(_, e)| KupferdrahtError::Storage(format!("close mls.db: {e}")))
    }

    /// Convenience constructor for tests and other call sites that do
    /// not need persistence across process restarts. Opens an in-memory
    /// `SQLite` database and runs the openmls migrations.
    ///
    /// # Errors
    /// Returns `KupferdrahtError::Storage` if the in-memory connection or
    /// the migrations fail.
    pub fn open_in_memory() -> Result<Self, KupferdrahtError> {
        let conn = Connection::open_in_memory()
            .map_err(|e| KupferdrahtError::Storage(format!("open_in_memory: {e}")))?;
        Self::open(conn)
    }

    /// Run one MLS operation that writes inside one `BEGIN IMMEDIATE`
    /// transaction: all its writes commit together, an error or an
    /// unwind rolls all of them back. Inside a running operation it
    /// joins that operation's transaction.
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` when the transaction cannot begin (the
    /// write lock stayed busy past `busy_timeout`) or commit; else
    /// whatever `op` returns.
    pub(crate) fn with_write_tx<T>(
        &self,
        op: impl FnOnce(&TxProvider<'_>) -> Result<T, KupferdrahtError>,
    ) -> Result<T, KupferdrahtError> {
        if self.window_open.get() && !self.window_op.get() {
            return Err(KupferdrahtError::Generic(
                "MLS write while a notification window is open — take the write gate first".into(),
            ));
        }
        self.with_tx(TransactionBehavior::Immediate, op)
    }

    /// Run an inbound operation (decrypt, absorb a Welcome): inside an
    /// open notification window it joins that window, else it is one
    /// write transaction like [`Self::with_write_tx`].
    ///
    /// # Errors
    /// As [`Self::with_write_tx`].
    pub(crate) fn with_inbound_tx<T>(
        &self,
        op: impl FnOnce(&TxProvider<'_>) -> Result<T, KupferdrahtError>,
    ) -> Result<T, KupferdrahtError> {
        if !self.window_open.get() {
            return self.with_write_tx(op);
        }
        debug_assert!(
            !self.conn.is_autocommit(),
            "an open window is a transaction"
        );
        self.window_op.set(true);
        let out = op(&self.tx_provider());
        self.window_op.set(false);
        out
    }

    /// Open a notification window: one write transaction that the
    /// inbound operations of a drained page join (Step 109, ADR-0037), with
    /// a savepoint per notification.
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` when the write lock stays busy past
    /// `busy_timeout`.
    pub(crate) fn begin_window(&self) -> Result<(), KupferdrahtError> {
        #[cfg(target_arch = "wasm32")]
        self.check_storage()?;
        assert!(
            self.conn.is_autocommit() && !self.window_open.get(),
            "a notification window opens outside any transaction"
        );
        self.conn
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|e| KupferdrahtError::Storage(format!("MLS window begin: {e}")))?;
        self.window_open.set(true);
        self.write_open.set(true);
        Ok(())
    }

    /// Commit the open notification window.
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` when the commit fails (the window's changes
    /// are then rolled back).
    pub(crate) fn commit_window(&self) -> Result<(), KupferdrahtError> {
        assert!(
            self.window_open.get(),
            "commit_window without an open window"
        );
        self.window_open.set(false);
        if let Err(e) = self.conn.execute_batch("COMMIT") {
            self.rollback_open_tx();
            return Err(KupferdrahtError::Storage(format!("MLS window commit: {e}")));
        }
        Ok(())
    }

    /// Mark the start of notification `index` inside the open window
    /// (Step 109, ADR-0037), so that [`Self::rollback_to_savepoint`] can
    /// undo that notification alone.
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` on `SQLite` failure.
    pub(crate) fn savepoint(&self, index: u32) -> Result<(), KupferdrahtError> {
        assert!(self.window_open.get(), "a savepoint inside an open window");
        self.conn
            .execute_batch(&format!("SAVEPOINT notification_{index}"))
            .map_err(|e| KupferdrahtError::Storage(format!("MLS window savepoint {index}: {e}")))
    }

    /// Undo the window's MLS changes since [`Self::savepoint`]`(index)`;
    /// what came before stays in the window.
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` on `SQLite` failure.
    pub(crate) fn rollback_to_savepoint(&self, index: u32) -> Result<(), KupferdrahtError> {
        assert!(self.window_open.get(), "a rollback inside an open window");
        self.conn
            .execute_batch(&format!("ROLLBACK TO notification_{index}"))
            .map_err(|e| KupferdrahtError::Storage(format!("MLS window rollback to {index}: {e}")))
    }

    /// Roll the open notification window back (a no-op without one).
    pub(crate) fn rollback_window(&self) {
        self.window_open.set(false);
        self.window_op.set(false);
        self.rollback_open_tx();
    }

    fn rollback_open_tx(&self) {
        if !self.conn.is_autocommit()
            && let Err(e) = self.conn.execute_batch("ROLLBACK")
        {
            tracing::warn!(error = %e, "MLS window rollback failed");
        }
    }

    /// Run one read-only MLS operation on a consistent snapshot (a
    /// deferred transaction): a multi-row load never mixes two epochs
    /// another process is writing. Inside a running operation it joins
    /// that operation's transaction.
    ///
    /// # Errors
    /// As [`Self::with_write_tx`].
    pub(crate) fn with_read_tx<T>(
        &self,
        op: impl FnOnce(&TxProvider<'_>) -> Result<T, KupferdrahtError>,
    ) -> Result<T, KupferdrahtError> {
        self.with_tx(TransactionBehavior::Deferred, op)
    }

    /// Begin (or join), run `op`, commit — the dropped transaction rolls
    /// back when `op` fails or unwinds.
    fn with_tx<T>(
        &self,
        behavior: TransactionBehavior,
        op: impl FnOnce(&TxProvider<'_>) -> Result<T, KupferdrahtError>,
    ) -> Result<T, KupferdrahtError> {
        #[cfg(target_arch = "wasm32")]
        self.check_storage()?;
        let writes = matches!(behavior, TransactionBehavior::Immediate);
        if !self.conn.is_autocommit() {
            assert!(
                !writes || self.write_open.get(),
                "a writing MLS operation must not run inside a read snapshot"
            );
            return op(&self.tx_provider());
        }
        let tx = Transaction::new_unchecked(&self.conn, behavior)
            .map_err(|e| KupferdrahtError::Storage(format!("MLS transaction begin: {e}")))?;
        self.write_open.set(writes);
        let out = op(&self.tx_provider())?;
        tx.commit()
            .map_err(|e| KupferdrahtError::Storage(format!("MLS transaction commit: {e}")))?;
        debug_assert!(
            self.conn.is_autocommit(),
            "the operation's transaction ended"
        );
        Ok(out)
    }

    /// The openmls provider over this connection.
    fn tx_provider(&self) -> TxProvider<'_> {
        TxProvider {
            crypto: &self.crypto,
            storage: SqliteStorageProvider::new(&self.conn),
        }
    }
}

#[cfg(test)]
impl KupferdrahtProvider {
    /// Test-only: an openmls provider outside any transaction (one
    /// autocommit per statement, as before Step 97c) — for raw openmls
    /// fixtures in unit tests, never for an `MlsClient` operation.
    pub(crate) fn autocommit_provider(&self) -> TxProvider<'_> {
        self.tx_provider()
    }
}

impl<'a> OpenMlsProvider for TxProvider<'a> {
    type CryptoProvider = RustCrypto;
    type RandProvider = RustCrypto;
    type StorageProvider = Storage<'a>;

    fn storage(&self) -> &Self::StorageProvider {
        &self.storage
    }

    fn crypto(&self) -> &Self::CryptoProvider {
        self.crypto
    }

    fn rand(&self) -> &Self::RandProvider {
        self.crypto
    }
}

// ── Step 97e: the notification window's fail-closed guard ───────────────────
#[cfg(test)]
mod window_tests {
    //! ADR-0032 item 5 pins: while a notification window is open, an MLS
    //! write that did not wait for the `Client`'s write gate is refused
    //! (it could be rolled back with the notification — a send's ratchet
    //! advance above all), an inbound operation joins the window, and
    //! writes run again once the window committed or rolled back.

    use super::KupferdrahtProvider;
    use crate::error::KupferdrahtError;

    #[test]
    fn a_write_that_skipped_the_gate_fails_closed_while_a_window_is_open() {
        let provider = KupferdrahtProvider::open_in_memory().expect("provider");
        provider.begin_window().expect("begin window");

        let refused = provider.with_write_tx(|_| Ok(()));
        assert!(
            matches!(refused, Err(KupferdrahtError::Generic(ref m)) if m.contains("write gate")),
            "an ungated write inside a window must fail closed, got {refused:?}"
        );
        provider
            .with_inbound_tx(|_| Ok(()))
            .expect("an inbound operation joins the window");
        assert!(!provider.conn.is_autocommit(), "the window is still open");

        provider.commit_window().expect("commit window");
        assert!(provider.conn.is_autocommit(), "the window ended");
        provider
            .with_write_tx(|_| Ok(()))
            .expect("writes run again after the window committed");
    }

    #[test]
    fn a_rolled_back_window_admits_writes_again() {
        let provider = KupferdrahtProvider::open_in_memory().expect("provider");
        provider.begin_window().expect("begin window");
        provider.rollback_window();
        assert!(
            provider.conn.is_autocommit(),
            "the rollback ended the window"
        );
        provider
            .with_write_tx(|_| Ok(()))
            .expect("writes run again after the window rolled back");
    }
}
