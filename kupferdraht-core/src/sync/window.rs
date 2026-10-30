// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! The window of one drained page (Step 97e, ADR-0032; per page since
//! Step 109, ADR-0037): the MLS changes and the data of a page commit
//! together — the data first, then the MLS state.
//!
//! Opened per page by the drain: take the MLS write gate, begin the
//! `mls.db` window, begin the store's page transaction. Every
//! notification of the page starts with a savepoint in both databases
//! and in the outbox of held notifications; rolling back to it undoes
//! that notification alone. Dropped without a commit — an error, an
//! unwind — the whole page rolls back and is replayed with its keys
//! intact.

use std::sync::{Arc, Mutex as StdMutex};

use crate::client::Client;
use crate::encryption::{MlsClient, lock_mls};
use crate::error::KupferdrahtError;
use crate::storage::Store;
use crate::sync::outbox;

/// An open page window; rolls back on drop unless committed.
pub(super) struct PageWindow<'a> {
    mls: Option<&'a Arc<StdMutex<MlsClient>>>,
    _gate: Option<tokio::sync::MutexGuard<'a, ()>>,
    store: Option<Arc<dyn Store>>,
    /// The outbox length at each notification's savepoint.
    marks: Vec<usize>,
    open: bool,
}

impl<'a> PageWindow<'a> {
    /// Open the window. Lock order: the MLS write gate, then the page.
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` when `mls.db`'s write lock stays busy or the
    /// store's page cannot begin.
    pub(super) async fn open(client: &'a Client) -> Result<Self, KupferdrahtError> {
        let mls = client.mls_client_required("page window").ok();
        let gate = match mls {
            Some(_) => Some(client.mls_write_gate().await),
            None => None,
        };
        if let Some(mls) = mls {
            lock_mls(mls).begin_notification_window()?;
        }
        let store = client.store_handle().cloned();
        if let Some(store) = &store
            && let Err(e) = store.begin_page().await
        {
            if let Some(mls) = mls {
                lock_mls(mls).rollback_notification_window();
            }
            return Err(e);
        }
        Ok(Self {
            mls,
            _gate: gate,
            store,
            marks: Vec::new(),
            open: true,
        })
    }

    /// Start the page's notification `index`.
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` when a savepoint cannot be set.
    pub(super) async fn savepoint(&mut self, index: u32) -> Result<(), KupferdrahtError> {
        debug_assert_eq!(self.marks.len(), index as usize, "notifications in order");
        if let Some(mls) = self.mls {
            lock_mls(mls).savepoint_notification(index)?;
        }
        if let Some(store) = &self.store {
            store.savepoint(index).await?;
        }
        self.marks.push(outbox::mark());
        Ok(())
    }

    /// Undo the page's notification `index`; the ones before it stay.
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` when a rollback fails.
    pub(super) async fn rollback_to(&mut self, index: u32) -> Result<(), KupferdrahtError> {
        if let Some(store) = &self.store {
            store.rollback_to_savepoint(index).await?;
        }
        if let Some(mls) = self.mls {
            lock_mls(mls).rollback_to_notification(index)?;
        }
        if let Some(mark) = self.marks.get(index as usize) {
            outbox::truncate(*mark);
        }
        Ok(())
    }

    /// Commit the page: its data first, then its MLS changes.
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` when a commit fails. A failed data commit
    /// rolls both back; a failed MLS commit after the data leaves the
    /// data stored and the MLS changes undone — the replay dedupes.
    pub(super) async fn commit(mut self) -> Result<(), KupferdrahtError> {
        self.open = false;
        if let Some(store) = &self.store
            && let Err(e) = store.commit_page().await
        {
            if let Some(mls) = self.mls {
                lock_mls(mls).rollback_notification_window();
            }
            return Err(e);
        }
        if let Some(mls) = self.mls {
            lock_mls(mls).commit_notification_window()?;
        }
        Ok(())
    }

    /// Roll the whole page back.
    pub(super) fn rollback(mut self) {
        self.abort();
    }

    fn abort(&mut self) {
        if !self.open {
            return;
        }
        self.open = false;
        if let Some(store) = &self.store {
            store.rollback_page();
        }
        if let Some(mls) = self.mls {
            lock_mls(mls).rollback_notification_window();
        }
    }
}

impl Drop for PageWindow<'_> {
    fn drop(&mut self) {
        self.abort();
    }
}
