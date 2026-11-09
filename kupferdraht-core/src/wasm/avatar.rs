// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 144 — avatars as the browser sees them: `{ bytes: Uint8Array, mimeType }`
//! or `null`, and the size by its name. Built by hand, since the one serializer
//! of the views writes bytes as a plain array.

use wasm_bindgen::JsValue;

use crate::avatar::{Avatar, AvatarSize};

/// The size a page names: `"thumbnail"` or `"full"`.
///
/// # Errors
/// `InvalidInput` for any other name.
pub fn avatar_size(name: &str) -> Result<AvatarSize, JsValue> {
    match name {
        "thumbnail" => Ok(AvatarSize::Thumbnail),
        "full" => Ok(AvatarSize::Full),
        other => Err(super::manager::named_error(
            "InvalidInput",
            &format!("unknown avatar size: {other}"),
        )),
    }
}

/// An image for the page, `null` for none.
///
/// # Errors
/// An `Encoding` error should the object not take its fields.
pub fn avatar_to_js(avatar: Option<Avatar>) -> Result<JsValue, JsValue> {
    let Some(avatar) = avatar else {
        return Ok(JsValue::NULL);
    };
    let view = js_sys::Object::new();
    let bytes = js_sys::Uint8Array::from(avatar.bytes.as_slice());
    for (name, value) in [
        ("bytes", JsValue::from(bytes)),
        ("mimeType", JsValue::from_str(&avatar.mime_type)),
    ] {
        js_sys::Reflect::set(&view, &JsValue::from_str(name), &value)
            .map_err(|_| super::manager::named_error("Encoding", "avatar view"))?;
    }
    Ok(view.into())
}
