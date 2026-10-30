// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! The browser's server mode (Step 111b, ADR-0035, ADR-0036): what an
//! account's databases on a `sqlite-remote-server` need besides the VFS —
//! the storage-login key the VFS signs the login with, the storage token
//! from the token exchange (bought with a Wire session), and the cache the
//! VFS's synchronous token source reads. Compiled for the browser and for
//! tests; the browser's registration of the databases is `server`.

pub(crate) mod health;
pub(crate) mod instance_id;
pub(crate) mod login_key;
#[cfg(target_arch = "wasm32")]
pub(crate) mod server;
// Step 120 (ADR-0040): the account's storage slot — is there a device, which.
pub(crate) mod slot;
pub(crate) mod token_cache;
pub(crate) mod token_exchange;
pub(crate) mod wire_session;
