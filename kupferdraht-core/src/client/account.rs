// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `Account` — the user's own profile, exposed via `Client::account()`.
//!
//! Mirrors `matrix_sdk::Account`. Step 1c only ships read accessors;
//! `set_display_name` / avatar / `update_account_data` etc. live in
//! later Wire-feature scope (out of Step 1).

use std::sync::Arc;

use crate::identity::UserId;

/// User-profile snapshot loaded at login time from `GET /v14/self`.
///
/// This is the matrix-rust-sdk-shaped public-API replacement for the
/// internal `wire::dto::SelfUser` type. Field names follow
/// matrix-rust-sdk conventions (`display_name`, not Wire's `name`),
/// while their values come from Wire's `/self` response.
#[derive(Debug, Clone)]
pub struct Account {
    user_id: UserId,
    display_name: String,
    handle: Option<String>,
    email: Option<String>,
}

impl Account {
    pub(crate) fn new(
        user_id: UserId,
        display_name: String,
        handle: Option<String>,
        email: Option<String>,
    ) -> Arc<Self> {
        Arc::new(Self {
            user_id,
            display_name,
            handle,
            email,
        })
    }

    /// The user's own qualified id `<localpart>@<domain>`.
    #[must_use]
    pub fn user_id(&self) -> &UserId {
        &self.user_id
    }

    /// Wire backend domain the user belongs to (e.g. `example.org`).
    /// Always present — carried inside [`UserId`].
    #[must_use]
    pub fn domain(&self) -> &str {
        self.user_id.domain()
    }

    #[must_use]
    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    #[must_use]
    pub fn handle(&self) -> Option<&str> {
        self.handle.as_deref()
    }

    #[must_use]
    pub fn email(&self) -> Option<&str> {
        self.email.as_deref()
    }
}
