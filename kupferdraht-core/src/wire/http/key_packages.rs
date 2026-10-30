// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `POST /v{N}/mls/key-packages/claim/{user_domain}/{user}` —
//! Wire's per-device `KeyPackage` claim endpoint, used by Step 6b's
//! `Room::invite` flow.
//!
//! For each invitee, Kupferdraht claims one `KeyPackage` **per device the
//! user has registered**. Wire returns a bundle that lists each
//! claimed package with its owning user / domain / `client_id`; the
//! caller deserializes the `key_package` field (base64 of a
//! TLS-serialized `KeyPackage`) into an openmls `KeyPackage` and
//! feeds it into `MlsGroup::add_members`.
//!
//! Wire-API quirks (see `docs/wire/api-notes.md` § "POST
//! mls/key-packages/claim"):
//!
//! * **Ciphersuite query is decimal** (`?ciphersuite=2`) even though
//!   the swagger says hex (`0xf031`). The Wire backend accepts
//!   both forms, but live smoke-tests against other Wire clients
//!   show decimal on the wire — that's what Kupferdraht ships too.
//! * **`skip_own=<own_device_id>` MUST be set** when claiming for
//!   yourself, otherwise Wire defensively returns an empty pool
//!   (live-validated, observations.md §27). Kupferdraht passes its own
//!   `client_id` here; the field is omitted for `user != self`.

use base64::Engine as _;
use serde::Deserialize;
use tracing::debug;

use crate::error::KupferdrahtError;

use super::HttpClient;

/// Response of `POST /v{N}/mls/key-packages/claim/...` — the
/// `KeyPackageBundle` schema. One entry per device of the claimed
/// user (excluding `skip_own` if set).
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(not(test), allow(dead_code))]
pub struct ClaimedKeyPackages {
    pub key_packages: Vec<ClaimedKeyPackage>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)] // user / domain are diagnostic-only fields today
pub struct ClaimedKeyPackage {
    pub user: String,
    pub domain: String,
    pub client: String,
    /// Base64-encoded TLS-serialized openmls `KeyPackage`.
    pub key_package: String,
    /// Wire's `KeyPackage` hash, opaque to Kupferdraht today (Wire uses
    /// it for ack accounting in commit bundles).
    #[serde(default)]
    pub key_package_ref: Option<String>,
}

impl ClaimedKeyPackage {
    /// Decode the base64 + TLS-serialized `KeyPackage` bytes that
    /// `MlsGroup::add_members` consumes after `KeyPackageIn::validate`.
    ///
    /// # Errors
    /// `KupferdrahtError::Encoding` if the field is not valid base64.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn key_package_bytes(&self) -> Result<Vec<u8>, KupferdrahtError> {
        base64::engine::general_purpose::STANDARD
            .decode(&self.key_package)
            .map_err(|e| KupferdrahtError::Encoding(format!("claim key_package b64: {e}")))
    }
}

#[cfg_attr(not(test), allow(dead_code))]
impl HttpClient {
    /// `POST /v{api_version}/mls/key-packages/claim/{user_domain}/{user}`
    /// — claim one `KeyPackage` per device the target user has
    /// registered, excluding the caller's own device when
    /// `skip_own` is set.
    ///
    /// `ciphersuite` is sent as decimal (Wire accepts both decimal
    /// and hex, but live smoke-tests show other Wire clients ship
    /// decimal on the wire — see `docs/wire/api-notes.md`
    /// § "POST mls/key-packages/claim").
    ///
    /// # Errors
    /// * `BackendRejected` on non-2xx statuses. Wire surfaces
    ///   `404 mls-not-enabled`, `404 mls-key-package-ref-not-found`,
    ///   and `403 mls-stale-message` here; all fold into
    ///   `BackendRejected` with the response body attached.
    /// * `Network` for transport failures.
    /// * `Encoding` for malformed response bodies.
    pub async fn claim_key_packages(
        &self,
        api_version: u32,
        access_token: &str,
        user_domain: &str,
        user_uuid: &str,
        ciphersuite_decimal: u16,
        skip_own_device_id: Option<&str>,
    ) -> Result<ClaimedKeyPackages, KupferdrahtError> {
        debug_assert!(!access_token.is_empty(), "access_token must be non-empty");
        debug_assert!(!user_domain.is_empty(), "user_domain must be non-empty");
        debug_assert!(!user_uuid.is_empty(), "user_uuid must be non-empty");
        let path = format!("/v{api_version}/mls/key-packages/claim/{user_domain}/{user_uuid}");
        let mut url = self.url(&path)?;
        {
            let mut q = url.query_pairs_mut();
            q.append_pair("ciphersuite", &ciphersuite_decimal.to_string());
            if let Some(own) = skip_own_device_id {
                q.append_pair("skip_own", own);
            }
        }
        debug!(
            %url,
            user = %crate::wire::dto::qualified_user_id(user_uuid, user_domain),
            ciphersuite = ciphersuite_decimal,
            "POST mls/key-packages/claim"
        );
        let req = self.client.post(url).bearer_auth(access_token);
        self.send_and_decode::<ClaimedKeyPackages>(req, "claim_key_packages")
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::Method::POST;
    use httpmock::MockServer;

    fn http(base_url: &str) -> HttpClient {
        HttpClient::new(base_url, true).expect("HttpClient::new (permit_http=true)")
    }

    #[tokio::test]
    async fn claim_key_packages_sends_decimal_ciphersuite_and_skip_own() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/v14/mls/key-packages/claim/example.test/bob")
                .header("authorization", "Bearer tok")
                .query_param("ciphersuite", "2")
                .query_param("skip_own", "my-device-id");
            then.status(200)
                .header("content-type", "application/json")
                .body(
                    r#"{"key_packages":[
                         {"user":"bob","domain":"example.test","client":"d1","key_package":"AQID","key_package_ref":"r1"},
                         {"user":"bob","domain":"example.test","client":"d2","key_package":"qrs="}
                       ]}"#,
                );
        });
        let resp = http(&server.base_url())
            .claim_key_packages(14, "tok", "example.test", "bob", 2, Some("my-device-id"))
            .await
            .expect("claim must succeed");
        assert_eq!(resp.key_packages.len(), 2);
        assert_eq!(resp.key_packages[0].client, "d1");
        // Round-trip the base64 — d2 entry has no ref so the field
        // defaults to None.
        assert!(resp.key_packages[1].key_package_ref.is_none());
        let bytes = resp.key_packages[0].key_package_bytes().expect("b64");
        assert_eq!(bytes, vec![0x01, 0x02, 0x03]);
        mock.assert_calls(1);
    }

    #[tokio::test]
    async fn claim_key_packages_omits_skip_own_when_none() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/v14/mls/key-packages/claim/example.test/alice")
                .is_true(|req| !req.query_params().iter().any(|(k, _)| k == "skip_own"));
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"key_packages":[]}"#);
        });
        let _ = http(&server.base_url())
            .claim_key_packages(14, "tok", "example.test", "alice", 2, None)
            .await
            .expect("claim without skip_own must succeed");
        mock.assert_calls(1);
    }

    #[tokio::test]
    async fn claim_key_packages_surfaces_mls_not_enabled() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path_includes("/key-packages/claim");
            then.status(404)
                .header("content-type", "application/json")
                .body(r#"{"code":404,"label":"mls-not-enabled","message":"off"}"#);
        });
        let err = http(&server.base_url())
            .claim_key_packages(14, "tok", "example.test", "bob", 2, None)
            .await
            .expect_err("404 must propagate");
        assert!(matches!(err, KupferdrahtError::BackendRejected(_)));
    }
}
