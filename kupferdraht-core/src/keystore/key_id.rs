// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Keystore secret ids — the single source for the strings under which
//! Kupferdraht stores database keys in the platform keystore.
//!
//! Every id carries the **home id** (Step 93, ADR-0025):
//! `kupferdraht.<home-id>.global-db`, `kupferdraht.<home-id>.account-<user>`,
//! `kupferdraht.<home-id>.account-<user>-mls`. The keystore is device-wide (one
//! keychain service, one dev keystore directory) while a home is the unit
//! of a Kupferdraht installation; two homes on one keystore therefore never
//! touch the same entry, and a purge of one home (Step 78) never reaches
//! another home's keys. The `kupferdraht.` prefix keeps the entries
//! recognisable beside other software's and beside the legacy ones.
//!
//! Each [`KeyId`] also names its pre-Step-93 keystore-wide form — the
//! *legacy* id — so `storage::load_or_create_key_locked` can adopt a key an
//! earlier build stored (read there, never written or deleted).

use crate::storage::account_keys::AccountKeyPurpose;

/// A database key's keystore id: the home-scoped form Kupferdraht reads and
/// writes, plus the keystore-wide form it may be adopted from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct KeyId {
    namespaced: String,
    legacy: String,
    owner: KeyOwner,
}

/// Whose key an id names — which decides where a new key comes from
/// (ADR-0035: a device key is random, an account key may come from the
/// account's seed).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum KeyOwner {
    /// The device's own database (`global.db`).
    Device,
    /// A database of an account.
    Account {
        account_id: String,
        purpose: AccountKeyPurpose,
    },
}

impl KeyId {
    fn new(home_id: &str, legacy: String, owner: KeyOwner) -> Self {
        debug_assert!(!home_id.is_empty(), "a key id needs its home id");
        Self {
            namespaced: format!("kupferdraht.{home_id}.{legacy}"),
            legacy,
            owner,
        }
    }

    /// Whose key this is.
    pub(crate) fn owner(&self) -> &KeyOwner {
        &self.owner
    }

    /// `kupferdraht.<home-id>.<suffix>` — the id in effect.
    pub(crate) fn namespaced(&self) -> &str {
        &self.namespaced
    }

    /// The pre-Step-93 keystore-wide id (`global-db`, `account-<user>`,
    /// `account-<user>-mls`) — adoption source only.
    pub(crate) fn legacy(&self) -> &str {
        &self.legacy
    }
}

/// The id of the device-level `global.db` key (the account registry) in
/// the home `home_id`. Openable at app start without opening any account.
pub(crate) fn global_db(home_id: &str) -> KeyId {
    KeyId::new(home_id, "global-db".to_owned(), KeyOwner::Device)
}

/// The id of an account's `kupferdraht.db` (app-data) key in the home `home_id`.
pub(crate) fn account_db(home_id: &str, account_id: &str) -> KeyId {
    KeyId::new(
        home_id,
        format!("account-{account_id}"),
        KeyOwner::Account {
            account_id: account_id.to_owned(),
            purpose: AccountKeyPurpose::Store,
        },
    )
}

/// The id of an account's `mls.db` (openmls state) key in the home
/// `home_id` — a **separate** key from `kupferdraht.db`, so the two databases
/// share no key material (ADR-0021).
pub(crate) fn account_mls(home_id: &str, account_id: &str) -> KeyId {
    KeyId::new(
        home_id,
        format!("account-{account_id}-mls"),
        KeyOwner::Account {
            account_id: account_id.to_owned(),
            purpose: AccountKeyPurpose::Mls,
        },
    )
}

/// The id of an account's seed in the home `home_id` (ADR-0035): the
/// secret a `Seed` home derives the account's keys from. Stored beside the
/// account's keys; never a database key itself.
pub(crate) fn account_seed(home_id: &str, account_id: &str) -> String {
    debug_assert!(!home_id.is_empty(), "a seed id needs its home id");
    format!("kupferdraht.{home_id}.account-{account_id}-seed")
}

/// The id of an account's storage-login key in the home `home_id`
/// (Step 111b, ADR-0035): the Ed25519 key the storage server's login is
/// signed with, beside the account's database keys.
#[cfg(any(target_arch = "wasm32", test))]
pub(crate) fn account_storage_login(home_id: &str, account_id: &str) -> String {
    debug_assert!(!home_id.is_empty(), "a key id needs its home id");
    format!("kupferdraht.{home_id}.account-{account_id}-storage-login")
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOME: &str = "0123456789abcdef0123456789abcdef";
    const USER: &str = "user-uuid@example.test";

    #[test]
    fn ids_carry_the_home_id_and_name_their_legacy_form() {
        let global = global_db(HOME);
        assert_eq!(global.namespaced(), format!("kupferdraht.{HOME}.global-db"));
        assert_eq!(global.legacy(), "global-db");
        let db = account_db(HOME, USER);
        assert_eq!(
            db.namespaced(),
            format!("kupferdraht.{HOME}.account-{USER}")
        );
        assert_eq!(db.legacy(), format!("account-{USER}"));
        let mls = account_mls(HOME, USER);
        assert_eq!(
            mls.namespaced(),
            format!("kupferdraht.{HOME}.account-{USER}-mls")
        );
        assert_eq!(mls.legacy(), format!("account-{USER}-mls"));
    }

    /// Step 110a: the seed sits beside the account's keys, home-scoped.
    #[test]
    fn the_seed_id_is_home_scoped() {
        assert_eq!(
            account_seed(HOME, USER),
            format!("kupferdraht.{HOME}.account-{USER}-seed")
        );
        assert_ne!(
            account_seed(HOME, USER),
            account_seed("fedcba9876543210fedcba9876543210", USER)
        );
    }

    #[test]
    fn two_homes_never_share_an_id() {
        let other = "fedcba9876543210fedcba9876543210";
        assert_ne!(account_db(HOME, USER), account_db(other, USER));
        assert_ne!(global_db(HOME), global_db(other));
    }

    /// The `FileKeyStore` writes `<id>.key` (+ `.tmp`): the longest id
    /// with a maximal DNS domain must stay under the 255-byte file-name
    /// limit that ADR-0025 documents (domains up to 153 bytes since the
    /// `kupferdraht.` prefix, ADR-0041).
    #[test]
    fn the_longest_id_stays_under_the_file_name_limit_for_documented_domains() {
        let domain = "d".repeat(153);
        let user = format!("12345678-1234-1234-1234-123456789012@{domain}");
        let id = account_mls(HOME, &user);
        assert!(id.namespaced().len() + ".key.tmp".len() <= 255);
    }
}
