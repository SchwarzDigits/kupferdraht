// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `Client` accessors for Wire's server-administered configuration.
//!
//! Two layers:
//!
//! - **Raw accessors (Step 10c)** — `feature_configs` /
//!   `feature_config(name)` / `user_properties` /
//!   `user_property(key)` return the persisted V18-table rows
//!   verbatim.
//! - **Typed getters (Step 10e)** — `file_sharing` /
//!   `receipt_mode` project the raw entries into
//!   [`FileSharingPolicy`] / [`ReceiptMode`] so reactive code
//!   doesn't re-parse JSON on every call. Both return the safest
//!   default (`Enabled`, `On`) when the underlying entry is
//!   missing per ADR-0012 §Decision: a permissive default that
//!   surfaces a clean Wire 403 in the action's error path is
//!   preferable to silently suppressing a feature the user is in
//!   fact entitled to.
//!
//! Both surfaces are seeded at login by
//! [`crate::client::login_pipeline::LoginPipeline::fetch_server_configs`]
//! (Step 10c) and refreshed live by Step 10d's notification dispatch.
//! These accessors read from the persisted V18 tables — no Wire
//! round-trip per call.

use tracing::warn;

use crate::client::Client;
use crate::error::KupferdrahtError;
use crate::storage::{FeatureConfigEntry, UserPropertyEntry};

/// Team-policy projection of `feature_configs.fileSharing`. Step 10e.
///
/// Wire's `fileSharing` feature carries a `status` and nothing else the
/// client acts on (the v14 Swagger's `FileSharingConfig` has `status`,
/// `lockStatus` and `ttl`), so the projection is a two-way switch.
/// Backs the file-attachment send-path's pre-flight check (Step 11):
/// `Disabled` rejects the action client-side without a Wire
/// round-trip, `Enabled` lets it through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum FileSharingPolicy {
    Disabled,
    Enabled,
}

impl FileSharingPolicy {
    /// Parse a `fileSharing` row into a `FileSharingPolicy`.
    ///
    /// - `status == "disabled"` → [`Self::Disabled`].
    /// - `status == "enabled"` (or null/missing — Wire treats absent
    ///   status as enabled for this feature) → [`Self::Enabled`]. The
    ///   row's `config` object is ignored: the feature defines none.
    /// - Any other status string is logged at warn and treated as
    ///   `Enabled` — the safest forward-compat default.
    pub(crate) fn from_entry(entry: &FeatureConfigEntry) -> Self {
        match entry.status.as_deref() {
            Some("disabled") => Self::Disabled,
            Some("enabled") | None => Self::Enabled,
            Some(other) => {
                warn!(
                    status = %other,
                    "unknown fileSharing status; defaulting to Enabled"
                );
                Self::Enabled
            }
        }
    }
}

/// User-property projection of `user_properties.WIRE_RECEIPT_MODE`.
/// Step 10e. Step 10f's `Timeline::mark_as_read` consults this to
/// suppress outbound `Confirmation { type: READ }` emission when
/// the user has read receipts disabled in their Wire settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ReceiptMode {
    Off,
    On,
}

impl ReceiptMode {
    /// Parse a Wire `WIRE_RECEIPT_MODE` value into `ReceiptMode`.
    ///
    /// Wire stores the property as either a bare numeric (`0` /
    /// `1`) or a JSON-encoded string (`"0"` / `"1"`) depending on
    /// which client wrote it. The parser strips optional surrounding
    /// double quotes before matching. Any value other than a stripped
    /// `0` is treated as `On` — the safest default per ADR-0012.
    pub(crate) fn from_raw(value: &str) -> Self {
        match value.trim_matches('"') {
            "0" => Self::Off,
            _ => Self::On,
        }
    }
}

impl Client {
    /// Snapshot of every team-feature-config entry the server has
    /// reported for this account's team. Empty when no `SqliteStore`
    /// is attached (e.g. an in-memory test client) or when login's
    /// `fetch_server_configs` could not populate the table — the
    /// warn-and-continue posture per ADR-0012 §Login pipeline
    /// integration deliberately treats this as "use defaults".
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` when the underlying `SQLite` read fails
    /// (e.g. corrupt database).
    pub async fn feature_configs(&self) -> Result<Vec<FeatureConfigEntry>, KupferdrahtError> {
        let Some(store) = self.store_handle() else {
            return Ok(Vec::new());
        };
        store.list_feature_configs().await
    }

    /// Look up a single feature config by Wire's exact key
    /// (`fileSharing`, `mls`, `mlsE2EId`, …). `Ok(None)` when the
    /// key is absent from the persisted set or when no `SqliteStore`
    /// is attached.
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` on `SQLite` read failure.
    pub async fn feature_config(
        &self,
        name: &str,
    ) -> Result<Option<FeatureConfigEntry>, KupferdrahtError> {
        let Some(store) = self.store_handle() else {
            return Ok(None);
        };
        store.get_feature_config(name).await
    }

    /// Snapshot of every user-property entry this account has set.
    /// Empty when no `SqliteStore` is attached or when login's
    /// `/properties` fetch could not populate the table.
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` on `SQLite` read failure.
    pub async fn user_properties(&self) -> Result<Vec<UserPropertyEntry>, KupferdrahtError> {
        let Some(store) = self.store_handle() else {
            return Ok(Vec::new());
        };
        store.list_user_properties().await
    }

    /// Look up a single user property by Wire's exact key
    /// (`WIRE_RECEIPT_MODE`, `labels`, …). `Ok(None)` when the key
    /// is absent or no `SqliteStore` is attached.
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` on `SQLite` read failure.
    pub async fn user_property(
        &self,
        key: &str,
    ) -> Result<Option<UserPropertyEntry>, KupferdrahtError> {
        let Some(store) = self.store_handle() else {
            return Ok(None);
        };
        store.get_user_property(key).await
    }

    /// Typed projection of `feature_configs.fileSharing` — Step 10e.
    /// Backs the Step 11 file-attachment pre-flight check.
    ///
    /// Returns [`FileSharingPolicy::Enabled`] when the entry is
    /// absent (login-time fetch failed, or the team config does not
    /// expose `fileSharing` at all). The permissive default is the
    /// safer error mode — see ADR-0012 §Decision.
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` on `SQLite` read failure.
    pub async fn file_sharing(&self) -> Result<FileSharingPolicy, KupferdrahtError> {
        Ok(self
            .feature_config("fileSharing")
            .await?
            .map_or(FileSharingPolicy::Enabled, |e| {
                FileSharingPolicy::from_entry(&e)
            }))
    }

    /// Typed projection of `feature_configs.consumableNotifications` —
    /// Step 62c. `true` iff the server reports the RabbitMQ-backed
    /// `/v14/events` delivery as enabled; the transport-choice input
    /// for [`SyncTransport`](crate::sync::SyncTransport) consumers.
    ///
    /// Returns `false` when the entry is absent — deliberately the
    /// **opposite** default direction to [`Self::file_sharing`]'s
    /// permissive one: recommending the WebSocket against a server
    /// that never publishes into the consumable queue silently breaks
    /// receive (observations §66), while the polling fallback always
    /// works. Kept fresh by the same login fetch +
    /// `feature-config.update` pipeline as every feature config
    /// (ADR-0012).
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` on `SQLite` read failure.
    pub async fn server_supports_push_events(&self) -> Result<bool, KupferdrahtError> {
        Ok(self
            .feature_config("consumableNotifications")
            .await?
            .is_some_and(|e| e.status.as_deref() == Some("enabled")))
    }

    /// Typed projection of `user_properties.WIRE_RECEIPT_MODE` —
    /// Step 10e. Backs Step 10f's `Timeline::mark_as_read` gate:
    /// when this returns [`ReceiptMode::Off`], outbound
    /// `Confirmation { type: READ }` emission is suppressed.
    ///
    /// Returns [`ReceiptMode::On`] when the property is absent
    /// (Wire's default for accounts that haven't toggled the
    /// setting). See ADR-0012 §Decision for the default rationale.
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` on `SQLite` read failure.
    pub async fn receipt_mode(&self) -> Result<ReceiptMode, KupferdrahtError> {
        Ok(self
            .user_property("WIRE_RECEIPT_MODE")
            .await?
            .map_or(ReceiptMode::On, |p| ReceiptMode::from_raw(&p.value)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fc(status: Option<&str>, config_json: &str) -> FeatureConfigEntry {
        FeatureConfigEntry {
            name: "fileSharing".into(),
            status: status.map(str::to_string),
            config_json: config_json.into(),
            last_synced_at_ms: 0,
        }
    }

    #[test]
    fn file_sharing_disabled_status_yields_disabled() {
        let p = FileSharingPolicy::from_entry(&fc(Some("disabled"), "{}"));
        assert_eq!(p, FileSharingPolicy::Disabled);
    }

    #[test]
    fn file_sharing_enabled_with_empty_config_yields_enabled() {
        // Fixture modelled on a live backend's response.
        let p = FileSharingPolicy::from_entry(&fc(Some("enabled"), "{}"));
        assert_eq!(p, FileSharingPolicy::Enabled);
    }

    #[test]
    fn file_sharing_enabled_ignores_any_config_object() {
        // The feature defines no `config` the client acts on; whatever a
        // backend puts there must not change the policy.
        let p = FileSharingPolicy::from_entry(&fc(Some("enabled"), r#"{"someFutureField":42}"#));
        assert_eq!(p, FileSharingPolicy::Enabled);
    }

    #[test]
    fn file_sharing_no_status_field_treats_as_enabled() {
        // Wire occasionally omits the status field on enabled-by-
        // default features; the parser treats null status the same
        // as `enabled` per ADR-0012.
        let p = FileSharingPolicy::from_entry(&fc(None, "{}"));
        assert_eq!(p, FileSharingPolicy::Enabled);
    }

    #[test]
    fn file_sharing_unknown_status_defaults_to_enabled_with_warn() {
        let p = FileSharingPolicy::from_entry(&fc(Some("partial"), "{}"));
        assert_eq!(p, FileSharingPolicy::Enabled);
    }

    #[test]
    fn file_sharing_malformed_config_json_is_still_enabled() {
        // Defensive: a config_json that is not JSON at all changes
        // nothing — the status alone decides.
        let p = FileSharingPolicy::from_entry(&fc(Some("enabled"), "not-json"));
        assert_eq!(p, FileSharingPolicy::Enabled);
    }

    #[test]
    fn receipt_mode_zero_is_off() {
        assert_eq!(ReceiptMode::from_raw("0"), ReceiptMode::Off);
    }

    #[test]
    fn receipt_mode_one_is_on() {
        assert_eq!(ReceiptMode::from_raw("1"), ReceiptMode::On);
    }

    #[test]
    fn receipt_mode_quoted_zero_is_off() {
        // Wire stores some property values as JSON-encoded strings
        // (`"\"0\""` rather than `"0"`); the parser strips outer
        // quotes before matching.
        assert_eq!(ReceiptMode::from_raw(r#""0""#), ReceiptMode::Off);
    }

    #[test]
    fn receipt_mode_quoted_one_is_on() {
        assert_eq!(ReceiptMode::from_raw(r#""1""#), ReceiptMode::On);
    }

    #[test]
    fn receipt_mode_unknown_value_defaults_to_on() {
        // Forward-compat: any value that isn't `0` (or `"0"`) is
        // treated as On — the safer default per ADR-0012.
        assert_eq!(ReceiptMode::from_raw("yes"), ReceiptMode::On);
        assert_eq!(ReceiptMode::from_raw(""), ReceiptMode::On);
    }
}
