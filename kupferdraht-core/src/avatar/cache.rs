// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! The per-client image cache (Step 144): written images by asset key and size,
//! bounded in total, least recently used out first. A changed avatar has a new
//! key, so nothing is invalidated, only evicted.

use super::{Avatar, AvatarSize};

/// The cache's bound over all its images.
pub(crate) const AVATAR_CACHE_MAX_BYTES: usize = 4 * 1024 * 1024;
/// An image over this is served but not held.
pub(crate) const AVATAR_CACHE_MAX_ENTRY_BYTES: usize = 1024 * 1024;

/// The images, least recently used first. Few enough for a linear search: the
/// bound holds at most a few hundred thumbnails.
#[derive(Debug, Default)]
pub(crate) struct AvatarCache {
    entries: Vec<(String, AvatarSize, Avatar)>,
    total_bytes: usize,
}

impl AvatarCache {
    fn position(&self, key: &str, size: AvatarSize) -> Option<usize> {
        self.entries
            .iter()
            .position(|(k, s, _)| k == key && *s == size)
    }

    /// The image, now the most recently used.
    pub(crate) fn get(&mut self, key: &str, size: AvatarSize) -> Option<Avatar> {
        let at = self.position(key, size)?;
        let entry = self.entries.remove(at);
        let avatar = entry.2.clone();
        self.entries.push(entry);
        Some(avatar)
    }

    /// Hold the image, unless it is over the entry cap, and evict down to the
    /// total bound.
    pub(crate) fn put(&mut self, key: &str, size: AvatarSize, avatar: &Avatar) {
        if avatar.bytes.len() > AVATAR_CACHE_MAX_ENTRY_BYTES {
            return;
        }
        if let Some(at) = self.position(key, size) {
            let (_, _, old) = self.entries.remove(at);
            self.total_bytes -= old.bytes.len();
        }
        self.total_bytes += avatar.bytes.len();
        self.entries.push((key.to_owned(), size, avatar.clone()));
        while self.total_bytes > AVATAR_CACHE_MAX_BYTES {
            let (_, _, oldest) = self.entries.remove(0);
            self.total_bytes -= oldest.bytes.len();
        }
    }

    #[cfg(test)]
    pub(crate) fn total_bytes(&self) -> usize {
        self.total_bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(bytes: usize) -> Avatar {
        Avatar {
            bytes: vec![0; bytes],
            mime_type: "image/jpeg".to_owned(),
        }
    }

    #[test]
    fn an_image_is_kept_per_key_and_size() {
        let mut cache = AvatarCache::default();
        cache.put("key", AvatarSize::Thumbnail, &image(10));

        assert_eq!(cache.get("key", AvatarSize::Thumbnail), Some(image(10)));
        assert_eq!(cache.get("key", AvatarSize::Full), None);
        assert_eq!(cache.get("other", AvatarSize::Thumbnail), None);
    }

    #[test]
    fn an_image_over_the_entry_cap_is_not_kept() {
        let mut cache = AvatarCache::default();
        cache.put(
            "key",
            AvatarSize::Full,
            &image(AVATAR_CACHE_MAX_ENTRY_BYTES + 1),
        );

        assert_eq!(cache.get("key", AvatarSize::Full), None);
        assert_eq!(cache.total_bytes(), 0);
    }

    #[test]
    fn the_total_stays_bounded_and_the_least_recently_used_goes_first() {
        let mut cache = AvatarCache::default();
        let entry = AVATAR_CACHE_MAX_ENTRY_BYTES;
        for key in ["a", "b", "c", "d"] {
            cache.put(key, AvatarSize::Full, &image(entry));
        }
        assert!(
            cache.get("a", AvatarSize::Full).is_some(),
            "a is used, b is now the oldest"
        );

        cache.put("e", AvatarSize::Full, &image(entry));

        assert!(cache.total_bytes() <= AVATAR_CACHE_MAX_BYTES);
        assert_eq!(cache.get("b", AvatarSize::Full), None, "b went");
        for key in ["a", "c", "d", "e"] {
            assert!(cache.get(key, AvatarSize::Full).is_some(), "{key} stayed");
        }
    }

    #[test]
    fn putting_a_key_again_does_not_count_it_twice() {
        let mut cache = AvatarCache::default();
        cache.put("key", AvatarSize::Full, &image(100));
        cache.put("key", AvatarSize::Full, &image(100));

        assert_eq!(cache.total_bytes(), 100);
    }
}
