// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `KupferdrahtHome` — the single source of the on-disk path layout and of the
//! home id (Step 93, ADR-0025).
//!
//! One base directory hosts the device-level registry and every
//! account's databases and asset cache. Centralizing the layout here
//! keeps storage and the asset cache in agreement and makes the layout
//! changeable in one place. Canonical layout:
//!
//! ```text
//! base/home.id                      (the home id — plaintext, an identifier)
//! base/global.db
//! base/kupferdraht.lock                  (native: the home-level process lock)
//! base/accounts/<id>/kupferdraht.db
//! base/accounts/<id>/mls.db
//! base/accounts/<id>/assets/
//! base/accounts/<id>/kupferdraht.lock    (native: the per-account process lock)
//! ```
//!
//! The home id is read once, when the home is resolved (`open`), and
//! held here: every keystore id of the home (`keystore::key_id`) and its
//! login cookie label derive from it.

use std::path::PathBuf;

use super::account_keys::AccountKeySource;
use crate::error::KupferdrahtError;

/// Owns the base directory and the home id; computes the canonical
/// sub-paths. Cheap to clone (a `PathBuf` and a short `String`).
#[derive(Clone, Debug)]
pub(crate) struct KupferdrahtHome {
    base: PathBuf,
    home_id: String,
    /// Where a new key of an account comes from (ADR-0035).
    account_keys: AccountKeySource,
}

impl KupferdrahtHome {
    /// Resolve the home at `base`: create the directory (native) and read
    /// its id, creating the id on the first open of a fresh home — under
    /// the home lock (`storage::home_id`).
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` (directory, unreadable or malformed
    /// `home.id`); `KupferdrahtError::AccountLocked` while another process
    /// creates the id (retry).
    pub(crate) fn open(base: PathBuf) -> Result<Self, KupferdrahtError> {
        let home_id = super::home_id::resolve_home_id(&base)?;
        Ok(Self::with_home_id(base, home_id))
    }

    /// A home whose id is already known — the layout without I/O (`open`
    /// after resolving, and the layout tests).
    pub(crate) fn with_home_id(base: PathBuf, home_id: String) -> Self {
        debug_assert!(
            super::home_id::is_home_id(&home_id),
            "home ids are 32 lowercase hex"
        );
        Self {
            base,
            home_id,
            account_keys: AccountKeySource::default(),
        }
    }

    /// This home with `source` as the origin of new account keys
    /// (ADR-0035: `Seed` in the browser).
    #[must_use]
    pub(crate) fn with_account_keys(mut self, source: AccountKeySource) -> Self {
        self.account_keys = source;
        self
    }

    /// Where a new key of an account comes from in this home.
    pub(crate) fn account_keys(&self) -> AccountKeySource {
        self.account_keys
    }

    /// The base directory itself (on wasm: the OPFS pool's name prefix).
    pub(crate) fn base_dir(&self) -> &std::path::Path {
        &self.base
    }

    /// The home id: 32 lowercase hex, persisted in `base/home.id`.
    pub(crate) fn home_id(&self) -> &str {
        &self.home_id
    }

    /// The Wire login cookie label of this home, `kupferdraht-<home-id>`.
    pub(crate) fn cookie_label(&self) -> String {
        super::home_id::cookie_label(&self.home_id)
    }

    // On wasm these are the OPFS pool's file names (Step 91, ADR-0022): the
    // same layout, no directory on disk.
    /// `base/global.db` — the device-level account registry.
    pub(crate) fn global_db(&self) -> PathBuf {
        self.base.join("global.db")
    }

    /// `base/accounts/<id>` — the per-account directory.
    pub(crate) fn account_dir(&self, account_id: &str) -> PathBuf {
        self.base.join("accounts").join(account_id)
    }

    /// `base/accounts/<id>/kupferdraht.db` — the per-account app-data store.
    pub(crate) fn account_db(&self, account_id: &str) -> PathBuf {
        self.account_dir(account_id).join("kupferdraht.db")
    }

    /// `base/accounts/<id>/mls.db` — the per-account openmls store.
    pub(crate) fn account_mls(&self, account_id: &str) -> PathBuf {
        self.account_dir(account_id).join("mls.db")
    }

    /// `base/accounts/<id>/assets` — the per-account asset cache.
    pub(crate) fn account_assets(&self, account_id: &str) -> PathBuf {
        self.account_dir(account_id).join("assets")
    }

    /// `base/kupferdraht.lock` — the home-level advisory lock (Step 85): guards
    /// the first-time creation of the device-registry key and (Step 93)
    /// of the home id. Native only: the browser has one instance per
    /// origin (ADR-0023).
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn home_lock(&self) -> PathBuf {
        self.base.join(super::lock::PROCESS_LOCK_FILE)
    }

    /// `base/accounts/<id>/kupferdraht.lock` — the per-account advisory lock
    /// (Step 85): guards the token refresh + persist and the first-time
    /// creation of the account's database keys. Native only.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn account_lock(&self, account_id: &str) -> PathBuf {
        self.account_dir(account_id)
            .join(super::lock::PROCESS_LOCK_FILE)
    }

    /// `base/accounts/<id>/sync.lock` — the account's drain lock (Step
    /// 97d, ADR-0031), separate from `kupferdraht.lock`. Native only.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn account_sync_lock(&self, account_id: &str) -> PathBuf {
        self.account_dir(account_id)
            .join(super::lock::SYNC_LOCK_FILE)
    }
}

/// Whether `id` is safe to use as a SINGLE path component (Step 80).
/// Envelope ids are peer-controlled runtime data that reach the
/// filesystem (asset cache); an id containing separators, `..`/`.`,
/// NUL/control characters, or nothing at all must never be joined
/// into a path. Length-capped to stay under filesystem component
/// limits with room for a suffix. Windows oddities (ADS via `:`,
/// device names like `CON`/`NUL`) are deliberately not excluded —
/// they cannot escape the parent directory, which is this guard's
/// contract.
pub(crate) fn is_path_safe_id(id: &str) -> bool {
    // 255 is the common filesystem component limit; the callers
    // append a short suffix, so stay well under it.
    if id.is_empty() || id.len() > 250 || id == "." || id == ".." {
        return false;
    }
    !id.chars().any(|c| c == '/' || c == '\\' || c.is_control())
}

#[cfg(test)]
mod path_safe_tests {
    use super::is_path_safe_id;

    /// Step 80: everything a hostile peer could use to escape the
    /// cache directory (or to confuse a directory walk) is refused.
    #[test]
    fn hostile_ids_are_rejected() {
        for bad in [
            "",
            ".",
            "..",
            "../evil",
            "..\\evil",
            "a/b",
            "a\\b",
            "/absolute",
            "id\u{0}null",
            "id\nnewline",
            "id\u{7f}del",
        ] {
            assert!(!is_path_safe_id(bad), "must reject {bad:?}");
        }
        let overlong = "a".repeat(300);
        assert!(!is_path_safe_id(&overlong), "must reject overlong ids");
    }

    /// Legitimate ids (Wire clients send UUIDs; be liberal beyond
    /// that as long as it is path-safe).
    #[test]
    fn legitimate_ids_pass() {
        for good in [
            "5c22fa31-77aa-4bde-9d10-223344556677",
            "abcDEF123",
            "id_with-dash.and.dots",
        ] {
            assert!(is_path_safe_id(good), "must accept {good:?}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "/tmp/kupferdraht-home";
    const ID: &str = "user-uuid@example.test";
    const HOME_ID: &str = "0123456789abcdef0123456789abcdef";

    fn home() -> KupferdrahtHome {
        KupferdrahtHome::with_home_id(PathBuf::from(BASE), HOME_ID.to_owned())
    }

    #[test]
    fn global_db_is_base_global_db() {
        assert_eq!(home().global_db(), PathBuf::from(BASE).join("global.db"));
    }

    /// Step 93: the home carries its id, and the label is the id behind
    /// the `kupferdraht-` prefix.
    #[test]
    fn the_home_carries_its_id_and_the_label_derives_from_it() {
        assert_eq!(home().home_id(), HOME_ID);
        assert_eq!(home().cookie_label(), format!("kupferdraht-{HOME_ID}"));
    }

    #[test]
    fn account_dir_is_base_accounts_id() {
        assert_eq!(
            home().account_dir(ID),
            PathBuf::from(BASE).join("accounts").join(ID)
        );
    }

    #[test]
    fn account_db_is_kupferdraht_db_under_account_dir() {
        assert_eq!(
            home().account_db(ID),
            home().account_dir(ID).join("kupferdraht.db")
        );
    }

    #[test]
    fn account_mls_is_mls_db_under_account_dir() {
        assert_eq!(
            home().account_mls(ID),
            home().account_dir(ID).join("mls.db")
        );
    }

    /// Step 85: the lock files live next to what they guard — one per
    /// home, one per account directory; never outside the home.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn lock_files_live_in_the_home_and_in_the_account_dir() {
        assert_eq!(
            home().home_lock(),
            PathBuf::from(BASE).join("kupferdraht.lock")
        );
        assert_eq!(
            home().account_lock(ID),
            home().account_dir(ID).join("kupferdraht.lock")
        );
    }

    #[test]
    fn account_assets_is_single_nested_under_account_dir() {
        // The double-nest fix: assets live directly under accounts/<id>,
        // NOT accounts/<id>/<id>/assets.
        assert_eq!(
            home().account_assets(ID),
            home().account_dir(ID).join("assets")
        );
        assert_eq!(
            home().account_assets(ID),
            PathBuf::from(BASE).join("accounts").join(ID).join("assets")
        );
    }
}
