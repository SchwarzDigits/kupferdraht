// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `Room::typing_notice` — matrix-rust-sdk's typing notice (Step 162). The
//! send side — the 11-second auto-stop, the in-flight dedupe, the request —
//! lives on `Client` (Step 21).

use crate::error::KupferdrahtError;
use crate::rooms::Room;

impl Room {
    /// Tell the room whether this user is typing. `true` may be sent on
    /// every keystroke: repeats while one is outstanding are dropped, and a
    /// `false` follows on its own after 11 seconds unless the application
    /// sends it sooner. matrix-rust-sdk analog: `Room::typing_notice`.
    ///
    /// # Errors
    /// * `InvalidInput` if the owning `Client` has been dropped.
    /// * `BackendRejected` / `Network` from the request.
    pub async fn typing_notice(&self, typing: bool) -> Result<(), KupferdrahtError> {
        self.owning_client("typing_notice")?
            .typing_notice(&self.id(), typing)
            .await
    }
}
