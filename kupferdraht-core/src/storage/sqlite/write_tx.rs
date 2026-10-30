// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! A write transaction that joins an open page (Step 109, ADR-0037).
//!
//! Outside a page a write runs in its own `BEGIN IMMEDIATE` transaction,
//! as before. Inside the drain's page transaction it becomes a savepoint:
//! its writes still land together or not at all, and they commit with
//! the page.

use std::ops::Deref;

use rusqlite::{Connection, Savepoint, Transaction, TransactionBehavior};

/// The transaction of one write: its own, or a savepoint in the page.
pub(super) enum WriteTx<'c> {
    Own(Transaction<'c>),
    InPage(Savepoint<'c>),
}

/// Begin the transaction of one write on `conn`.
///
/// # Errors
/// The `rusqlite` error when the transaction or savepoint cannot begin.
pub(super) fn begin(conn: &mut Connection) -> rusqlite::Result<WriteTx<'_>> {
    if conn.is_autocommit() {
        conn.transaction_with_behavior(TransactionBehavior::Immediate)
            .map(WriteTx::Own)
    } else {
        conn.savepoint().map(WriteTx::InPage)
    }
}

impl WriteTx<'_> {
    /// Commit the write — into the page when it runs inside one.
    ///
    /// # Errors
    /// The `rusqlite` error when the commit or release fails.
    pub(super) fn commit(self) -> rusqlite::Result<()> {
        match self {
            Self::Own(tx) => tx.commit(),
            Self::InPage(sp) => sp.commit(),
        }
    }
}

impl Deref for WriteTx<'_> {
    type Target = Connection;

    fn deref(&self) -> &Connection {
        match self {
            Self::Own(tx) => tx,
            Self::InPage(sp) => sp,
        }
    }
}
