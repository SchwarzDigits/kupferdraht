// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `recovery_state` table (Step 112, ADR-0035): the singleton row (PK = 1)
//! that says since when the account's recovery data exists and holds the
//! local PIN verifier. No encrypted columns — the whole database is.

use rusqlite::{OptionalExtension, params};

use crate::error::KupferdrahtError;
use crate::storage::records::RecoveryStateRecord;

use super::SqliteStore;

pub(super) fn save_recovery_state(
    store: &SqliteStore,
    state: &RecoveryStateRecord,
) -> Result<(), KupferdrahtError> {
    let conn = store.lock()?;
    conn.execute(
        "INSERT INTO recovery_state (id, exported_at_ms, pin_verifier) VALUES (1, ?1, ?2)
         ON CONFLICT (id) DO UPDATE SET
            exported_at_ms = excluded.exported_at_ms,
            pin_verifier   = excluded.pin_verifier",
        params![state.exported_at_ms, state.pin_verifier.as_slice()],
    )
    .map_err(|e| KupferdrahtError::Storage(format!("save_recovery_state: {e}")))?;
    Ok(())
}

pub(super) fn load_recovery_state(
    store: &SqliteStore,
) -> Result<Option<RecoveryStateRecord>, KupferdrahtError> {
    let conn = store.lock()?;
    let row: Option<(i64, Vec<u8>)> = conn
        .query_row(
            "SELECT exported_at_ms, pin_verifier FROM recovery_state WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|e| KupferdrahtError::Storage(format!("load_recovery_state: {e}")))?;
    let Some((exported_at_ms, verifier)) = row else {
        return Ok(None);
    };
    let pin_verifier = verifier.try_into().map_err(|_| {
        KupferdrahtError::Storage("load_recovery_state: a PIN verifier of another length".into())
    })?;
    Ok(Some(RecoveryStateRecord {
        exported_at_ms,
        pin_verifier,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::sqlite::{fresh_store, test_db_key};

    #[test]
    fn a_new_store_has_no_recovery_state() {
        let (_dir, store) = fresh_store();
        assert_eq!(load_recovery_state(&store).expect("loaded"), None);
    }

    #[test]
    fn the_recovery_state_round_trips_and_is_replaced() {
        let (_dir, store) = fresh_store();
        let first = RecoveryStateRecord {
            exported_at_ms: 1_000,
            pin_verifier: [1; 32],
        };
        let second = RecoveryStateRecord {
            exported_at_ms: 2_000,
            pin_verifier: [2; 32],
        };

        save_recovery_state(&store, &first).expect("saved");
        assert_eq!(load_recovery_state(&store).expect("loaded"), Some(first));
        save_recovery_state(&store, &second).expect("replaced");
        assert_eq!(load_recovery_state(&store).expect("loaded"), Some(second));
    }

    /// The state is the database's: a reopened store still knows it.
    #[test]
    fn the_recovery_state_survives_reopening_the_store() {
        let (dir, store) = fresh_store();
        let state = RecoveryStateRecord {
            exported_at_ms: 3_000,
            pin_verifier: [3; 32],
        };
        save_recovery_state(&store, &state).expect("saved");
        drop(store);

        let reopened = SqliteStore::open(&dir.path().join("kupferdraht.db"), &test_db_key())
            .expect("reopened");

        assert_eq!(load_recovery_state(&reopened).expect("loaded"), Some(state));
    }
}
