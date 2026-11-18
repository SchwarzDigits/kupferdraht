// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 84 — the attachment cache keeps only what a message names.
//!
//! A delete for everyone removes its message's cached ciphertext after the
//! store took the file's row. A client's start removes what is left over: the
//! staged copies of sends — at a start none can be running — and every cached
//! file no attachment row names (a crash between a delete's commit and its
//! removal, a removal that failed, a history cleared at login), and the cache
//! directory of a release before Step 163b, which nothing else reaches any
//! more. A failure is logged, never raised: the next start tries again.

use std::collections::HashSet;
use std::path::Path;

use tracing::warn;

use super::attachment_receive::attachment_cache_file_name;
use super::attachment_send::{STAGING_DIR, remove_ciphertext_file};
use crate::client::Client;

impl Client {
    /// Remove the cached ciphertext of `message_id`, whose row a delete for
    /// everyone took. Absent is fine; a failure is logged.
    pub(crate) fn remove_cached_file(&self, message_id: &str) {
        debug_assert!(!message_id.is_empty(), "a deleted message has an id");
        let Some(dir) = self.attachments_dir() else {
            return;
        };
        match attachment_cache_file_name(message_id) {
            Ok(name) => remove_ciphertext_file(&dir.join(name)),
            Err(e) => warn!(message_id, error = %e, "no cache file name for a deleted message"),
        }
    }

    /// Remove the cache directory a release before Step 163b kept. A row that
    /// still names a file there downloads it again when asked.
    fn remove_former_cache_dir(&self) {
        let (Some(former), Some(dir)) = (self.former_attachments_dir(), self.attachments_dir())
        else {
            return;
        };
        debug_assert!(
            former.parent() == dir.parent() && former != dir,
            "the former cache sits beside today's"
        );
        match std::fs::remove_dir_all(&former) {
            Ok(()) => tracing::info!("the former attachment cache directory removed"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => warn!(error = %e, "the former attachment cache directory kept"),
        }
    }

    /// Remove what the account's attachment cache holds that no message names:
    /// every staged copy, every cached file no attachment row names and the
    /// former cache directory.
    pub(crate) async fn remove_leftover_files(&self) {
        self.remove_former_cache_dir();
        let (Some(dir), Some(store)) = (self.attachments_dir(), self.store_handle()) else {
            return;
        };
        debug_assert!(
            dir.ends_with("attachments"),
            "the sweep runs on the attachment cache"
        );
        remove_ciphertext_files(&dir.join(STAGING_DIR), |_| true);
        match store.attachment_message_ids().await {
            Ok(ids) => {
                let named: HashSet<String> = ids.into_iter().collect();
                remove_ciphertext_files(&dir, |message_id| !named.contains(message_id));
            }
            Err(e) => warn!(error = %e, "attachment cache kept: attachment rows unreadable"),
        }
    }
}

/// Remove every `<stem>.enc` file directly in `dir` that `unnamed` picks by
/// its stem. A missing `dir` holds nothing; subdirectories are not entered.
fn remove_ciphertext_files(dir: &Path, unnamed: impl Fn(&str) -> bool) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(e) => {
            warn!(dir = %dir.display(), error = %e, "attachment cache not readable");
            return;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        debug_assert!(path.starts_with(dir), "only the cache's own files go");
        let is_file = entry.file_type().is_ok_and(|kind| kind.is_file());
        let stem = path
            .extension()
            .filter(|ext| *ext == "enc")
            .and(path.file_stem())
            .and_then(|stem| stem.to_str());
        if is_file && stem.is_some_and(&unnamed) {
            remove_ciphertext_file(&path);
        }
    }
}
