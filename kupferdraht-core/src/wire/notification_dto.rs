// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg_attr(not(test), allow(dead_code))]
#![allow(clippy::doc_markdown)]
//! Wire's notification-fetch JSON shapes — the REST (`/v14/
//! notifications`) side of the receive path. The push transport's
//! frame model (`ConsumableNotification`, Step 62) lives in
//! `wire/websocket.rs` and reuses [`QueuedNotification`] for the
//! inner notification: both transports deliver the same shape.

use serde::Deserialize;

/// Response of `GET /v{N}/notifications` — the REST queue Kupferdraht
/// drains via `kupferdraht-cli sync-fetch`.
#[derive(Debug, Clone, Deserialize)]
pub struct NotificationListResponse {
    /// Each entry is one queued notification with a UUID v1 id and
    /// a `payload` array of `Event` objects.
    #[serde(default)]
    pub notifications: Vec<QueuedNotification>,
    /// `true` when there is at least one more page beyond the
    /// returned slice. Caller pages with `?since=<last id>` until
    /// `has_more` is `false`.
    #[serde(default)]
    pub has_more: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct QueuedNotification {
    pub id: String,
    #[serde(default)]
    pub payload: serde_json::Value,
    /// Step 73 (ADR-0019): the legacy `/await` socket marks live-only
    /// frames (`conversation.typing`) as transient — they dispatch
    /// but never advance the cursor. REST responses omit the field
    /// (transient events are never enqueued) — defaults to `false`.
    #[serde(default)]
    pub transient: bool,
}
