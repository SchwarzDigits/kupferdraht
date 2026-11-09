// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! The one serializer of the browser's views (Step 130c): a missing value
//! crosses as `null`, as the hand-written types of the tabs module declare
//! it, and a thin tab compares against it. A field a view skips stays absent.

use serde::Serialize;
use wasm_bindgen::JsValue;

/// Serialize a view for the page, `None` as `null`.
///
/// # Errors
/// An `Encoding` error when the value cannot be serialized.
pub fn to_js<T: Serialize>(view: &T) -> Result<JsValue, JsValue> {
    let serializer = serde_wasm_bindgen::Serializer::new().serialize_missing_as_null(true);
    view.serialize(&serializer)
        .map_err(|e| super::manager::named_error("Encoding", &e.to_string()))
}
