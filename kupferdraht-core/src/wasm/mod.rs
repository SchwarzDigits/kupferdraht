// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Browser (`wasm-bindgen`) binding layer — W10.
//!
//! A JS/TS-callable surface over the same core API the native FFI wraps
//! (the core types carry no `UniFFI` annotations, so this layer is parallel to
//! `ffi`, not layered on it). Built with `wasm-pack build --target web`.
//!
//! Started as a **toolchain spike** (a couple of trivial exports proving
//! `wasm-pack` produces a loadable ES module that coexists with the
//! unconditional `uniffi::setup_scaffolding!`); now the login / sync / send
//! vertical over the `SQLite3MC` store (on the storage server, ADR-0035, or
//! in OPFS in the local mode, Step 91, ADR-0022) — which
//! is why the surface must be driven from a **dedicated Web Worker**.
#![allow(clippy::unused_unit)]

mod devices;
mod live_sync;
// Step 117: Kupferdraht's logs to the app's `onLog`.
mod logging;
mod manager;
// Step 115: the doorbell the live-sync loop waits on.
pub mod push_socket;
// Step 122: whether an account's sync loop runs on push.
pub mod push_state;
// Step 118: how long the live-sync loop waits, when it may reconnect.
pub mod sync_pace;

pub use live_sync::SyncHandle;
pub use manager::{Client, Kupferdraht, PendingLogin, js_error};

use wasm_bindgen::prelude::*;

/// Kupferdraht crate version (`CARGO_PKG_VERSION`) — a trivial export to prove the
/// JS↔wasm boundary end to end.
#[wasm_bindgen]
#[must_use]
pub fn kupferdraht_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// Current wall-clock time in milliseconds since the UNIX epoch, read through
/// the same [`crate::time::now_ms`] shim the rest of the engine uses (W8 —
/// `js_sys::Date::now()` on wasm). Returned as `f64` so it crosses the boundary
/// as a plain JS number.
#[wasm_bindgen]
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn kupferdraht_now_ms() -> f64 {
    crate::time::now_ms() as f64
}
