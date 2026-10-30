// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Inbound `Confirmation` (READ / DELIVERED) dispatch — Step 6-II-f + 25b.

use std::sync::Arc;

use tracing::{debug, info};

use crate::client::Client;
use crate::error::KupferdrahtError;
use crate::sync::service::SyncService;
use crate::time::now_ms;

impl SyncService {
    /// Apply an inbound `Confirmation` of type `READ` (Step 6-II-f) or
    /// `DELIVERED` (Step 25b) to its sibling table (`read_receipts` /
    /// `delivery_receipts`). Wire batches a confirmation: one event carries
    /// one `first_message_id` plus N `more_message_ids` — all from the same
    /// confirmer. READ and DELIVERED are independent lists; the two share the
    /// id-resolution + `UpdateAt` emit, differing only in which table is
    /// written and the field it overlays.
    pub(crate) async fn dispatch_inbound_confirmation(
        &self,
        client: &Arc<Client>,
        room_id: &str,
        confirmer_user_id: &str,
        confirmation_event_id: &str,
        conf: crate::wire::proto::Confirmation,
    ) -> Result<(), KupferdrahtError> {
        const READ: i32 = crate::wire::proto::confirmation::Type::Read as i32;
        const DELIVERED: i32 = crate::wire::proto::confirmation::Type::Delivered as i32;
        if conf.r#type != READ && conf.r#type != DELIVERED {
            debug!(
                confirmation_event_id,
                ty = conf.r#type,
                "Confirmation type not READ/DELIVERED — skip dispatch"
            );
            return Ok(());
        }
        let is_read = conf.r#type == READ;
        let message_ids = collect_confirmation_ids(conf);
        info!(
            confirmation_event_id,
            confirmer = confirmer_user_id,
            count = message_ids.len(),
            kind = if is_read { "READ" } else { "DELIVERED" },
            "Confirmation received"
        );
        apply_confirmation_receipts(client, room_id, confirmer_user_id, &message_ids, is_read).await
    }
}

/// Resolve canonical ids, write the confirmation rows to the matching table
/// (`read_receipts` for READ, `delivery_receipts` for DELIVERED), and emit an
/// `UpdateAt` per message so the overlaid `read_by` / `delivered_to` refreshes.
/// Shared tail of [`SyncService::dispatch_inbound_confirmation`]. Step 25b.
async fn apply_confirmation_receipts(
    client: &Arc<Client>,
    room_id: &str,
    confirmer_user_id: &str,
    message_ids: &[String],
    is_read: bool,
) -> Result<(), KupferdrahtError> {
    debug_assert!(
        !room_id.is_empty(),
        "apply_confirmation_receipts: room_id must be non-empty"
    );
    let Some(store) = client.store_handle() else {
        return Ok(());
    };
    let canonical_ids = client.resolve_canonical_ids(room_id, message_ids).await?;
    let confirmed_at_ms = now_ms();
    let inserted = if is_read {
        store
            .apply_read_receipts(room_id, confirmer_user_id, &canonical_ids, confirmed_at_ms)
            .await?
    } else {
        store
            .apply_delivery_receipts(room_id, confirmer_user_id, &canonical_ids, confirmed_at_ms)
            .await?
    };
    debug!(inserted, "applied confirmation receipts");
    for canonical in &canonical_ids {
        client.emit_message_update_at(room_id, canonical).await?;
    }
    Ok(())
}

/// Flatten Wire's `Confirmation { first_message_id, more_message_ids }`
/// shape into a single ordered `Vec<String>`. Wire batches reads into
/// one Confirmation event per reader.
fn collect_confirmation_ids(conf: crate::wire::proto::Confirmation) -> Vec<String> {
    let mut out = Vec::with_capacity(1 + conf.more_message_ids.len());
    out.push(conf.first_message_id);
    out.extend(conf.more_message_ids);
    out
}
