// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Parsing one legacy `/await` frame — no transport, so it holds on every
//! target (Step 115b).
//!
//! It lived in `websocket.rs` until the browser needed it: that module is
//! native-only because `tokio-tungstenite` must never enter the wasm dependency
//! graph, while this is `serde_json` over a DTO that was already portable. Two
//! callers now, one implementation: the native `EventSocket` and
//! `wasm/push_socket.rs`.

use crate::error::KupferdrahtError;

/// Parse one legacy `/await` text frame into its notification.
///
/// # Errors
/// `KupferdrahtError::Encoding` on malformed JSON or an empty notification id.
pub(crate) fn parse_legacy_frame(
    frame: &str,
) -> Result<crate::wire::notification_dto::QueuedNotification, KupferdrahtError> {
    let notification = serde_json::from_str(frame)
        .map_err(|e| KupferdrahtError::Encoding(format!("await frame decode: {e}")))?;
    check_notification_id(&notification)?;
    Ok(notification)
}

/// Step 95c: the notification id becomes the persisted cursor and the
/// dedup key — an empty one is a malformed frame.
pub(crate) fn check_notification_id(
    notification: &crate::wire::notification_dto::QueuedNotification,
) -> Result<(), KupferdrahtError> {
    if notification.id.is_empty() {
        return Err(KupferdrahtError::Encoding(
            "frame: empty notification id".into(),
        ));
    }
    Ok(())
}
