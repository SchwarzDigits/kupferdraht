// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Avatars on the client (Step 144): whose keys, the cached images written
//! anew, and setting the own. The own keys are the `self_user` row's, with the
//! login's snapshot where there is no row (the Step 126 pattern): `Account` is
//! frozen, and an upload changes them.

use std::sync::PoisonError;

use tracing::warn;

use crate::avatar::{self, AVATAR_MAX_INPUT_BYTES, Avatar, AvatarKeys, AvatarSize};
use crate::client::Client;
use crate::error::KupferdrahtError;
use crate::identity::UserId;
use crate::wire::dto::{QualifiedId, UserAsset};

impl Client {
    /// A user's avatar in `size`, an image Kupferdraht wrote; `Ok(None)` for a
    /// user without one, and for a stored image that is none. A profile with
    /// one size serves both, scaled to the size asked for.
    ///
    /// # Errors
    /// `BackendRejected` and `Network` for a failed lookup or download, or an
    /// answer over [`AVATAR_MAX_INPUT_BYTES`].
    pub async fn avatar(
        &self,
        user: &UserId,
        size: AvatarSize,
    ) -> Result<Option<Avatar>, KupferdrahtError> {
        let keys = self.avatar_keys_of(user).await?;
        match keys.for_size(size) {
            Some(key) => self.written_avatar(user.domain(), key, size).await,
            None => Ok(None),
        }
    }

    /// The version marker of a user's avatar: the thumbnail's asset key, the
    /// full image's when the profile has only that. A changed avatar has a new
    /// key, so an embedder keys its own store by it. A method, not a snapshot:
    /// the own key changes with [`Self::upload_avatar`].
    ///
    /// # Errors
    /// `BackendRejected` and `Network` for a failed lookup.
    pub async fn avatar_key(&self, user: &UserId) -> Result<Option<String>, KupferdrahtError> {
        let keys = self.avatar_keys_of(user).await?;
        Ok(keys.version().map(str::to_owned))
    }

    /// The icon of the account's team, as of the last login, an image
    /// Kupferdraht wrote. `Ok(None)` without a team, for the default icon, and
    /// when the team request failed at login.
    ///
    /// # Errors
    /// `BackendRejected` and `Network` for a failed download.
    pub async fn team_avatar(&self) -> Result<Option<Avatar>, KupferdrahtError> {
        let account = self.account();
        match account.team_avatar_key() {
            Some(key) => {
                self.written_avatar(account.domain(), key, AvatarSize::Full)
                    .await
            }
            None => Ok(None),
        }
    }

    /// Set the own avatar from one image: Kupferdraht decodes it, writes both
    /// sizes anew, uploads them and names them on the profile. The replaced
    /// assets stay on the server. Cropping is the embedder's.
    ///
    /// # Errors
    /// `InvalidInput` for what is no accepted image or exceeds the limits,
    /// before any request; `BackendRejected` and `Network` for a failed upload
    /// or profile update, which leave the current avatar as it is.
    pub async fn upload_avatar(&self, image: Vec<u8>) -> Result<(), KupferdrahtError> {
        let (thumbnail, full) = write_both_sizes(image).await?;
        let thumbnail_key = self.upload_written_image(&thumbnail).await?;
        let full_key = self.upload_written_image(&full).await?;
        let assets = [
            UserAsset::image(&thumbnail_key, UserAsset::PREVIEW),
            UserAsset::image(&full_key, UserAsset::COMPLETE),
        ];
        self.http
            .update_self_assets(self.api_version, &self.current_access_token(), &assets)
            .await?;
        self.remember_own_avatar(AvatarKeys {
            thumbnail: Some(thumbnail_key),
            full: Some(full_key),
        })
        .await
    }

    /// Remove the own avatar. The asset stays on the server.
    ///
    /// # Errors
    /// `BackendRejected` and `Network` for a failed profile update.
    pub async fn remove_avatar(&self) -> Result<(), KupferdrahtError> {
        self.http
            .update_self_assets(self.api_version, &self.current_access_token(), &[])
            .await?;
        self.remember_own_avatar(AvatarKeys::default()).await
    }

    /// The own keys as they are now: a write of this session is in the row.
    pub(super) async fn own_avatar_keys(&self) -> Result<AvatarKeys, KupferdrahtError> {
        let Some(store) = self.store_handle() else {
            return Ok(self.account().login_avatar_keys().clone());
        };
        let row = self.current_self_row(store.as_ref()).await?;
        Ok(AvatarKeys {
            thumbnail: row.avatar_thumbnail_key,
            full: row.avatar_full_key,
        })
    }

    /// A user's keys: the own, a stored member's, else one `list-users` for a
    /// user in no stored room.
    async fn avatar_keys_of(&self, user: &UserId) -> Result<AvatarKeys, KupferdrahtError> {
        if user == self.account().user_id() {
            return self.own_avatar_keys().await;
        }
        if let Some(store) = self.store_handle()
            && let Some(keys) = store.member_avatar_keys(user.as_str()).await?
        {
            return Ok(keys);
        }
        let found = self
            .http
            .list_users(
                self.api_version,
                &self.current_access_token(),
                &[QualifiedId::from(user)],
            )
            .await?
            .found;
        Ok(found
            .iter()
            .find(|profile| profile.qualified_id.to_string() == user.as_str())
            .map_or_else(AvatarKeys::default, |profile| {
                AvatarKeys::from_assets(&profile.assets)
            }))
    }

    /// The image under `key`, from the cache or downloaded and written anew.
    /// What the store holds there is someone else's choice: a key that cannot
    /// be fetched and a file that is no image are "no avatar", with a warning.
    async fn written_avatar(
        &self,
        domain: &str,
        key: &str,
        size: AvatarSize,
    ) -> Result<Option<Avatar>, KupferdrahtError> {
        debug_assert!(!key.is_empty(), "AvatarKeys holds no empty key");
        if let Some(cached) = self.avatar_cache().get(key, size) {
            return Ok(Some(cached));
        }
        let downloaded = self
            .http
            .download_public_image(
                self.api_version,
                &self.current_access_token(),
                domain,
                key,
                AVATAR_MAX_INPUT_BYTES,
            )
            .await;
        let written = match downloaded {
            Ok(input) => rewrite_off_the_runtime(input, size).await,
            Err(e) => Err(e),
        };
        match written {
            Ok(avatar) => {
                self.avatar_cache().put(key, size, &avatar);
                Ok(Some(avatar))
            }
            Err(KupferdrahtError::InvalidInput(reason)) => {
                warn!(%reason, "an avatar that is no image; treated as none");
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }

    fn avatar_cache(&self) -> std::sync::MutexGuard<'_, avatar::cache::AvatarCache> {
        self.avatar_cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    async fn upload_written_image(&self, image: &Avatar) -> Result<String, KupferdrahtError> {
        let uploaded = self
            .http
            .upload_public_image(
                self.api_version,
                &self.current_access_token(),
                &image.bytes,
                &image.mime_type,
            )
            .await?;
        Ok(uploaded.key)
    }

    /// Remember keys the backend accepted, in the row `resume` reads. A session
    /// without a store has nothing to remember.
    async fn remember_own_avatar(&self, keys: AvatarKeys) -> Result<(), KupferdrahtError> {
        let Some(store) = self.store_handle() else {
            return Ok(());
        };
        let mut row = self.current_self_row(store.as_ref()).await?;
        row.avatar_thumbnail_key = keys.thumbnail;
        row.avatar_full_key = keys.full;
        store.save_self_user(&row).await
    }
}

/// Decoding a large image takes a noticeable time, so natively it leaves the
/// runtime's threads. A panic in a decoder then costs this one image, not the
/// worker. The browser has one thread and no such choice.
#[cfg(not(target_arch = "wasm32"))]
async fn off_the_runtime<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, KupferdrahtError> + Send + 'static,
) -> Result<T, KupferdrahtError> {
    tokio::task::spawn_blocking(work).await.unwrap_or_else(|_| {
        Err(KupferdrahtError::InvalidInput(
            "avatar image: a decoder gave up on it".into(),
        ))
    })
}

#[cfg(target_arch = "wasm32")]
fn off_the_runtime<T>(
    work: impl FnOnce() -> Result<T, KupferdrahtError>,
) -> std::future::Ready<Result<T, KupferdrahtError>> {
    std::future::ready(work())
}

async fn rewrite_off_the_runtime(
    input: Vec<u8>,
    size: AvatarSize,
) -> Result<Avatar, KupferdrahtError> {
    off_the_runtime(move || avatar::rewrite_image(&input, size)).await
}

async fn write_both_sizes(image: Vec<u8>) -> Result<(Avatar, Avatar), KupferdrahtError> {
    off_the_runtime(move || {
        let pixels = avatar::decode_image(&image)?;
        Ok((
            pixels.write_as(AvatarSize::Thumbnail)?,
            pixels.write_as(AvatarSize::Full)?,
        ))
    })
    .await
}
