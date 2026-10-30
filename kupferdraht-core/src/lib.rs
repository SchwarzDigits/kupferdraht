// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![deny(warnings)]
#![deny(clippy::all)]
#![deny(clippy::pedantic)]
#![deny(unused_must_use)]
#![allow(clippy::module_name_repetitions)]
// The one permanent wasm relaxation: the browser target is single-threaded, so
// `Arc` of a `!Send` value (JS state) is intentional. Every other lint stays at
// `deny` on wasm too — the few transitional cases (stub `async` fns, native-only
// path helpers, a by-value stub arg) carry their own **targeted** `#[allow]` at
// the site, so real issues aren't masked crate-wide.
#![cfg_attr(target_arch = "wasm32", allow(clippy::arc_with_non_send_sync))]

uniffi::setup_scaffolding!("kupferdraht");

pub mod client;
pub mod error;
// The UniFFI C-ABI wrapper layer is native/mobile only. The browser target
// binds the core via `wasm-bindgen` (a later step), so `ffi` is gated out on
// wasm. See docs/plan/44-*.
#[cfg(not(target_arch = "wasm32"))]
pub mod ffi;
pub mod identity;
pub mod keystore;
// Step 117: Kupferdraht's logs reach the app through a listener (G11).
pub mod logging;
pub mod manager;
pub mod rooms;
pub mod storage;
pub mod sync;
pub mod timeline;

// Browser (`wasm-bindgen`) binding layer — W10. Parallel to `ffi`, wasm-only.
#[cfg(target_arch = "wasm32")]
pub mod wasm;

// Implementation detail — Wire-specific transport + openmls glue.
// Not part of the matrix-rust-sdk-shaped public surface (ADR-0009).
// Consumed only by sibling modules inside `kupferdraht-core`; integration
// tests and kupferdraht-cli use the curated facade re-exported below.
pub(crate) mod encryption;
// Step 112 (ADR-0035): recovery data — the browser's account seed sealed
// with a PIN. Compiled for the browser and for tests.
#[cfg(any(target_arch = "wasm32", test))]
pub(crate) mod recovery;
pub(crate) mod redact;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod secret_debug_tests;
pub(crate) mod time;
pub(crate) mod wire;

pub use client::{
    Account, Client, ClientBuilder, FileSharingPolicy, JoinSummary, PendingClient, ReceiptMode,
    ResyncSummary, ServerConfig,
};
pub use error::KupferdrahtError;
pub use identity::UserId;
pub use manager::{Kupferdraht, KupferdrahtBuilder, PendingAccount, PendingLogin};
pub use rooms::{
    DeviceCredential, JoinLink, Room, RoomAccess, RoomDevice, RoomDevicesChange,
    RoomDevicesListener, RoomMember, RoomMemberRole, RoomType,
};
pub use storage::{AccountEntry, GlobalStore, StorageProvider};
pub use sync::{EventHandler, Message, MessageBody, SyncSettings, SyncSummary, SyncTransport};
pub use time::now_ms;
pub use timeline::{AssetSummary, MessageHandle, ResendSummary, Subscription, Timeline};
