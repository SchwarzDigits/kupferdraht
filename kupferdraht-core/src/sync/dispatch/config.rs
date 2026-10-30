// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Inbound server-administered-config event arms — Step 10d.

use std::sync::Arc;

use crate::client::Client;
use crate::error::KupferdrahtError;
use crate::storage::UserPropertyEntry;
use crate::sync::follow_ups::FollowUp;
use crate::sync::service::SyncService;
use crate::time::now_ms;

impl SyncService {
    /// Step 10d: `feature-config.update` event arm. Wire ships the
    /// event without the new value (and v14 has no per-feature read
    /// endpoint), so the dispatcher re-fetches `/v14/feature-configs`
    /// and replaces the persisted whole-team snapshot. ADR-0012
    /// §Notification dispatch. The re-fetch is network, so it runs as a
    /// follow-up after the drain lock (Step 97d, ADR-0031).
    pub(crate) fn dispatch_feature_config_update(&self) {
        self.queue_follow_up(FollowUp::RefetchFeatureConfigs);
    }

    /// Step 10d: `user.properties-set` event arm. The event carries
    /// the affected `key` only — the dispatcher re-fetches
    /// `/v14/properties/{key}` and upserts the row — as a follow-up
    /// after the drain lock (Step 97d, ADR-0031).
    pub(crate) fn dispatch_user_property_set(
        &self,
        ev: &serde_json::Value,
    ) -> Result<(), KupferdrahtError> {
        let key = ev
            .get("key")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                KupferdrahtError::Encoding("user.properties-set event missing `key` field".into())
            })?;
        self.queue_follow_up(FollowUp::RefetchProperty(key.to_string()));
        Ok(())
    }

    /// Step 10d: `user.properties-delete` event arm. ADR-0012
    /// §Out of scope — the dispatcher MUST NOT call Wire's
    /// `DELETE /v14/properties/{key}`; Wire is the source of truth
    /// for the deletion in the first place, and re-issuing the call
    /// would produce another `user.properties-delete` event.
    pub(crate) async fn dispatch_user_property_delete(
        &self,
        client: &Arc<Client>,
        ev: &serde_json::Value,
    ) -> Result<(), KupferdrahtError> {
        let key = ev
            .get("key")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                KupferdrahtError::Encoding(
                    "user.properties-delete event missing `key` field".into(),
                )
            })?;
        if let Some(store) = client.store_handle() {
            store.delete_user_property(key).await?;
        }
        Ok(())
    }
}

/// Re-fetch `/v14/feature-configs` and replace the persisted snapshot —
/// the follow-up of a `feature-config.update` event (Step 10d).
pub(in crate::sync) async fn refetch_feature_configs(
    client: &Arc<Client>,
) -> Result<(), KupferdrahtError> {
    let raw = client.http_fetch_feature_configs().await?;
    if let Some(store) = client.store_handle() {
        store
            .replace_feature_configs(&raw.into_entries(now_ms()))
            .await?;
    }
    Ok(())
}

/// Re-fetch `/v14/properties/{key}` and upsert the row — the follow-up
/// of a `user.properties-set` event (Step 10d).
pub(in crate::sync) async fn refetch_user_property(
    client: &Arc<Client>,
    key: &str,
) -> Result<(), KupferdrahtError> {
    // The key is server data: `http_fetch_property_value` refuses a
    // hostile one before any request (Step 95c) — never asserted here.
    let value = client.http_fetch_property_value(key).await?;
    if let Some(store) = client.store_handle() {
        store
            .upsert_user_property(&UserPropertyEntry {
                key: key.to_string(),
                value,
                last_synced_at_ms: now_ms(),
            })
            .await?;
    }
    Ok(())
}
