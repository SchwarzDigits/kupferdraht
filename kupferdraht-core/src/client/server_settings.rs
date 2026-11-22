// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `Client` accessors for the server's settings and the account data.
//!
//! Two layers:
//!
//! - **Raw accessors (Step 10c)** — `server_settings()` /
//!   `server_setting(name)` / `account_data()` /
//!   `account_data_entry(key)` return the persisted rows verbatim.
//! - **Typed getters (Step 10e)** — `file_sharing()` /
//!   `read_receipt_setting()` project the raw entries into
//!   [`FileSharingPolicy`] / [`ReadReceiptSetting`] so reactive code
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
//! These accessors read from the persisted rows — no Wire
//! round-trip per call.

use tracing::warn;

use crate::client::Client;
use crate::error::KupferdrahtError;
use crate::storage::{AccountDataEntry, ServerSetting};

/// Team-policy projection of the `fileSharing` server setting. Step 10e.
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
    pub(crate) fn from_entry(entry: &ServerSetting) -> Self {
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

/// The read-receipt setting, projected from the account data. Step 10e.
/// Step 10f's `Timeline::mark_as_read` consults this to suppress
/// outbound `Confirmation { type: READ }` emission when the user has
/// read receipts disabled in their Wire settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ReadReceiptSetting {
    Off,
    On,
}

impl ReadReceiptSetting {
    /// Parse the backend's `WIRE_RECEIPT_MODE` value into `ReadReceiptSetting`.
    ///
    /// Wire stores the entry as either a bare numeric (`0` /
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
    /// Snapshot of every server setting the backend has reported
    /// for this account's team. Empty when no `SqliteStore`
    /// is attached (e.g. an in-memory test client) or when login's
    /// settings fetch could not populate them — the
    /// warn-and-continue posture per ADR-0012 §Login pipeline
    /// integration deliberately treats this as "use defaults".
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` when the underlying `SQLite` read fails
    /// (e.g. corrupt database).
    pub async fn server_settings(&self) -> Result<Vec<ServerSetting>, KupferdrahtError> {
        let Some(store) = self.store_handle() else {
            return Ok(Vec::new());
        };
        store.list_server_settings().await
    }

    /// Look up a single server setting by the backend's exact name
    /// (`fileSharing`, `mls`, `mlsE2EId`, …). `Ok(None)` when the
    /// name is absent from the persisted set or when no `SqliteStore`
    /// is attached.
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` on `SQLite` read failure.
    pub async fn server_setting(
        &self,
        name: &str,
    ) -> Result<Option<ServerSetting>, KupferdrahtError> {
        let Some(store) = self.store_handle() else {
            return Ok(None);
        };
        store.get_server_setting(name).await
    }

    /// Snapshot of every account-data entry this account has set.
    /// Empty when no `SqliteStore` is attached or when login's
    /// `/properties` fetch could not populate them.
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` on `SQLite` read failure.
    pub async fn account_data(&self) -> Result<Vec<AccountDataEntry>, KupferdrahtError> {
        let Some(store) = self.store_handle() else {
            return Ok(Vec::new());
        };
        store.list_account_data().await
    }

    /// Look up a single account-data entry by the backend's exact key
    /// (`labels`, …). `Ok(None)` when the key is absent or no
    /// `SqliteStore` is attached.
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` on `SQLite` read failure.
    pub async fn account_data_entry(
        &self,
        key: &str,
    ) -> Result<Option<AccountDataEntry>, KupferdrahtError> {
        let Some(store) = self.store_handle() else {
            return Ok(None);
        };
        store.get_account_data_entry(key).await
    }

    /// Typed projection of the `fileSharing` server setting — Step 10e.
    /// Backs the Step 11 file-attachment pre-flight check.
    ///
    /// Returns [`FileSharingPolicy::Enabled`] when the entry is
    /// absent (login-time fetch failed, or the team's settings do not
    /// expose `fileSharing` at all). The permissive default is the
    /// safer error mode — see ADR-0012 §Decision.
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` on `SQLite` read failure.
    pub async fn file_sharing(&self) -> Result<FileSharingPolicy, KupferdrahtError> {
        Ok(self
            .server_setting("fileSharing")
            .await?
            .map_or(FileSharingPolicy::Enabled, |e| {
                FileSharingPolicy::from_entry(&e)
            }))
    }

    /// Typed projection of the `consumableNotifications` server setting —
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
    /// `feature-config.update` pipeline as every server setting
    /// (ADR-0012).
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` on `SQLite` read failure.
    pub async fn server_supports_push_events(&self) -> Result<bool, KupferdrahtError> {
        Ok(self
            .server_setting("consumableNotifications")
            .await?
            .is_some_and(|e| e.status.as_deref() == Some("enabled")))
    }

    /// Typed projection of the account's read-receipt entry —
    /// Step 10e. Backs Step 10f's `Timeline::mark_as_read` gate:
    /// when this returns [`ReadReceiptSetting::Off`], outbound
    /// `Confirmation { type: READ }` emission is suppressed.
    ///
    /// Returns [`ReadReceiptSetting::On`] when the entry is absent
    /// (Wire's default for accounts that haven't toggled the
    /// setting). See ADR-0012 §Decision for the default rationale.
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` on `SQLite` read failure.
    pub async fn read_receipt_setting(&self) -> Result<ReadReceiptSetting, KupferdrahtError> {
        Ok(self
            .account_data_entry("WIRE_RECEIPT_MODE")
            .await?
            .map_or(ReadReceiptSetting::On, |p| {
                ReadReceiptSetting::from_raw(&p.value)
            }))
    }
}

/// Refuse a backend whose team default MLS ciphersuite is not ours (Step
/// 132, NG-25): one suite, not negotiated — on another default the first
/// group add would fail with `no KeyPackages` (live: observations §94).
/// Runs wherever the server settings are fetched: both login paths and the
/// `feature-config.update` refetch. A backend that names no default passes.
///
/// # Errors
/// `KupferdrahtError::CiphersuiteMismatch` naming both suites.
pub(crate) fn ensure_default_ciphersuite(
    raw: &crate::wire::http::feature_configs::RawFeatureConfigs,
) -> Result<(), KupferdrahtError> {
    let ours = u16::from(crate::encryption::MLS_CIPHERSUITE_FOR_CLAIM);
    match raw.default_cipher_suite() {
        Some(backend) if backend != ours => {
            Err(KupferdrahtError::CiphersuiteMismatch { backend, ours })
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fc(status: Option<&str>, config_json: &str) -> ServerSetting {
        ServerSetting {
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
    fn read_receipt_setting_zero_is_off() {
        assert_eq!(ReadReceiptSetting::from_raw("0"), ReadReceiptSetting::Off);
    }

    #[test]
    fn read_receipt_setting_one_is_on() {
        assert_eq!(ReadReceiptSetting::from_raw("1"), ReadReceiptSetting::On);
    }

    #[test]
    fn read_receipt_setting_quoted_zero_is_off() {
        // Wire stores some property values as JSON-encoded strings
        // (`"\"0\""` rather than `"0"`); the parser strips outer
        // quotes before matching.
        assert_eq!(
            ReadReceiptSetting::from_raw(r#""0""#),
            ReadReceiptSetting::Off
        );
    }

    #[test]
    fn read_receipt_setting_quoted_one_is_on() {
        assert_eq!(
            ReadReceiptSetting::from_raw(r#""1""#),
            ReadReceiptSetting::On
        );
    }

    #[test]
    fn read_receipt_setting_unknown_value_defaults_to_on() {
        // Forward-compat: any value that isn't `0` (or `"0"`) is
        // treated as On — the safer default per ADR-0012.
        assert_eq!(ReadReceiptSetting::from_raw("yes"), ReadReceiptSetting::On);
        assert_eq!(ReadReceiptSetting::from_raw(""), ReadReceiptSetting::On);
    }
}
