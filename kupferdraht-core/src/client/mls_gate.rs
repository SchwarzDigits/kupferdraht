// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! The MLS write gate (Step 97e, ADR-0032): one MLS writer at a time
//! with respect to the drain's notification windows.
//!
//! The drain holds the gate while a notification's window is open —
//! decrypt, persist to `kupferdraht.db`, commit the MLS state. Every MLS
//! write outside inbound dispatch (sends, own commit flows, key
//! packages, group creation / join / leave / discard) takes it first,
//! so it never lands in an open window whose rollback would undo it (a
//! send's ratchet advance above all). Reads need no gate. Inbound
//! dispatch never calls a gated write path — those run as follow-ups
//! after the window (the gate is not reentrant). A write that skips the
//! gate while a window is open fails closed in `KupferdrahtProvider`.

use crate::client::Client;

impl Client {
    /// Wait for the MLS write gate; hold the guard around the MLS write
    /// (and no network call — an upload happens between two gated steps).
    pub(crate) async fn mls_write_gate(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.mls_write_gate.lock().await
    }
}
