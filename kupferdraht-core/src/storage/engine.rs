// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! [`StorageEngineInfo`] — what an open whole-DB-encrypted store reports
//! about its encryption engine (Step 90, ADR-0021).

/// Engine, cipher scheme and header mode of an open whole-DB-encrypted
/// store, read back from the **live connection** — never from a constant
/// — so `kupferdraht-cli status` (and a binding) shows what is actually
/// linked and in effect. `Store::storage_engine` returns it; backends
/// without such an engine return `None`.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct StorageEngineInfo {
    /// The engine as it names itself (`SQLite3 Multiple Ciphers`).
    pub engine: String,
    /// The engine release (`2.5.1`).
    pub version: String,
    /// The cipher scheme in effect on the connection (`chacha20`).
    pub cipher: String,
    /// Bytes of the SQLite header kept in plaintext (`32` — iOS WAL
    /// identification in a shared container).
    pub plaintext_header_size: u32,
}
