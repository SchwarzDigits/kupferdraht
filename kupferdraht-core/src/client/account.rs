// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `Account` — the user's own profile, exposed via `Client::account()`.
//!
//! Mirrors `matrix_sdk::Account`. Step 1c only ships read accessors;
//! `set_display_name` / avatar / `update_account_data` etc. live in
//! later Wire-feature scope (out of Step 1).

use std::sync::Arc;

use crate::avatar::AvatarSource;
use crate::identity::UserId;

/// The user's own team as a login found it (Step 143): `id` from `GET /self`,
/// lowercase, `None` for a personal account; `name` from `GET /teams/{tid}`,
/// `None` too when that fetch failed; `avatar_version` the same request's
/// `icon` (Step 144), the id of the icon's image, `None` as well for the
/// default icon.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct OwnTeam {
    pub(crate) id: Option<String>,
    pub(crate) name: Option<String>,
    pub(crate) avatar_version: Option<String>,
}

/// The user's own profile from `GET /v14/self`, as of the last login,
/// `Client::refresh_account` or own profile call (Step 152).
///
/// This is the matrix-rust-sdk-shaped public-API replacement for the
/// backend's profile type. Field names follow matrix-rust-sdk conventions
/// (`display_name`, not the backend's `name`), while their values come
/// from the backend's `/self` response.
#[derive(Debug, Clone)]
pub struct Account {
    user_id: UserId,
    display_name: String,
    username: Option<String>,
    email: Option<String>,
    team: OwnTeam,
    avatar_source: AvatarSource,
}

impl Account {
    pub(crate) fn new(
        user_id: UserId,
        display_name: String,
        username: Option<String>,
        email: Option<String>,
        team: OwnTeam,
        avatar_source: AvatarSource,
    ) -> Arc<Self> {
        Arc::new(Self {
            user_id,
            display_name,
            username,
            email,
            team,
            avatar_source,
        })
    }

    /// The user's own id `<localpart>@<domain>`.
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

    /// The user's unique username, if one is set.
    #[must_use]
    pub fn username(&self) -> Option<&str> {
        self.username.as_deref()
    }

    #[must_use]
    pub fn email(&self) -> Option<&str> {
        self.email.as_deref()
    }

    /// The user's team, as of the last login or `Client::refresh_account`
    /// (Steps 143, 152); `None` for a personal account. Lowercase.
    #[must_use]
    pub fn team_id(&self) -> Option<&str> {
        self.team.id.as_deref()
    }

    /// The team's name (Step 143), as of the last login or
    /// `Client::refresh_account` (Step 152). `None` for a personal account,
    /// and for a team whose name the login could not fetch until a refresh
    /// does.
    #[must_use]
    pub fn team_name(&self) -> Option<&str> {
        self.team.name.as_deref()
    }

    /// The version marker of the team's icon, as of the last login or
    /// refresh (Steps 144, 152): a changed icon has a new one;
    /// [`Client::team_avatar`](super::Client::team_avatar) fetches the image.
    #[must_use]
    pub fn team_avatar_version(&self) -> Option<&str> {
        self.team.avatar_version.as_deref()
    }

    /// Where the own avatar's images are (Step 144). Since Step 152 the
    /// account follows `refresh_account` and the client's own calls.
    #[must_use]
    pub fn avatar_source(&self) -> &AvatarSource {
        &self.avatar_source
    }
}
