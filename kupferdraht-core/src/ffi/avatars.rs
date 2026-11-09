// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! FFI wrappers for avatars (Step 144). `Avatar` and `AvatarSize` are FFI-shaped
//! and derive on the core type; a user crosses as their qualified id.

use super::Client;
use crate::avatar::{Avatar, AvatarSize};
use crate::error::KupferdrahtError;
use crate::identity::UserId;

#[uniffi::export(async_runtime = "tokio")]
impl Client {
    /// A user's avatar in `size`, an image Kupferdraht wrote; `None` without one.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn avatar(
        &self,
        user_id: String,
        size: AvatarSize,
    ) -> Result<Option<Avatar>, KupferdrahtError> {
        self.inner.avatar(&UserId::parse(&user_id)?, size).await
    }

    /// The version marker of a user's avatar: a changed avatar has a new key.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn avatar_key(&self, user_id: String) -> Result<Option<String>, KupferdrahtError> {
        self.inner.avatar_key(&UserId::parse(&user_id)?).await
    }

    /// The icon of the account's team as of the last login; `None` without one.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn team_avatar(&self) -> Result<Option<Avatar>, KupferdrahtError> {
        self.inner.team_avatar().await
    }

    /// Set the own avatar from one image: JPEG, PNG, GIF or WebP.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn upload_avatar(&self, image: Vec<u8>) -> Result<(), KupferdrahtError> {
        self.inner.upload_avatar(image).await
    }

    /// Remove the own avatar.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn remove_avatar(&self) -> Result<(), KupferdrahtError> {
        self.inner.remove_avatar().await
    }
}
