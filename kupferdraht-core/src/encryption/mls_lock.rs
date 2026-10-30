// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 95c — lock the shared [`MlsClient`], recovering from a poisoned
//! mutex (plan D4). What makes that safe: `KupferdrahtProvider`'s storage is
//! `openmls_sqlite_storage::SqliteStorageProvider` over a
//! `rusqlite::Connection` it **owns directly, with no mutex of its own**
//! — so this `Mutex<MlsClient>` is the only lock a panic under it can
//! poison. On top, openmls reloads every group from storage per
//! operation (`MlsGroup::load`) and `MlsClient` holds only the signature
//! keys and the provider — no in-memory half-state. A poisoned lock
//! therefore stays usable instead of failing every later MLS operation
//! of the session. If the provider's connection is ever wrapped in a
//! `Mutex` of its own (as `SqliteStore` does), this recovery is no
//! longer sufficient.

use std::sync::{Mutex, MutexGuard};

use crate::encryption::MlsClient;

/// Lock `mls`; a poisoned mutex is recovered (and its flag cleared)
/// with a warning instead of failing the caller.
pub(crate) fn lock_mls(mls: &Mutex<MlsClient>) -> MutexGuard<'_, MlsClient> {
    mls.lock().unwrap_or_else(|poisoned| {
        tracing::warn!("MLS mutex was poisoned by an earlier panic; recovering");
        mls.clear_poison();
        poisoned.into_inner()
    })
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::lock_mls;
    use crate::encryption::MlsClient;

    /// A panic while the MLS lock is held (a defect — foreign data no
    /// longer reaches one) used to poison the mutex for the rest of the
    /// session: every MLS operation failed until restart. The lock is
    /// recovered and the poison flag cleared.
    #[test]
    fn a_poisoned_mls_mutex_is_recovered() {
        let mls = Arc::new(Mutex::new(
            MlsClient::new_fresh(
                "a11ce000-0000-4000-8000-000000000001@wire.com",
                "a11ce00000000001",
            )
            .expect("MlsClient"),
        ));
        let poisoner = mls.clone();
        let _ = std::thread::spawn(move || {
            let _guard = poisoner.lock().expect("first lock");
            panic!("a defect while holding the MLS lock");
        })
        .join();
        assert!(mls.is_poisoned());

        let exists = lock_mls(&mls)
            .group_exists(b"no-such-group")
            .expect("MLS is usable after the recovery");

        assert!(!exists);
        assert!(!mls.is_poisoned(), "the poison flag is cleared");
    }
}
