// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

pub mod dto;
pub(crate) mod http;
pub(crate) mod notification_dto;
// Step 115b: frame parsing without transport, shared by the native socket and
// the browser one.
pub(crate) mod proto;
pub(crate) mod push_frame;
mod version;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod websocket;

pub use http::HttpClient;
#[cfg(any(target_arch = "wasm32", test))]
pub(crate) use http::WEB_DEVICE_MODEL;
pub use version::select_api_version;
