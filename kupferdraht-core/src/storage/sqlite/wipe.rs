// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! One-shot clear of per-account messaging state — used when a re-login
//! registers a NEW device that supersedes a gone one (Step 40 Part B). The new
//! device starts from a clean slate and re-joins every room fresh, so the old
//! device's rooms, history, and sync cursor are dropped. Session / device /
//! self-user / server-config / feature-config rows are left intact (login
//! re-establishes them); MLS group state lives in the separate `mls.db` and is
//! discarded by the caller via `discard_group`.

use crate::error::KupferdrahtError;

use super::SqliteStore;

/// Delete every row from the messaging tables in a single transaction.
/// Children before parents so foreign keys never block the delete.
pub(super) fn clear_messaging_state(store: &SqliteStore) -> Result<(), KupferdrahtError> {
    let mut conn = store.lock()?;
    let tx = super::write_tx::begin(&mut conn)
        .map_err(|e| KupferdrahtError::Storage(format!("clear_messaging_state begin tx: {e}")))?;
    for table in [
        "reactions",
        "mentions",
        "attachments",
        "read_receipts",
        "delivery_receipts",
        "timeline_events",
        "room_members",
        "rooms",
        "sync_state",
        "room_devices",
    ] {
        tx.execute(&format!("DELETE FROM {table}"), [])
            .map_err(|e| {
                KupferdrahtError::Storage(format!("clear_messaging_state {table}: {e}"))
            })?;
    }
    tx.commit()
        .map_err(|e| KupferdrahtError::Storage(format!("clear_messaging_state commit: {e}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::fresh_store;
    use crate::storage::Store as _;
    use crate::storage::records::{RoomRecord, SyncStateRecord};

    #[tokio::test]
    async fn clear_messaging_state_empties_rooms_and_cursor() {
        let (_dir, store) = fresh_store();
        let room = RoomRecord {
            room_id: "room-a@example.test".to_string(),
            group_id: Some("Z3JwLWFh".to_string()),
            name: "Engineering".to_string(),
            room_type: "group".to_string(),
            epoch: 1,
            counterpart_user_id: None,
            last_synced_at_ms: 1_700_000_000_000,
        };
        store
            .replace_rooms(&[(room, vec![])])
            .await
            .expect("seed rooms");
        store
            .save_sync_state(&SyncStateRecord {
                sync_marker_uuid: None,
                last_event_id: Some("ev-1".to_string()),
                last_connected_at_ms: None,
                last_key_package_check_at_ms: None,
            })
            .await
            .expect("seed sync state");
        assert_eq!(store.load_rooms().await.expect("load").len(), 1);

        store.clear_messaging_state().await.expect("clear");

        assert!(
            store.load_rooms().await.expect("load").is_empty(),
            "rooms must be cleared"
        );
        assert!(
            store.load_sync_state().await.expect("load").is_none(),
            "sync cursor must be cleared"
        );
    }
}
