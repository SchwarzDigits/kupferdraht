// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Device-lifecycle HTTP surface: `POST /v{N}/clients`,
//! `PUT /v{N}/clients/{id}`.

use base64::Engine as _;
use serde::Serialize;
use tracing::debug;

use crate::encryption::prekeys::{PreKey, PrekeyBundle};
use crate::error::KupferdrahtError;
use crate::wire::dto::{NewClientResponse, WireClient};

use super::{HttpClient, reject_from_response};

#[derive(Serialize)]
struct WirePreKeyJson {
    id: u32,
    key: String,
}

/// Body of `POST /v{N}/clients` — the Swagger's `NewClient` schema, the
/// fields Kupferdraht sends, in the schema's (alphabetical) order.
#[derive(Serialize)]
struct NewClientRequest<'a> {
    class: &'static str,
    cookie: &'a str,
    label: &'a str,
    lastkey: WirePreKeyJson,
    model: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    password: Option<&'a str>,
    prekeys: Vec<WirePreKeyJson>,
    #[serde(rename = "type")]
    client_type: &'static str,
}

impl HttpClient {
    /// `POST /v{api_version}/clients` — register this device.
    ///
    /// Uploads the Proteus prekey bundle (Wire schema requirement)
    /// together with the user's password (Wire re-confirms identity
    /// to prevent a stolen-session registration), client class, and
    /// cookie label. Returns the new `client_id` from Wire's 201.
    ///
    /// `password` is `Option`: Wire requires it for the 2nd, 3rd, …
    /// client of a password account but **not** for the first client,
    /// and password-less accounts verify a registration differently
    /// (`verification_code`). When `None`, the field is omitted.
    ///
    /// # Errors
    /// * `BackendRejected` on non-2xx statuses.
    /// * `Network` for transport failures.
    /// * `Encoding` for malformed response bodies.
    pub async fn register_client(
        &self,
        api_version: u32,
        access_token: &str,
        password: Option<&str>,
        prekeys: &PrekeyBundle,
        cookie_label: &str,
        device_label: Option<&str>,
    ) -> Result<NewClientResponse, KupferdrahtError> {
        debug_assert!(!access_token.is_empty(), "access_token must be non-empty");
        let path = format!("/v{api_version}/clients");
        let url = self.url(&path)?;
        let body = build_register_client_body(password, prekeys, cookie_label, device_label);
        debug!(%url, "POST register client");
        let req = self.client.post(url).bearer_auth(access_token).json(&body);
        let registered = self
            .send_and_decode::<NewClientResponse>(req, "register_client")
            .await?;
        // Step 95c: the id becomes the device id — a path segment of
        // every client-scoped call and the rebind's query parameter.
        Self::path_segment(&registered.id, "register_client id")
            .map_err(|e| KupferdrahtError::Encoding(e.to_string()))?;
        Ok(registered)
    }

    /// `GET /v{api_version}/clients/{client_id}` — probe whether the
    /// device is still registered on Wire's side. Used by Step 7e's
    /// login-reuse path to validate a persisted `device_id` before
    /// rebinding the new access token to it.
    ///
    /// Returns:
    /// * `Ok(true)` — `200 OK`, the client exists.
    /// * `Ok(false)` — `404 Not Found`, the client was deleted (e.g.
    ///   the user removed it via the Wire UI, or the cap-7 sweep
    ///   evicted it). The caller falls through to a fresh
    ///   `POST /clients` registration.
    /// * `Err(BackendRejected)` for any other non-2xx — surfaced as
    ///   a hard failure so the login flow does not silently mint a
    ///   new device when the cause is something other than a clean
    ///   "device gone" signal (auth, rate-limit, server outage).
    ///
    /// # Errors
    /// * `BackendRejected` on non-200/404 statuses.
    /// * `Network` for transport failures.
    pub async fn client_exists(
        &self,
        api_version: u32,
        access_token: &str,
        client_id: &str,
    ) -> Result<bool, KupferdrahtError> {
        debug_assert!(!access_token.is_empty(), "access_token must be non-empty");
        debug_assert!(!client_id.is_empty(), "client_id must be non-empty");
        let path = format!("/v{api_version}/clients/{client_id}");
        let url = self.url(&path)?;
        debug!(%url, "GET client_exists");
        let resp = self
            .client
            .get(url)
            .bearer_auth(access_token)
            .send()
            .await
            .map_err(|e| KupferdrahtError::Network(format!("GET {path}: {e}")))?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(false);
        }
        if !resp.status().is_success() {
            return Err(reject_from_response(resp, "client_exists").await);
        }
        Ok(true)
    }

    /// `PUT /v{api_version}/clients/{client_id}` — register this device's
    /// MLS signature public key with the backend.
    ///
    /// Must be called **before** `POST /v14/mls/key-packages/self/{id}`:
    /// Wire validates every uploaded `KeyPackage` against the
    /// `(identity, signature_scheme)` pair registered for the client,
    /// and without a prior `PUT` the upload is rejected with
    /// `400 mls-protocol-error "No key associated to the given
    /// identity and signature scheme"` (observations.md §19).
    ///
    /// Auth is the rebound access token (`Bearer`).
    ///
    /// # Errors
    /// * `BackendRejected` on non-2xx statuses.
    /// * `Network` for transport failures.
    pub async fn update_client_mls_public_key(
        &self,
        api_version: u32,
        access_token: &str,
        client_id: &str,
        signature_scheme: &str,
        public_key_b64: &str,
    ) -> Result<(), KupferdrahtError> {
        debug_assert!(!access_token.is_empty(), "access_token must be non-empty");
        debug_assert!(!client_id.is_empty(), "client_id must be non-empty");
        debug_assert!(
            !public_key_b64.is_empty(),
            "public_key_b64 must be non-empty"
        );
        let path = format!("/v{api_version}/clients/{client_id}");
        let url = self.url(&path)?;
        let body = serde_json::json!({
            "mls_public_keys": {
                signature_scheme: public_key_b64,
            }
        });
        debug!(%url, "PUT client (mls_public_keys)");
        let req = self.client.put(url).bearer_auth(access_token).json(&body);
        self.send_no_body(req, "update_client_mls_public_key").await
    }

    /// `GET /v{api_version}/clients` — list every device currently
    /// registered for the authenticated user.
    ///
    /// Used by Step 1d's `Client::devices()`. Wire returns the full
    /// `Client` schema per device, but Kupferdraht only deserializes the
    /// fields exposed via `DeviceInfo` (`id`, `class`, `label`,
    /// `model`, `time`, `last_active`).
    ///
    /// # Errors
    /// * `BackendRejected` on non-2xx statuses.
    /// * `Network` for transport failures.
    /// * `Encoding` for malformed response bodies.
    pub async fn list_clients(
        &self,
        api_version: u32,
        access_token: &str,
    ) -> Result<Vec<WireClient>, KupferdrahtError> {
        debug_assert!(!access_token.is_empty(), "access_token must be non-empty");
        let path = format!("/v{api_version}/clients");
        let url = self.url(&path)?;
        debug!(%url, "GET list-clients");
        let req = self.client.get(url).bearer_auth(access_token);
        self.send_and_decode::<Vec<WireClient>>(req, "list_clients")
            .await
    }

    /// `DELETE /v{api_version}/clients/{client_id}` — revoke a
    /// registered device. Wire confirms the caller's identity via the
    /// user's password in the JSON body (the access token alone is not
    /// sufficient — Wire's threat model treats device revocation as an
    /// account-level action).
    ///
    /// `password` is `Option`: Wire does not require it for temporary
    /// clients, and password-less accounts confirm differently. When
    /// `None`, the field is omitted from the body.
    ///
    /// # Errors
    /// * `BackendRejected` on non-2xx statuses.
    /// * `Network` for transport failures.
    pub async fn delete_client(
        &self,
        api_version: u32,
        access_token: &str,
        client_id: &str,
        password: Option<&str>,
    ) -> Result<(), KupferdrahtError> {
        debug_assert!(!access_token.is_empty(), "access_token must be non-empty");
        debug_assert!(!client_id.is_empty(), "client_id must be non-empty");
        let path = format!("/v{api_version}/clients/{client_id}");
        let url = self.url(&path)?;
        let body = match password {
            Some(pw) => serde_json::json!({ "password": pw }),
            None => serde_json::json!({}),
        };
        debug!(%url, "DELETE client");
        let req = self
            .client
            .delete(url)
            .bearer_auth(access_token)
            .json(&body);
        self.send_no_body(req, "delete_client").await
    }

    /// `PUT /v{api_version}/clients/{client_id}` with a `label` body —
    /// renames a registered device. The label is the user-visible
    /// device name shown in Wire's settings UI.
    ///
    /// # Errors
    /// * `BackendRejected` on non-2xx statuses.
    /// * `Network` for transport failures.
    pub async fn update_client_label(
        &self,
        api_version: u32,
        access_token: &str,
        client_id: &str,
        label: &str,
    ) -> Result<(), KupferdrahtError> {
        debug_assert!(!access_token.is_empty(), "access_token must be non-empty");
        debug_assert!(!client_id.is_empty(), "client_id must be non-empty");
        debug_assert!(!label.is_empty(), "label must be non-empty");
        let path = format!("/v{api_version}/clients/{client_id}");
        let url = self.url(&path)?;
        let body = serde_json::json!({ "label": label });
        debug!(%url, "PUT client (label)");
        let req = self.client.put(url).bearer_auth(access_token).json(&body);
        self.send_no_body(req, "update_client_label").await
    }

    /// `PUT /v{api_version}/clients/{client_id}` with a `capabilities`
    /// body — declares this device's optional protocol capabilities
    /// to Wire.
    ///
    /// Wire **requires** `consumable-notifications` for v9+ devices
    /// to receive frames over `/v14/events` (live-confirmed —
    /// without it the WebSocket opens but Wire never
    /// sends an `event` frame). Kupferdraht's login pipeline calls this
    /// with `["consumable-notifications", "legalhold-implicit-consent"]`
    /// — both live-validated as the capability set v9+ devices
    /// must declare to bootstrap against a live Wire v14 backend.
    ///
    /// Idempotent — re-PUTting the same set is a no-op server-side,
    /// so `kupferdraht-cli sync-stream` can safely call this on every
    /// startup to remediate devices registered before this fix.
    ///
    /// # Errors
    /// * `BackendRejected` on non-2xx statuses.
    /// * `Network` for transport failures.
    pub async fn update_client_capabilities(
        &self,
        api_version: u32,
        access_token: &str,
        client_id: &str,
        capabilities: &[&str],
    ) -> Result<(), KupferdrahtError> {
        debug_assert!(!access_token.is_empty(), "access_token must be non-empty");
        debug_assert!(!client_id.is_empty(), "client_id must be non-empty");
        debug_assert!(!capabilities.is_empty(), "capabilities must be non-empty");
        let path = format!("/v{api_version}/clients/{client_id}");
        let url = self.url(&path)?;
        let body = serde_json::json!({ "capabilities": capabilities });
        debug!(%url, ?capabilities, "PUT client (capabilities)");
        let req = self.client.put(url).bearer_auth(access_token).json(&body);
        self.send_no_body(req, "update_client_capabilities").await
    }
}

fn encode_prekey(pk: &PreKey) -> WirePreKeyJson {
    WirePreKeyJson {
        id: u32::from(pk.id),
        key: base64::engine::general_purpose::STANDARD.encode(&pk.key),
    }
}

/// Build the typed body for `POST /v{N}/clients`. Keeps Wire's field
/// names (`type`, `class`, `cookie`, `lastkey`) since these are
/// externally constrained.
fn build_register_client_body<'a>(
    password: Option<&'a str>,
    prekeys: &PrekeyBundle,
    cookie_label: &'a str,
    device_label: Option<&'a str>,
) -> NewClientRequest<'a> {
    NewClientRequest {
        // Step 41: real device class + a human-readable model/label so two
        // devices of one account are distinguishable in Wire's device list
        // (the remove-a-device UX). `cookie` stays the unique per-install slot.
        class: device_class(),
        cookie: cookie_label,
        label: device_label.unwrap_or(device_model()),
        lastkey: encode_prekey(&prekeys.lastkey),
        model: device_model(),
        password,
        prekeys: prekeys.prekeys.iter().map(encode_prekey).collect(),
        client_type: "permanent",
    }
}

/// Step 41: Wire device class, derived from the build target.
fn device_class() -> &'static str {
    if cfg!(any(target_os = "ios", target_os = "android")) {
        "phone"
    } else {
        "desktop"
    }
}

/// The device `model` of a browser (ADR-0035): an account's device list
/// shows by it which devices a browser holds.
pub(crate) const WEB_DEVICE_MODEL: &str = "Kupferdraht (Web)";

/// Step 41: a platform model string, used as the device `model` and as the
/// fallback display `label` when the embedder supplies no device name.
fn device_model() -> &'static str {
    if cfg!(target_arch = "wasm32") {
        WEB_DEVICE_MODEL
    } else if cfg!(target_os = "ios") {
        "Kupferdraht (iOS)"
    } else if cfg!(target_os = "android") {
        "Kupferdraht (Android)"
    } else if cfg!(target_os = "macos") {
        "Kupferdraht (macOS)"
    } else {
        "Kupferdraht"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_bundle() -> PrekeyBundle {
        PrekeyBundle {
            prekeys: vec![
                PreKey {
                    id: 0,
                    key: vec![0xAB, 0xCD],
                },
                PreKey {
                    id: 1,
                    key: vec![0xEF],
                },
            ],
            lastkey: PreKey {
                id: 0xFFFF,
                key: vec![0x01, 0x02, 0x03],
            },
        }
    }

    #[test]
    fn build_register_client_body_has_wire_required_fields() {
        let bundle = sample_bundle();
        let req =
            build_register_client_body(Some("pw"), &bundle, "kupferdraht-install-1", Some("Pixel"));
        assert_eq!(req.password, Some("pw"));
        assert_eq!(req.client_type, "permanent");
        // Step 41: cookie = the unique per-install slot; label = the supplied
        // device name; class/model are platform-derived (desktop on the test host).
        assert_eq!(
            req.cookie, "kupferdraht-install-1",
            "cookie carries the per-install label"
        );
        assert_eq!(req.label, "Pixel", "label is the supplied device name");
        assert_eq!(req.class, device_class());
        assert_eq!(req.model, device_model());
        assert!(req.model.starts_with("Kupferdraht"));
        assert_eq!(req.prekeys.len(), 2);
        assert_eq!(req.lastkey.id, u32::from(0xFFFFu16));
    }

    #[test]
    fn build_register_client_body_label_falls_back_to_platform_model() {
        let bundle = sample_bundle();
        let req = build_register_client_body(Some("pw"), &bundle, "kupferdraht-install-1", None);
        assert_eq!(
            req.label,
            device_model(),
            "no device name supplied -> label is the platform model"
        );
    }

    #[test]
    fn build_register_client_body_prekeys_are_base64_encoded() {
        let bundle = sample_bundle();
        let req = build_register_client_body(Some("pw"), &bundle, "label", None);
        assert_eq!(
            req.prekeys[0].key, "q80=",
            "bytes 0xAB 0xCD must round-trip through base64"
        );
        assert_eq!(req.lastkey.key, "AQID", "bytes 0x01 0x02 0x03 → \"AQID\"");
    }

    #[test]
    fn build_register_client_body_omits_password_when_none() {
        let bundle = sample_bundle();
        let req = build_register_client_body(None, &bundle, "label", None);
        assert_eq!(req.password, None);
        let json = serde_json::to_value(&req).expect("serialize");
        assert!(
            json.get("password").is_none(),
            "a None password must be omitted from the body, not sent as null"
        );
    }

    // --- client_exists -----------------------------------------------

    fn http(base_url: &str) -> HttpClient {
        HttpClient::new(base_url, true).expect("HttpClient::new (permit_http=true)")
    }

    #[tokio::test]
    async fn client_exists_returns_true_on_200() {
        use httpmock::Method::GET;
        use httpmock::MockServer;
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET)
                .path("/v14/clients/abcd1234")
                .header("authorization", "Bearer tok");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"abcd1234"}"#);
        });
        assert!(
            http(&server.base_url())
                .client_exists(14, "tok", "abcd1234")
                .await
                .expect("200 must yield Ok(true)")
        );
    }

    #[tokio::test]
    async fn client_exists_returns_false_on_404() {
        use httpmock::Method::GET;
        use httpmock::MockServer;
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/v14/clients/missing");
            then.status(404)
                .header("content-type", "application/json")
                .body(r#"{"code":404,"label":"client-not-found","message":"gone"}"#);
        });
        assert!(
            !http(&server.base_url())
                .client_exists(14, "tok", "missing")
                .await
                .expect("404 must yield Ok(false)"),
            "404 must signal `device cleanly absent` so login can fall through"
        );
    }

    #[tokio::test]
    async fn client_exists_surfaces_other_non_2xx_as_backend_rejected() {
        use httpmock::Method::GET;
        use httpmock::MockServer;
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/v14/clients/anyid");
            then.status(401)
                .header("content-type", "application/json")
                .body(r#"{"code":401,"label":"invalid-token","message":"x"}"#);
        });
        let err = http(&server.base_url())
            .client_exists(14, "tok", "anyid")
            .await
            .expect_err("401 must propagate as BackendRejected");
        assert!(matches!(err, KupferdrahtError::BackendRejected(_)));
    }

    // --- token-refresh seam: 401 -> Unauthorized (TDD, red) ----------

    #[tokio::test]
    async fn send_and_decode_maps_401_to_unauthorized() {
        use httpmock::Method::GET;
        use httpmock::MockServer;
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/v14/clients");
            then.status(401)
                .header("content-type", "application/json")
                .body(r#"{"code":401,"label":"invalid-token","message":"x"}"#);
        });
        let err = http(&server.base_url())
            .list_clients(14, "tok")
            .await
            .expect_err("401 must surface as Unauthorized");
        assert!(
            matches!(err, KupferdrahtError::Unauthorized),
            "expected Unauthorized, got {err:?}"
        );
    }

    #[tokio::test]
    async fn send_no_body_maps_401_to_unauthorized() {
        use httpmock::Method::PUT;
        use httpmock::MockServer;
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(PUT).path("/v14/clients/cid");
            then.status(401);
        });
        let err = http(&server.base_url())
            .update_client_mls_public_key(14, "tok", "cid", "ecdsa_secp256r1_sha256", "a2V5")
            .await
            .expect_err("401 must surface as Unauthorized");
        assert!(
            matches!(err, KupferdrahtError::Unauthorized),
            "expected Unauthorized, got {err:?}"
        );
    }

    #[tokio::test]
    async fn send_and_decode_keeps_403_as_backend_rejected() {
        use httpmock::Method::GET;
        use httpmock::MockServer;
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/v14/clients");
            then.status(403)
                .header("content-type", "application/json")
                .body(r#"{"code":403,"label":"forbidden","message":"x"}"#);
        });
        let err = http(&server.base_url())
            .list_clients(14, "tok")
            .await
            .expect_err("403 must stay BackendRejected");
        assert!(
            matches!(err, KupferdrahtError::BackendRejected(_)),
            "expected BackendRejected, got {err:?}"
        );
    }

    // --- list_clients / delete_client / update_client_label ----------

    #[tokio::test]
    async fn list_clients_parses_array_response() {
        use httpmock::Method::GET;
        use httpmock::MockServer;
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET)
                .path("/v14/clients")
                .header("authorization", "Bearer tok");
            then.status(200)
                .header("content-type", "application/json")
                .body(
                    r#"[
                      {"id":"abcd1234","type":"permanent","class":"desktop",
                       "label":"kupferdraht-cli","model":"kupferdraht-cli",
                       "time":"2026-04-29T10:00:00Z"},
                      {"id":"deadbeef","type":"permanent","class":"phone",
                       "label":"iPhone","model":"iPhone16,1",
                       "time":"2026-04-20T08:00:00Z",
                       "last_active":"2026-04-29T11:30:00Z"}
                    ]"#,
                );
        });
        let clients = http(&server.base_url())
            .list_clients(14, "tok")
            .await
            .expect("list_clients must succeed");
        assert_eq!(clients.len(), 2);
        assert_eq!(clients[0].id, "abcd1234");
        assert_eq!(clients[0].class.as_deref(), Some("desktop"));
        assert_eq!(clients[1].label.as_deref(), Some("iPhone"));
        assert_eq!(
            clients[1].last_active.as_deref(),
            Some("2026-04-29T11:30:00Z")
        );
    }

    #[tokio::test]
    async fn list_clients_tolerates_minimal_fields() {
        use httpmock::Method::GET;
        use httpmock::MockServer;
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/v14/clients");
            // Only `id` is required per Wire's schema; the rest are
            // optional and our DTO must tolerate their absence.
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"[{"id":"minimal","type":"permanent","time":"2026-04-29T10:00:00Z"}]"#);
        });
        let clients = http(&server.base_url())
            .list_clients(14, "tok")
            .await
            .expect("minimal payload must parse");
        assert_eq!(clients.len(), 1);
        assert_eq!(clients[0].id, "minimal");
        assert!(clients[0].label.is_none());
        assert!(clients[0].class.is_none());
    }

    #[tokio::test]
    async fn delete_client_sends_password_in_body() {
        use httpmock::Method::DELETE;
        use httpmock::MockServer;
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(DELETE)
                .path("/v14/clients/abcd1234")
                .header("authorization", "Bearer tok")
                .is_true(|req| {
                    let bytes = req.body_ref();
                    let Ok(v): Result<serde_json::Value, _> = serde_json::from_slice(bytes) else {
                        return false;
                    };
                    v.get("password").and_then(|x| x.as_str()) == Some("hunter2")
                });
            then.status(200);
        });
        http(&server.base_url())
            .delete_client(14, "tok", "abcd1234", Some("hunter2"))
            .await
            .expect("delete must succeed");
        mock.assert_calls(1);
    }

    #[tokio::test]
    async fn delete_client_omits_password_when_none() {
        use httpmock::Method::DELETE;
        use httpmock::MockServer;
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(DELETE)
                .path("/v14/clients/abcd1234")
                .is_true(|req| {
                    let bytes = req.body_ref();
                    let Ok(v): Result<serde_json::Value, _> = serde_json::from_slice(bytes) else {
                        return false;
                    };
                    // No password field at all (not present, not null).
                    v.get("password").is_none()
                });
            then.status(200);
        });
        http(&server.base_url())
            .delete_client(14, "tok", "abcd1234", None)
            .await
            .expect("password-less delete must succeed");
        mock.assert_calls(1);
    }

    #[tokio::test]
    async fn delete_client_surfaces_403_as_backend_rejected() {
        use httpmock::Method::DELETE;
        use httpmock::MockServer;
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(DELETE).path("/v14/clients/abcd1234");
            then.status(403)
                .header("content-type", "application/json")
                .body(r#"{"code":403,"label":"invalid-credentials","message":"bad pw"}"#);
        });
        let err = http(&server.base_url())
            .delete_client(14, "tok", "abcd1234", Some("wrong"))
            .await
            .expect_err("wrong password must surface as BackendRejected");
        assert!(matches!(err, KupferdrahtError::BackendRejected(_)));
    }

    #[tokio::test]
    async fn update_client_label_sends_label_only() {
        use httpmock::Method::PUT;
        use httpmock::MockServer;
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(PUT)
                .path("/v14/clients/abcd1234")
                .is_true(|req| {
                    let bytes = req.body_ref();
                    let Ok(v): Result<serde_json::Value, _> = serde_json::from_slice(bytes) else {
                        return false;
                    };
                    // Body must contain `label` and nothing else from
                    // the UpdateClient schema (no capabilities, no
                    // mls_public_keys, no prekeys).
                    v.get("label").and_then(|x| x.as_str()) == Some("My MacBook")
                        && v.get("capabilities").is_none()
                        && v.get("mls_public_keys").is_none()
                        && v.get("prekeys").is_none()
                });
            then.status(200);
        });
        http(&server.base_url())
            .update_client_label(14, "tok", "abcd1234", "My MacBook")
            .await
            .expect("rename must succeed");
        mock.assert_calls(1);
    }
}
