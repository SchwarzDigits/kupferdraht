// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! MLS-key HTTP surface — two endpoints:
//! `POST /v{N}/mls/key-packages/self/{client_id}` uploads the initial
//! `KeyPackage` batch; `GET /v{N}/mls/public-keys` fetches Wire's
//! backend external-sender keys (ADR-0004).

use base64::Engine as _;
use tracing::debug;

use crate::error::KupferdrahtError;
use crate::wire::dto::MlsKeysByPurpose;

use super::HttpClient;

impl HttpClient {
    /// `POST /v{api_version}/mls/key-packages/self/{client_id}` — upload
    /// a batch of freshly-generated `KeyPackage`s for this device.
    ///
    /// Each element of `key_packages` is the TLS-serialized bytes of
    /// one `KeyPackage`; the body wraps them base64-encoded into a
    /// JSON `{"key_packages": ["<b64>", ...]}` payload per Wire's
    /// swagger (`KeyPackageUpload` schema).
    ///
    /// Auth is the rebound access token (`Bearer`). Wire's nginx
    /// injects `Z-Client` from the access-token claim (LEARNINGS
    /// §1.4), so the caller must have gone through
    /// `rebind_access_token` for this `client_id` first, and must
    /// have PUT the MLS signature public key (observations.md §19) before
    /// invoking this method.
    ///
    /// # Errors
    /// * `BackendRejected` on non-2xx statuses. Wire surfaces
    ///   `400 mls-protocol-error` and `403 mls-identity-mismatch`;
    ///   both are folded into `BackendRejected` with the response
    ///   body attached for post-hoc diagnosis.
    /// * `Network` for transport failures.
    pub async fn upload_key_packages(
        &self,
        api_version: u32,
        access_token: &str,
        client_id: &str,
        key_packages: &[Vec<u8>],
    ) -> Result<(), KupferdrahtError> {
        debug_assert!(!access_token.is_empty(), "access_token must be non-empty");
        debug_assert!(!client_id.is_empty(), "client_id must be non-empty");
        debug_assert!(
            !key_packages.is_empty(),
            "key_packages must be non-empty — Wire rejects empty uploads"
        );
        let path = format!("/v{api_version}/mls/key-packages/self/{client_id}");
        let url = self.url(&path)?;
        let body = build_key_package_upload_body(key_packages);
        debug!(%url, count = key_packages.len(), "POST upload_key_packages");
        let req = self.client.post(url).bearer_auth(access_token).json(&body);
        self.send_no_body(req, "upload_key_packages").await
    }

    /// `GET /v{api_version}/mls/public-keys` — fetch Wire's backend
    /// external-sender public keys (`removal` purpose only today).
    ///
    /// Auth is the rebound access token (`Bearer`). The response is
    /// the `MLSKeysByPurpose` struct from the Wire swagger. Kupferdraht's
    /// caller picks the `ecdsa_secp256r1_sha256` entry (matching our
    /// P-256 ciphersuite, CHANGELOG.md §13) and stores it for the
    /// `external_senders` extension in every group we create or join
    /// (ADR-0004).
    ///
    /// # Errors
    /// * `BackendRejected` on non-2xx statuses.
    /// * `Network` for transport failures.
    /// * `Encoding` for malformed response bodies.
    pub async fn fetch_mls_backend_public_keys(
        &self,
        api_version: u32,
        access_token: &str,
    ) -> Result<MlsKeysByPurpose, KupferdrahtError> {
        debug_assert!(!access_token.is_empty(), "access_token must be non-empty");
        let path = format!("/v{api_version}/mls/public-keys");
        let url = self.url(&path)?;
        debug!(%url, "GET mls/public-keys");
        let req = self.client.get(url).bearer_auth(access_token);
        self.send_and_decode::<MlsKeysByPurpose>(req, "fetch_mls_backend_public_keys")
            .await
    }

    /// `GET /v{N}/mls/key-packages/self/{client_id}/count` — remaining
    /// unclaimed key packages for this device (Step 63). Every peer
    /// that adds this device to a conversation claims one; the count
    /// only ever shrinks between uploads. Response: `{"count": <n>}`
    /// (swagger `OwnKeyPackages`).
    ///
    /// `ciphersuite` is **required** by current wire-server (400
    /// without it — live-observed on the self-hosted
    /// instance; the pinned swagger does not mark it) and is sent
    /// decimal, same convention as the claim endpoint.
    ///
    /// # Errors
    /// * `BackendRejected` on non-2xx statuses.
    /// * `Network` for transport failures.
    /// * `Encoding` for malformed response bodies.
    pub async fn fetch_key_package_count(
        &self,
        api_version: u32,
        access_token: &str,
        client_id: &str,
        ciphersuite_decimal: u16,
    ) -> Result<u64, KupferdrahtError> {
        debug_assert!(!access_token.is_empty(), "access_token must be non-empty");
        debug_assert!(!client_id.is_empty(), "client_id must be non-empty");
        let path = format!(
            "/v{api_version}/mls/key-packages/self/{client_id}/count?ciphersuite={ciphersuite_decimal}"
        );
        let url = self.url(&path)?;
        debug!(%url, "GET key-package count");
        let req = self.client.get(url).bearer_auth(access_token);
        Ok(self
            .send_and_decode::<OwnKeyPackages>(req, "fetch_key_package_count")
            .await?
            .count)
    }
}

/// Response of the key-package count endpoint — swagger
/// `OwnKeyPackages`, verbatim Wire naming.
#[derive(Debug, serde::Deserialize)]
struct OwnKeyPackages {
    count: u64,
}

/// Build the JSON body for `POST /v{N}/mls/key-packages/self/{client_id}`.
/// Each entry is the base64-encoded TLS-serialization of a `KeyPackage`.
fn build_key_package_upload_body(key_packages: &[Vec<u8>]) -> serde_json::Value {
    let encoded: Vec<String> = key_packages
        .iter()
        .map(|kp| base64::engine::general_purpose::STANDARD.encode(kp))
        .collect();
    serde_json::json!({ "key_packages": encoded })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_key_package_upload_body_encodes_entries_base64() {
        let packages = vec![vec![0x01u8, 0x02, 0x03], vec![0xAA, 0xBB]];
        let body = build_key_package_upload_body(&packages);
        let arr = body["key_packages"]
            .as_array()
            .expect("key_packages must be a JSON array");
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0], "AQID", "0x01 0x02 0x03 → AQID");
        assert_eq!(arr[1], "qrs=", "0xAA 0xBB → qrs=");
    }

    #[test]
    fn build_key_package_upload_body_accepts_empty_vec_but_schema_is_a_kupferdraht_concern() {
        // The helper itself does not guard against empty input — the
        // upstream assertion in `upload_key_packages` does. This test
        // documents that separation and keeps us from accidentally
        // moving the empty-check into the body builder.
        let body = build_key_package_upload_body(&[]);
        assert!(body["key_packages"].as_array().unwrap().is_empty());
    }
}
