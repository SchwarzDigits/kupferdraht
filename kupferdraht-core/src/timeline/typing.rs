// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Typing indicators (`conversation.typing` — ephemeral, *not* a
//! `GenericMessage`) — Step 21, facade split out of the former
//! monolithic `timeline/send.rs` by Step 30.
//!
//! This file holds only the per-room `Timeline` facade. The send-side
//! orchestration — the 11-second auto-`stopped` window, the in-flight
//! dedupe, and the typing POST itself — lives on `Client` (Step 21).

use crate::error::KupferdrahtError;
use crate::timeline::Timeline;

impl Timeline {
    /// Send a typing `started` notification for this room. Kupferdraht
    /// auto-enqueues a `stopped` after an 11-second window unless the
    /// caller sends `stopped` sooner, and dedupes repeat calls while one
    /// is in flight — so the app can call this freely on keystrokes.
    /// Step 21.
    ///
    /// # Errors
    /// * `KupferdrahtError::InvalidInput` — the room id is malformed.
    /// * `KupferdrahtError::BackendRejected` / `Network` — the typing POST
    ///   failed.
    pub async fn send_typing_started(&self) -> Result<(), KupferdrahtError> {
        self.client.send_typing_started(&self.room_id).await
    }

    /// Send a typing `stopped` notification for this room (a no-op when
    /// no `started` is outstanding). Step 21.
    ///
    /// # Errors
    /// * `KupferdrahtError::InvalidInput` — the room id is malformed.
    /// * `KupferdrahtError::BackendRejected` / `Network` — the typing POST
    ///   failed.
    pub async fn send_typing_stopped(&self) -> Result<(), KupferdrahtError> {
        self.client.send_typing_stopped(&self.room_id).await
    }
}
