// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Avatars (Step 144): the types, and the rewrite every image passes through.
//!
//! A crafted image attacks the decoder that renders it, and avatars are chosen
//! freely by every user in a shared room. So no byte of a downloaded or a given
//! image reaches the embedder: Rust decoders read it under limits, and what goes
//! out is a new image written from its pixels. The format crates are used
//! directly, `zune-jpeg` without its SIMD features (`docs/THREAT_MODEL.md`).
#![forbid(unsafe_code)]

pub(crate) mod cache;
mod decode;
mod encode;
mod exif;
mod pixels;

use crate::error::KupferdrahtError;
use crate::wire::dto::UserAsset;

pub(crate) use pixels::Pixels;

/// The largest image file accepted, downloaded or given.
pub const AVATAR_MAX_INPUT_BYTES: usize = 4 * 1024 * 1024;

/// The longer side of a thumbnail, as Wire's clients upload a `preview`.
const THUMBNAIL_PIXELS: u32 = 280;
/// The longer side of a full image, as Wire's webapp uploads a `complete`.
const FULL_PIXELS: u32 = 1448;

/// An image Kupferdraht wrote: no byte of what came in passes through.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct Avatar {
    pub bytes: Vec<u8>,
    /// `image/jpeg` or `image/png`, from what Kupferdraht wrote.
    pub mime_type: String,
}

/// Which of a user's two images: Wire's `preview` and `complete`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum AvatarSize {
    Thumbnail,
    Full,
}

impl AvatarSize {
    const fn target_pixels(self) -> u32 {
        match self {
            Self::Thumbnail => THUMBNAIL_PIXELS,
            Self::Full => FULL_PIXELS,
        }
    }
}

/// The two asset keys a profile names.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AvatarKeys {
    pub thumbnail: Option<String>,
    pub full: Option<String>,
}

impl AvatarKeys {
    /// The keys a profile's `assets` name. An asset of another type, of an
    /// unknown size or without a key is no avatar.
    pub(crate) fn from_assets(assets: &[UserAsset]) -> Self {
        let key_of = |size: &str| {
            assets
                .iter()
                .find(|a| {
                    a.size.as_deref() == Some(size)
                        && a.asset_type.as_deref() == Some("image")
                        && !a.key.is_empty()
                })
                .map(|a| a.key.clone())
        };
        Self {
            thumbnail: key_of(UserAsset::PREVIEW),
            full: key_of(UserAsset::COMPLETE),
        }
    }

    /// The key to fetch for `size`: its own, else the other size's, scaled to
    /// the size asked for. A profile may name only one.
    pub(crate) fn for_size(&self, size: AvatarSize) -> Option<&str> {
        let (wanted, other) = match size {
            AvatarSize::Thumbnail => (&self.thumbnail, &self.full),
            AvatarSize::Full => (&self.full, &self.thumbnail),
        };
        wanted.as_deref().or(other.as_deref())
    }

    /// The version marker: the thumbnail's key, the full image's when a
    /// profile has only that.
    #[must_use]
    pub fn version(&self) -> Option<&str> {
        self.thumbnail.as_deref().or(self.full.as_deref())
    }
}

/// Decode `input` under the limits and write a new image for `size`.
///
/// # Errors
/// `InvalidInput` for what is no accepted image or exceeds the limits;
/// `Encoding` should writing the new image fail.
pub fn rewrite_image(input: &[u8], size: AvatarSize) -> Result<Avatar, KupferdrahtError> {
    decode::decode(input)?.write_as(size)
}

/// Decode `input` under the limits, for writing it in more than one size.
///
/// # Errors
/// `InvalidInput` for what is no accepted image or exceeds the limits.
pub(crate) fn decode_image(input: &[u8]) -> Result<Pixels, KupferdrahtError> {
    decode::decode(input)
}
