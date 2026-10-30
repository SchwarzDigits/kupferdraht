// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 122 — whether an account's sync loop runs on push: `"connected"`
//! while the loop has an open socket, `"polling"` while it runs without
//! one, `"stopped"` without a running loop. The loop records it; the
//! `Client`'s `pushState()` reads it — by account, because several `Client`
//! wrappers can stand for one account's client.

use std::cell::RefCell;
use std::collections::HashMap;

thread_local! {
    /// Account → whether its running sync loop has an open socket.
    static PUSH: RefCell<HashMap<String, bool>> = RefCell::new(HashMap::new());
}

/// The account's push state, as `pushState()` reports it.
#[must_use]
pub fn push_state(account: &str) -> &'static str {
    match PUSH.with(|push| push.borrow().get(account).copied()) {
        Some(true) => "connected",
        Some(false) => "polling",
        None => "stopped",
    }
}

/// Record whether the account's running sync loop has an open socket.
pub fn set_push_state(account: &str, connected: bool) {
    PUSH.with(|push| push.borrow_mut().insert(account.to_owned(), connected));
}

/// Forget the account's push state: its sync loop ended.
pub fn clear_push_state(account: &str) {
    PUSH.with(|push| push.borrow_mut().remove(account));
}
