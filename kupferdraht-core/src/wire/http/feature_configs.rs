// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Wire feature-configs and user-properties HTTP layer (Step 10a).
//!
//! Three `GET` endpoints from Wire API v14, all read-only:
//!
//! - `GET /v{N}/feature-configs` — entire team-feature-config object
//!   in one response. Returns a JSON map of feature name →
//!   `{status, config}`.
//! - `GET /v{N}/properties` — list of user-property keys.
//! - `GET /v{N}/properties/{key}` — opaque value for one property.
//!
//! `PUT` and `DELETE` on `/v{N}/properties/{key}` are intentionally
//! NOT implemented — see ADR-0012 §Out of scope and
//! `docs/plan/10-server-feature-configs.md`. The
//! `user.properties-delete` event handler in 10d clears only the
//! local `SQLite` row; calling Wire's DELETE from the handler would
//! re-trigger the same event (Wire is the source of truth for the
//! deletion in the first place).
//!
//! [`RawFeatureConfigs`] wraps `serde_json::Value` so callers — 10c's
//! login pipeline integration and 10d's event dispatch — can iterate
//! over per-feature entries without per-feature deserialisation here.
//!
//! # TODO(gap-B.1: ciphersuite ADR, number TBD): consume `mls.config.*`
//!
//! Step 10's generic store accepts `mls.config.defaultCipherSuite` and
//! `mls.config.allowedCipherSuites` like every other config, but
//! Kupferdraht currently hardcodes `MlsClient::MLS_CIPHERSUITE` to P-256.
//! A team whose feature config requires a different ciphersuite will
//! see silent group-add failures. Ciphersuite negotiation is deferred
//! to a future ciphersuite ADR (open.md gap-B.1); see
//! `docs/plan/archive/10-server-feature-configs.md`
//! §Implementation notes for the path forward.

use serde_json::Value;
use tracing::debug;

use crate::error::KupferdrahtError;
use crate::storage::FeatureConfigEntry;

use super::{HttpClient, reject_from_response};

/// Opaque wrapper around the parsed `/v{N}/feature-configs` response.
///
/// Wire ships the response as a JSON object whose keys are feature
/// names (`fileSharing`, `mls`, `mlsE2EId`, …) and whose values are
/// per-feature objects of shape `{status, config}`. 10a's HTTP layer
/// parses the envelope only — typed per-feature interpretation
/// (`FileSharingPolicy`, `ReceiptMode`, …) lives in 10e.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone)]
pub struct RawFeatureConfigs {
    inner: Value,
}

#[cfg_attr(not(test), allow(dead_code))]
impl RawFeatureConfigs {
    /// Build from a parsed `serde_json::Value`. Returns
    /// `KupferdrahtError::Encoding` if the top-level value is not a JSON
    /// object.
    ///
    /// # Errors
    /// Returns `KupferdrahtError::Encoding` for non-object payloads.
    pub fn from_value(v: Value) -> Result<Self, KupferdrahtError> {
        if !v.is_object() {
            return Err(KupferdrahtError::Encoding(
                "/feature-configs body must be a JSON object".into(),
            ));
        }
        Ok(Self { inner: v })
    }

    /// Iterate over `(name, status, config)` triples.
    ///
    /// `status` is the raw `"enabled"` / `"disabled"` JSON string when
    /// present, `None` for status-only or config-only features.
    /// `config` is the per-feature `config` value when present.
    #[must_use]
    pub fn iter(&self) -> RawFeatureConfigsIter<'_> {
        RawFeatureConfigsIter {
            inner: self.inner.as_object().map(serde_json::Map::iter),
        }
    }

    /// Convert into the persistence-shape entries for
    /// `Store::replace_feature_configs`. Every entry receives the
    /// same `last_synced_at_ms` so a single login fetch produces a
    /// coherent timestamp across rows.
    #[must_use]
    pub fn into_entries(self, last_synced_at_ms: i64) -> Vec<FeatureConfigEntry> {
        self.iter()
            .map(|(name, status, config)| FeatureConfigEntry {
                name: name.to_string(),
                status: status.map(str::to_string),
                config_json: config.map_or_else(empty_config_json, Value::to_string),
                last_synced_at_ms,
            })
            .collect()
    }
}

/// Returns the JSON representation of an empty object (`"{}"`),
/// used as the default `config_json` when a feature carries no
/// `config` field in the Wire response.
fn empty_config_json() -> String {
    String::from("{}")
}

/// Iterator returned by [`RawFeatureConfigs::iter`].
#[cfg_attr(not(test), allow(dead_code))]
pub struct RawFeatureConfigsIter<'a> {
    inner: Option<serde_json::map::Iter<'a>>,
}

impl<'a> Iterator for RawFeatureConfigsIter<'a> {
    type Item = (&'a str, Option<&'a str>, Option<&'a Value>);

    fn next(&mut self) -> Option<Self::Item> {
        let (k, v) = self.inner.as_mut()?.next()?;
        let status = v.get("status").and_then(Value::as_str);
        let config = v.get("config");
        Some((k.as_str(), status, config))
    }
}

#[cfg_attr(not(test), allow(dead_code))]
impl HttpClient {
    /// `GET /v{api_version}/feature-configs` — full team-feature-config
    /// response in a single call.
    ///
    /// Wire's response is `{ "fileSharing": {...}, "mls": {...}, ... }`
    /// — see [`RawFeatureConfigs`] for the iteration shape. Wire does
    /// not expose per-feature read endpoints under v14, so the entire
    /// response is the only granularity available; both login-time
    /// seed (10c) and live `feature-config.update` refetch (10d) call
    /// this same method.
    ///
    /// # Errors
    /// * `BackendRejected` on non-2xx statuses (including `404
    ///   no-team` for users not on a team).
    /// * `Network` for transport failures.
    /// * `Encoding` for malformed response bodies.
    pub async fn fetch_feature_configs(
        &self,
        api_version: u32,
        access_token: &str,
    ) -> Result<RawFeatureConfigs, KupferdrahtError> {
        debug_assert!(!access_token.is_empty(), "access_token must be non-empty");
        let path = format!("/v{api_version}/feature-configs");
        let url = self.url(&path)?;
        debug!(%url, "GET feature-configs");
        let req = self.client.get(url).bearer_auth(access_token);
        let body = self
            .send_and_decode::<Value>(req, "fetch_feature_configs")
            .await?;
        RawFeatureConfigs::from_value(body)
    }

    /// `GET /v{api_version}/properties` — list of user-property keys.
    ///
    /// Wire returns a JSON array of strings (`["WIRE_RECEIPT_MODE",
    /// "labels", …]`). The values themselves are fetched per-key via
    /// [`HttpClient::fetch_property_value`].
    ///
    /// # Errors
    /// * `BackendRejected` on non-2xx statuses.
    /// * `Network` for transport failures.
    /// * `Encoding` for malformed response bodies.
    pub async fn fetch_property_keys(
        &self,
        api_version: u32,
        access_token: &str,
    ) -> Result<Vec<String>, KupferdrahtError> {
        debug_assert!(!access_token.is_empty(), "access_token must be non-empty");
        let path = format!("/v{api_version}/properties");
        let url = self.url(&path)?;
        debug!(%url, "GET property keys");
        let req = self.client.get(url).bearer_auth(access_token);
        self.send_and_decode::<Vec<String>>(req, "fetch_property_keys")
            .await
    }

    /// `GET /v{api_version}/properties/{key}` — opaque value for one
    /// property.
    ///
    /// The response body is returned verbatim as a `String`. Wire
    /// mixes JSON-encoded strings (e.g. `"\"on\""`) with raw numeric
    /// strings (e.g. `"1"`) here depending on the client that wrote
    /// the value; typed getters in 10e strip optional surrounding
    /// quotes before parsing.
    ///
    /// # Errors
    /// * `BackendRejected` on non-2xx statuses (including 404
    ///   `not-found` when the key was deleted between the
    ///   keys-list fetch and this call).
    /// * `InvalidInput` when `key` is not a single URL path segment.
    /// * `Network` for transport failures.
    pub async fn fetch_property_value(
        &self,
        api_version: u32,
        access_token: &str,
        key: &str,
    ) -> Result<String, KupferdrahtError> {
        debug_assert!(!access_token.is_empty(), "access_token must be non-empty");
        let key = Self::path_segment(key, "fetch_property_value key")?;
        let path = format!("/v{api_version}/properties/{key}");
        let url = self.url(&path)?;
        debug!(%url, "GET property value");
        let resp = self
            .client
            .get(url)
            .bearer_auth(access_token)
            .send()
            .await
            .map_err(|e| KupferdrahtError::Network(format!("GET {path}: {e}")))?;
        if !resp.status().is_success() {
            return Err(reject_from_response(resp, "fetch_property_value").await);
        }
        resp.text()
            .await
            .map_err(|e| KupferdrahtError::Network(format!("read property body: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::Method::GET;
    use httpmock::MockServer;
    use serde_json::json;

    /// Synthesised representative `/v14/feature-configs` body. Mirrors
    /// the shape pinned by ADR-0012 §Context: a JSON object keyed by
    /// feature name, each value `{status, config}` where `config` is
    /// optional. Five entries chosen to cover the corners:
    ///
    /// - `fileSharing` — `enabled`, with an empty `config` (the live
    ///   shape observed 2026-05-08; Step 11 entry point).
    /// - `mls` — `enabled`, with a multi-field `config` payload
    ///   (the `mls.config.*` cluster the future ADR-0013 will
    ///   consume).
    /// - `mlsE2EId` — `disabled`, no `config`.
    /// - `selfDeletingMessages` — `enabled`, single-field `config`.
    /// - `appLock` — `disabled`, no `config`.
    ///
    /// The 10c/10f follow-up captures an anonymised real Wire response
    /// and replaces this fixture; until then this synthesis is the
    /// canonical input shape.
    fn representative_feature_configs() -> serde_json::Value {
        json!({
            "fileSharing": {
                "status": "enabled",
                "config": {}
            },
            "mls": {
                "status": "enabled",
                "config": {
                    "defaultProtocol": "mls",
                    "defaultCipherSuite": 1,
                    "allowedCipherSuites": [1, 2]
                }
            },
            "mlsE2EId": { "status": "disabled" },
            "selfDeletingMessages": {
                "status": "enabled",
                "config": { "enforcedTimeoutSeconds": 0 }
            },
            "appLock": { "status": "disabled" }
        })
    }

    fn http(base_url: &str) -> HttpClient {
        HttpClient::new(base_url, true).expect("HttpClient::new (permit_http=true)")
    }

    // -- RawFeatureConfigs unit tests ---------------------------------

    #[test]
    fn from_value_rejects_non_object() {
        let arr = json!([1, 2, 3]);
        let err = RawFeatureConfigs::from_value(arr).expect_err("expected error");
        assert!(matches!(err, KupferdrahtError::Encoding(_)), "got {err:?}");
    }

    #[test]
    fn iter_yields_status_and_config_for_each_feature() {
        let raw =
            RawFeatureConfigs::from_value(representative_feature_configs()).expect("from_value");
        let mut by_name = std::collections::BTreeMap::new();
        for (name, status, config) in raw.iter() {
            by_name.insert(
                name.to_string(),
                (status.map(str::to_string), config.cloned()),
            );
        }
        // mls: enabled + config object present
        let (status, config) = by_name.get("mls").expect("mls present");
        assert_eq!(status.as_deref(), Some("enabled"));
        assert!(
            config
                .as_ref()
                .and_then(|c| c.get("defaultProtocol"))
                .is_some()
        );

        // mlsE2EId: disabled, no config
        let (status, config) = by_name.get("mlsE2EId").expect("mlsE2EId present");
        assert_eq!(status.as_deref(), Some("disabled"));
        assert!(config.is_none(), "mlsE2EId should have no config");

        // appLock: disabled, no config
        let (status, config) = by_name.get("appLock").expect("appLock present");
        assert_eq!(status.as_deref(), Some("disabled"));
        assert!(config.is_none(), "appLock should have no config");
    }

    #[test]
    fn into_entries_emits_one_row_per_feature_with_uniform_timestamp() {
        let raw =
            RawFeatureConfigs::from_value(representative_feature_configs()).expect("from_value");
        let entries = raw.into_entries(1_700_000_000_000);
        assert_eq!(entries.len(), 5);
        for e in &entries {
            assert_eq!(e.last_synced_at_ms, 1_700_000_000_000);
        }
        let by_name: std::collections::BTreeMap<_, _> =
            entries.iter().map(|e| (e.name.clone(), e)).collect();
        // mlsE2EId: status=Some("disabled"), config_json="{}"
        let e = by_name.get("mlsE2EId").expect("mlsE2EId present");
        assert_eq!(e.status.as_deref(), Some("disabled"));
        assert_eq!(e.config_json, "{}");
        // mls: status=Some("enabled"), config_json keeps the object verbatim
        let e = by_name.get("mls").expect("mls present");
        assert_eq!(e.status.as_deref(), Some("enabled"));
        assert!(
            e.config_json.contains("defaultProtocol"),
            "got {}",
            e.config_json
        );
    }

    #[test]
    fn into_entries_uses_empty_object_when_config_is_absent() {
        // Feature with no `config` field must produce `config_json == "{}"`
        // (the empty_config_json fallback), not an empty string or null.
        let raw = RawFeatureConfigs::from_value(json!({
            "appLock": { "status": "disabled" }
        }))
        .expect("from_value");
        let entries = raw.into_entries(0);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].config_json, "{}");
    }

    // -- HttpClient::fetch_feature_configs ----------------------------

    #[tokio::test]
    async fn fetch_feature_configs_parses_representative_response() {
        let server = MockServer::start();
        let body = representative_feature_configs().to_string();
        let mock = server.mock(|when, then| {
            when.method(GET)
                .path("/v14/feature-configs")
                .header("authorization", "Bearer tok");
            then.status(200)
                .header("content-type", "application/json")
                .body(body);
        });
        let raw = http(&server.base_url())
            .fetch_feature_configs(14, "tok")
            .await
            .expect("fetch_feature_configs");
        let entries = raw.into_entries(42);
        assert_eq!(entries.len(), 5);
        mock.assert_calls(1);
    }

    #[tokio::test]
    async fn fetch_feature_configs_404_is_backend_rejected() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(GET).path("/v14/feature-configs");
            then.status(404)
                .header("content-type", "application/json")
                .body(r#"{"code":404,"label":"no-team","message":"Team not found"}"#);
        });
        let err = http(&server.base_url())
            .fetch_feature_configs(14, "tok")
            .await
            .expect_err("expected 404 to surface as error");
        assert!(
            matches!(err, KupferdrahtError::BackendRejected(_)),
            "got {err:?}"
        );
        mock.assert_calls(1);
    }

    // -- HttpClient::fetch_property_keys ------------------------------

    #[tokio::test]
    async fn fetch_property_keys_parses_json_array() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(GET)
                .path("/v14/properties")
                .header("authorization", "Bearer tok");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"["WIRE_RECEIPT_MODE","labels"]"#);
        });
        let keys = http(&server.base_url())
            .fetch_property_keys(14, "tok")
            .await
            .expect("fetch_property_keys");
        assert_eq!(
            keys,
            vec!["WIRE_RECEIPT_MODE".to_string(), "labels".to_string()]
        );
        mock.assert_calls(1);
    }

    #[tokio::test]
    async fn fetch_property_keys_404_is_backend_rejected() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(GET).path("/v14/properties");
            then.status(404).body(r#"{"code":404,"label":"not-found"}"#);
        });
        let err = http(&server.base_url())
            .fetch_property_keys(14, "tok")
            .await
            .expect_err("expected 404 to surface as error");
        assert!(
            matches!(err, KupferdrahtError::BackendRejected(_)),
            "got {err:?}"
        );
        mock.assert_calls(1);
    }

    // -- HttpClient::fetch_property_value -----------------------------

    #[tokio::test]
    async fn fetch_property_value_returns_quoted_string_verbatim() {
        // Wire stores some property values as JSON-encoded strings —
        // body bytes look like `"on"` (four chars including the
        // surrounding quotes). The HTTP layer hands them back
        // unaltered; typed getters in 10e strip the quotes.
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(GET)
                .path("/v14/properties/labels")
                .header("authorization", "Bearer tok");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#""on""#);
        });
        let v = http(&server.base_url())
            .fetch_property_value(14, "tok", "labels")
            .await
            .expect("fetch_property_value");
        assert_eq!(v, r#""on""#);
        mock.assert_calls(1);
    }

    #[tokio::test]
    async fn fetch_property_value_returns_bare_numeric_verbatim() {
        // Wire stores `WIRE_RECEIPT_MODE` as a bare numeric (`1` /
        // `0`) — body bytes are a single digit, not a JSON-encoded
        // string. Same handing-back-verbatim contract.
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(GET).path("/v14/properties/WIRE_RECEIPT_MODE");
            then.status(200)
                .header("content-type", "application/json")
                .body("1");
        });
        let v = http(&server.base_url())
            .fetch_property_value(14, "tok", "WIRE_RECEIPT_MODE")
            .await
            .expect("fetch_property_value");
        assert_eq!(v, "1");
        mock.assert_calls(1);
    }

    #[tokio::test]
    async fn fetch_property_value_404_is_backend_rejected() {
        // Per ADR-0012: at the HTTP layer a 404 is uniform with every
        // other endpoint — `BackendRejected`. The "404 means deleted"
        // higher-layer interpretation is 10c/10d's concern, not 10a's.
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(GET).path("/v14/properties/missing");
            then.status(404).body(r#"{"code":404,"label":"not-found"}"#);
        });
        let err = http(&server.base_url())
            .fetch_property_value(14, "tok", "missing")
            .await
            .expect_err("expected 404 to surface as error");
        assert!(
            matches!(err, KupferdrahtError::BackendRejected(_)),
            "got {err:?}"
        );
        mock.assert_calls(1);
    }
}
