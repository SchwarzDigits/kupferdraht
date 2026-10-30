// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Wire `/v14/assets` HTTP layer (Step 11c).
//!
//! Three endpoints, all behind bearer-auth:
//!
//! - `POST /v{N}/assets` — upload encrypted asset. Body is
//!   `multipart/mixed; boundary=frontier` with two parts (JSON
//!   metadata + raw ciphertext). Manually-built, not via reqwest's
//!   `multipart` module — that emits `multipart/form-data` and
//!   Wire's backend rejects anything but `multipart/mixed` here.
//!   See `docs/wire/asset-protocol.md` §Upload request body.
//! - `GET /v{N}/assets/{domain}/{key}` — stream-download the
//!   encrypted blob. `Asset-Token: <token>` header for private
//!   (E2EE) assets.
//! - `DELETE /v{N}/assets/{domain}/{key}` — sender-driven removal.
//!
//! ADR-0011 §Streaming model pins file-streaming for both upload
//! and download — Kupferdraht handles 100 MB+ assets without holding
//! plaintext in memory. Memory is constant per-buffer regardless
//! of asset size.
//!
//! The metadata JSON uses the v12+ shape (the audit-log
//! extensions): `convId`, `filename`, `filetype`. Wire API v14
//! inherits the v12 metadata shape — the upload metadata field
//! set is monotonic across v12-v14.

#![cfg_attr(not(test), allow(dead_code))]

use std::path::Path;

// File-streamed asset upload/download is native-only (tokio::fs + reqwest
// streaming bodies). The browser asset path (OPFS/Blob, W7) is a later step.
#[cfg(not(target_arch = "wasm32"))]
use futures_util::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};
#[cfg(not(target_arch = "wasm32"))]
use tokio::fs::File;
#[cfg(not(target_arch = "wasm32"))]
use tokio_util::bytes::Bytes;
#[cfg(not(target_arch = "wasm32"))]
use tokio_util::io::ReaderStream;
use tracing::debug;

use crate::error::KupferdrahtError;

use super::HttpClient;
use super::reject_from_response;

/// Multipart boundary string. Wire's `POST /assets` documentation
/// shows `frontier` in its example body (`docs/wire/asset-protocol.md`
/// §Upload request body); Kupferdraht emits that value verbatim so its
/// uploads have exactly the documented shape.
const MULTIPART_BOUNDARY: &str = "frontier";

/// Response of `POST /v{N}/assets`. Wire returns the asset triple
/// the sender embeds inside `Asset.RemoteData`.
#[derive(Deserialize)]
pub struct AssetUploadResponse {
    pub key: String,
    pub token: Option<String>,
    pub domain: Option<String>,
    /// Optional `expires` timestamp for `retention=expiring` uploads.
    /// Kupferdraht always uses `retention=persistent`, so this is
    /// effectively never populated; kept for forward-compat.
    #[serde(default)]
    #[allow(dead_code)]
    pub expires: Option<String>,
}

impl std::fmt::Debug for AssetUploadResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Step 101: secrets redacted (`crate::redact`).
        f.debug_struct("AssetUploadResponse")
            .field("key", &self.key)
            .field("token", &crate::redact::hidden_option(self.token.as_ref()))
            .field("domain", &self.domain)
            .field("expires", &self.expires)
            .finish()
    }
}

/// JSON metadata for the first part of the multipart upload body.
///
/// **Source:** `reference/wire.swagger.json#/paths/~1assets/post`.
/// Wire's swagger references `AssetSource` / `AssetSettings` by
/// name but leaves both entries in `components/schemas/` empty,
/// so the operation's description (and its example body) is the
/// authoritative spec.
///
/// **Field set (5 fields, all Kupferdraht uses):**
/// - `public`, `retention` — optional per spec; Kupferdraht always
///   sends them explicitly with `public=false`,
///   `retention=persistent` (canonical E2EE upload posture,
///   ADR-0011).
/// - `convId`, `filename`, `filetype` — required when asset
///   audit logging is enabled on the team; Kupferdraht ships them on
///   every upload so the same code path works regardless of
///   the team's audit-log setting.
///
/// **Field order matches Wire's example body verbatim**
/// (`public`, `retention`, `convId`, `filename`, `filetype`).
/// JSON object key order is wire-cosmetic, but matching the
/// example keeps live request captures readable side-by-side
/// with Wire's documented shape. `convId` is camelCase on the
/// wire; `serde(rename)` bridges to Rust's `conv_id`.
#[derive(Debug, Serialize)]
struct UploadMetadata<'a> {
    public: bool,
    retention: &'a str,
    #[serde(skip_serializing_if = "Option::is_none", rename = "convId")]
    conv_id: Option<ConvIdRef<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    filename: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    filetype: Option<&'a str>,
}

#[derive(Debug, Serialize)]
struct ConvIdRef<'a> {
    id: &'a str,
    domain: &'a str,
}

/// Convenience input bundle for [`HttpClient::upload_asset`]. Keeps
/// the call site readable — the underlying multipart body has 7
/// caller-supplied bits and a positional list would be unreadable.
#[derive(Debug, Clone, Copy)]
pub struct AssetMultipart<'a> {
    /// Wire conversation reference for the v12+ audit-log fields.
    /// `None` produces an upload without `convId` (Wire's old shape).
    pub conv_id: Option<(&'a str, &'a str)>,
    /// Display filename. Stored alongside the asset for UI purposes;
    /// Wire's web client uses this for the download-link `filename`
    /// query parameter.
    pub filename: Option<&'a str>,
    /// Mime type — same string Kupferdraht puts in `Asset.Original.mime_type`.
    pub filetype: Option<&'a str>,
    /// Path to the encrypted ciphertext blob (`iv(16) || aes-cbc(...)`).
    pub ciphertext_path: &'a Path,
    /// Length in bytes of the ciphertext blob — needed for
    /// Content-Length and the JSON metadata's `Content-Length`
    /// header on the second part. Caller has it from the
    /// `EncryptedAsset.ciphertext_size` field.
    pub ciphertext_size: u64,
    /// Base64-encoded MD5 over the ciphertext bytes. Wire's nginx
    /// validates this transport-level header on the second part.
    /// Caller computes it once over the same tempfile pass that
    /// produced the SHA-256.
    pub md5_b64: &'a str,
}

impl HttpClient {
    /// `POST /v{api_version}/assets` — upload an encrypted asset
    /// blob. Streams the ciphertext file as the second multipart
    /// part; the JSON metadata is built in memory (small).
    ///
    /// Always sends `public=false`, `retention=persistent` — the
    /// canonical E2EE upload posture (ADR-0011): the blob is opaque
    /// ciphertext, so neither public availability nor expiry make
    /// sense. `retention=expiring` and `public=true` paths are
    /// out of scope.
    ///
    /// # Errors
    /// * `BackendRejected` on non-2xx (`413` for assets exceeding
    ///   Wire's per-message size limit, `403 not-allowed-by-team-feature-config`
    ///   when the team policy disables file sharing — Kupferdraht's
    ///   pre-flight gate at `Timeline::send_file` should have
    ///   caught the latter).
    /// * `Network` for transport failures.
    /// * `Storage` for filesystem read errors during streaming.
    /// * `Encoding` for malformed response bodies.
    // wasm uses `upload_asset_bytes` instead; this path-based signature is kept
    // only so the shared `http_upload_asset` wrapper compiles. Stub → no await.
    #[cfg(target_arch = "wasm32")]
    #[allow(clippy::unused_async, clippy::unused_async_trait_impl)]
    pub async fn upload_asset(
        &self,
        api_version: u32,
        access_token: &str,
        params: AssetMultipart<'_>,
    ) -> Result<AssetUploadResponse, KupferdrahtError> {
        let _ = (api_version, access_token, params);
        Err(KupferdrahtError::Generic(
            "asset upload is not available on wasm yet".into(),
        ))
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub async fn upload_asset(
        &self,
        api_version: u32,
        access_token: &str,
        params: AssetMultipart<'_>,
    ) -> Result<AssetUploadResponse, KupferdrahtError> {
        debug_assert!(!access_token.is_empty(), "access_token must be non-empty");
        let path = format!("/v{api_version}/assets");
        let url = self.url(&path)?;
        debug!(%url, "POST asset upload");

        let metadata = UploadMetadata {
            public: false,
            retention: "persistent",
            conv_id: params.conv_id.map(|(id, domain)| ConvIdRef { id, domain }),
            filename: params.filename,
            filetype: params.filetype,
        };
        let metadata_json = serde_json::to_string(&metadata)
            .map_err(|e| KupferdrahtError::Encoding(format!("multipart metadata json: {e}")))?;

        let header_part = format!(
            "--{MULTIPART_BOUNDARY}\r\n\
             Content-Type: application/json;charset=utf-8\r\n\
             Content-Length: {}\r\n\
             \r\n\
             {metadata_json}\r\n\
             --{MULTIPART_BOUNDARY}\r\n\
             Content-Type: application/octet-stream\r\n\
             Content-Length: {ct_len}\r\n\
             Content-MD5: {md5}\r\n\
             \r\n",
            metadata_json.len(),
            ct_len = params.ciphertext_size,
            md5 = params.md5_b64,
        );
        let header_bytes = Bytes::from(header_part.into_bytes());
        let trailer_bytes = Bytes::from_static(concat!("\r\n--", "frontier", "--\r\n").as_bytes());
        let total_len = u64::try_from(header_bytes.len()).unwrap_or(u64::MAX)
            + params.ciphertext_size
            + u64::try_from(trailer_bytes.len()).unwrap_or(u64::MAX);

        let file = File::open(params.ciphertext_path).await.map_err(|e| {
            KupferdrahtError::Storage(format!(
                "open ciphertext {}: {e}",
                params.ciphertext_path.display()
            ))
        })?;
        let file_stream = ReaderStream::new(file);

        let header_stream = stream::iter(vec![Ok::<_, std::io::Error>(header_bytes)]);
        let trailer_stream = stream::iter(vec![Ok::<_, std::io::Error>(trailer_bytes)]);
        let body_stream = header_stream.chain(file_stream).chain(trailer_stream);
        let body = reqwest::Body::wrap_stream(body_stream);

        // Step 98: the transfer client — no total deadline for an upload.
        let req = self
            .transfer
            .post(url)
            .bearer_auth(access_token)
            .header(
                "Content-Type",
                format!("multipart/mixed; boundary={MULTIPART_BOUNDARY}"),
            )
            .header("Content-Length", total_len.to_string())
            .body(body);
        self.send_and_decode::<AssetUploadResponse>(req, "upload_asset")
            .await
    }

    /// `GET /v{api_version}/assets/{asset_domain}/{asset_id}` —
    /// stream the encrypted asset to `sink_path`. Adds
    /// `Asset-Token: <token>` when supplied (private E2EE assets).
    ///
    /// Returns the byte count actually written to `sink_path`.
    /// Caller is responsible for verifying SHA-256 over the file
    /// before decrypting (see [`crate::encryption::asset_crypto`]).
    ///
    /// # Errors
    /// * `BackendRejected` on non-2xx (`404 not-found` when the
    ///   asset has been deleted, `403` for token mismatch).
    /// * `InvalidInput` when `asset_domain` or `asset_id` is not a single
    ///   URL path segment (they are the sender's `RemoteData`, Step 95c).
    /// * `Network` for transport failures.
    /// * `Storage` for filesystem write errors during streaming.
    // wasm uses `download_asset_bytes` instead; this path-based signature is kept
    // only so the shared `http_download_asset` wrapper compiles. Stub → no await.
    #[cfg(target_arch = "wasm32")]
    #[allow(clippy::unused_async, clippy::unused_async_trait_impl)]
    pub async fn download_asset(
        &self,
        api_version: u32,
        access_token: &str,
        asset_domain: &str,
        asset_id: &str,
        asset_token: Option<&str>,
        sink_path: &Path,
    ) -> Result<u64, KupferdrahtError> {
        let _ = (
            api_version,
            access_token,
            asset_domain,
            asset_id,
            asset_token,
            sink_path,
        );
        Err(KupferdrahtError::Generic(
            "asset download is not available on wasm yet".into(),
        ))
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub async fn download_asset(
        &self,
        api_version: u32,
        access_token: &str,
        asset_domain: &str,
        asset_id: &str,
        asset_token: Option<&str>,
        sink_path: &Path,
    ) -> Result<u64, KupferdrahtError> {
        debug_assert!(!access_token.is_empty(), "access_token must be non-empty");
        let asset_domain = Self::path_segment(asset_domain, "download_asset domain")?;
        let asset_id = Self::path_segment(asset_id, "download_asset id")?;
        let path = format!("/v{api_version}/assets/{asset_domain}/{asset_id}");
        let url = self.url(&path)?;
        debug!(%url, "GET asset download");
        // Step 98: the transfer client — no total deadline; the download
        // fails after `transfer_idle` without a byte instead.
        let mut request = self.transfer.get(url).bearer_auth(access_token);
        if let Some(token) = asset_token {
            request = request.header("Asset-Token", token);
        }
        let resp = tokio::time::timeout(self.transfer_idle, request.send())
            .await
            .map_err(|_| stalled_download(self.transfer_idle))?
            .map_err(|e| KupferdrahtError::Network(format!("GET {path}: {e}")))?;
        if !resp.status().is_success() {
            return Err(reject_from_response(resp, "download_asset").await);
        }
        stream_response_to_path(resp, sink_path, self.transfer_idle).await
    }

    /// `DELETE /v{api_version}/assets/{asset_domain}/{asset_id}` —
    /// sender-driven asset removal. The HTTP surface ships; `Timeline`
    /// and the CLI do not expose it yet (Step 84).
    ///
    /// # Errors
    /// `BackendRejected` on non-2xx; `Network` for transport.
    pub async fn delete_asset(
        &self,
        api_version: u32,
        access_token: &str,
        asset_domain: &str,
        asset_id: &str,
    ) -> Result<(), KupferdrahtError> {
        debug_assert!(!access_token.is_empty(), "access_token must be non-empty");
        debug_assert!(!asset_domain.is_empty(), "asset_domain must be non-empty");
        debug_assert!(!asset_id.is_empty(), "asset_id must be non-empty");
        let path = format!("/v{api_version}/assets/{asset_domain}/{asset_id}");
        let url = self.url(&path)?;
        debug!(%url, "DELETE asset");
        let req = self.client.delete(url).bearer_auth(access_token);
        self.send_no_body(req, "delete_asset").await
    }
}

// ── wasm: in-memory (buffer) asset transfer (W7) ────────────────────────────
//
// The browser has no filesystem + no reqwest `stream`/`multipart` features, so
// the ciphertext is held in a `Vec<u8>`: the multipart/mixed body is assembled
// as one buffer and POSTed via `.body(Vec<u8>)` (core reqwest, available on the
// fetch backend); downloads buffer the whole body via `resp.bytes()`.
#[cfg(target_arch = "wasm32")]
impl HttpClient {
    /// `POST /v{api_version}/assets` with the ciphertext held in memory (W7).
    /// Same multipart/mixed body as the native streaming path, assembled as a
    /// single `Vec<u8>`.
    ///
    /// # Errors
    /// `BackendRejected`/`Network`/`Encoding` as for the native `upload_asset`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn upload_asset_bytes(
        &self,
        api_version: u32,
        access_token: &str,
        conv_id: Option<(&str, &str)>,
        filename: Option<&str>,
        filetype: Option<&str>,
        ciphertext: &[u8],
        md5_b64: &str,
    ) -> Result<AssetUploadResponse, KupferdrahtError> {
        let path = format!("/v{api_version}/assets");
        let url = self.url(&path)?;
        let metadata = UploadMetadata {
            public: false,
            retention: "persistent",
            conv_id: conv_id.map(|(id, domain)| ConvIdRef { id, domain }),
            filename,
            filetype,
        };
        let metadata_json = serde_json::to_string(&metadata)
            .map_err(|e| KupferdrahtError::Encoding(format!("multipart metadata json: {e}")))?;
        let header_part = format!(
            "--{MULTIPART_BOUNDARY}\r\n\
             Content-Type: application/json;charset=utf-8\r\n\
             Content-Length: {}\r\n\
             \r\n\
             {metadata_json}\r\n\
             --{MULTIPART_BOUNDARY}\r\n\
             Content-Type: application/octet-stream\r\n\
             Content-Length: {ct_len}\r\n\
             Content-MD5: {md5}\r\n\
             \r\n",
            metadata_json.len(),
            ct_len = ciphertext.len(),
            md5 = md5_b64,
        );
        let trailer = format!("\r\n--{MULTIPART_BOUNDARY}--\r\n");
        let mut body: Vec<u8> =
            Vec::with_capacity(header_part.len() + ciphertext.len() + trailer.len());
        body.extend_from_slice(header_part.as_bytes());
        body.extend_from_slice(ciphertext);
        body.extend_from_slice(trailer.as_bytes());

        let req = self
            .client
            .post(url)
            .bearer_auth(access_token)
            .header(
                "Content-Type",
                format!("multipart/mixed; boundary={MULTIPART_BOUNDARY}"),
            )
            .header("Content-Length", body.len().to_string())
            .body(body);
        self.send_and_decode::<AssetUploadResponse>(req, "upload_asset_bytes")
            .await
    }

    /// `GET /v{api_version}/assets/{domain}/{id}` buffering the whole encrypted
    /// blob into memory (W7).
    ///
    /// # Errors
    /// `InvalidInput`/`BackendRejected`/`Network` as for the native
    /// `download_asset`.
    pub(crate) async fn download_asset_bytes(
        &self,
        api_version: u32,
        access_token: &str,
        asset_domain: &str,
        asset_id: &str,
        asset_token: Option<&str>,
    ) -> Result<Vec<u8>, KupferdrahtError> {
        let asset_domain = Self::path_segment(asset_domain, "download_asset_bytes domain")?;
        let asset_id = Self::path_segment(asset_id, "download_asset_bytes id")?;
        let path = format!("/v{api_version}/assets/{asset_domain}/{asset_id}");
        let url = self.url(&path)?;
        let mut request = self.client.get(url).bearer_auth(access_token);
        if let Some(token) = asset_token {
            request = request.header("Asset-Token", token);
        }
        let resp = request
            .send()
            .await
            .map_err(|e| KupferdrahtError::Network(format!("GET {path}: {e}")))?;
        if !resp.status().is_success() {
            return Err(reject_from_response(resp, "download_asset_bytes").await);
        }
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| KupferdrahtError::Network(format!("download_asset body: {e}")))?;
        Ok(bytes.to_vec())
    }
}

/// A download that went silent for `idle` (Step 98).
#[cfg(not(target_arch = "wasm32"))]
fn stalled_download(idle: std::time::Duration) -> KupferdrahtError {
    KupferdrahtError::Network(format!("download_asset stalled: no byte for {idle:?}"))
}

/// Stream a successful download response body to `sink_path`. Uses
/// `bytes_stream` so memory profile stays constant regardless of
/// asset size; each chunk must arrive within `idle` of the previous
/// one (Step 98). Returns the byte count written.
#[cfg(not(target_arch = "wasm32"))]
async fn stream_response_to_path(
    resp: reqwest::Response,
    sink_path: &Path,
    idle: std::time::Duration,
) -> Result<u64, KupferdrahtError> {
    use tokio::io::AsyncWriteExt as _;
    let mut sink = File::create(sink_path)
        .await
        .map_err(|e| KupferdrahtError::Storage(format!("create {}: {e}", sink_path.display())))?;
    let mut written: u64 = 0;
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = tokio::time::timeout(idle, stream.next())
        .await
        .map_err(|_| stalled_download(idle))?
    {
        let bytes =
            chunk.map_err(|e| KupferdrahtError::Network(format!("download_asset stream: {e}")))?;
        sink.write_all(&bytes)
            .await
            .map_err(|e| KupferdrahtError::Storage(format!("write asset chunk: {e}")))?;
        written += u64::try_from(bytes.len()).unwrap_or(0);
    }
    sink.flush()
        .await
        .map_err(|e| KupferdrahtError::Storage(format!("flush asset sink: {e}")))?;
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::Method::{DELETE, GET, POST};
    use httpmock::MockServer;
    use std::io::Write as _;
    use tempfile::TempDir;

    fn http(base_url: &str) -> HttpClient {
        HttpClient::new(base_url, true).expect("HttpClient::new (permit_http=true)")
    }

    fn write_ct(dir: &TempDir, name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path = dir.path().join(name);
        let mut f = std::fs::File::create(&path).expect("create");
        f.write_all(bytes).expect("write");
        path
    }

    #[tokio::test]
    async fn upload_asset_posts_multipart_mixed_with_metadata_and_ciphertext() {
        let server = MockServer::start();
        let dir = TempDir::new().expect("tempdir");
        let ct = write_ct(&dir, "asset.enc", b"encrypted-bytes-here");
        let upload_mock = server.mock(|when, then| {
            when.method(POST)
                .path("/v14/assets")
                .header("authorization", "Bearer tok")
                .header_exists("content-type")
                .body_includes("--frontier")
                .body_includes(r#""public":false"#)
                .body_includes(r#""retention":"persistent""#)
                .body_includes(r#""filename":"photo.png""#)
                .body_includes(r#""filetype":"image/png""#)
                .body_includes("encrypted-bytes-here");
            then.status(201)
                .header("content-type", "application/json")
                .body(r#"{"key":"key-1","token":"tok-1","domain":"prod.wire.com"}"#);
        });
        let resp = http(&server.base_url())
            .upload_asset(
                14,
                "tok",
                AssetMultipart {
                    conv_id: Some(("conv-uuid", "wire.com")),
                    filename: Some("photo.png"),
                    filetype: Some("image/png"),
                    ciphertext_path: &ct,
                    ciphertext_size: 20,
                    md5_b64: "ZmFrZS1tZDU=",
                },
            )
            .await
            .expect("upload_asset");
        assert_eq!(resp.key, "key-1");
        assert_eq!(resp.token.as_deref(), Some("tok-1"));
        assert_eq!(resp.domain.as_deref(), Some("prod.wire.com"));
        upload_mock.assert();
    }

    #[tokio::test]
    async fn upload_asset_emits_content_md5_header_in_multipart_body() {
        let server = MockServer::start();
        let dir = TempDir::new().expect("tempdir");
        let ct = write_ct(&dir, "asset.enc", b"x");
        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/v14/assets")
                // Exact MD5 string must appear in the body's
                // second-part header.
                .body_includes("Content-MD5: AbCdEfG=");
            then.status(201)
                .header("content-type", "application/json")
                .body(r#"{"key":"k","token":null,"domain":null}"#);
        });
        let _ = http(&server.base_url())
            .upload_asset(
                14,
                "tok",
                AssetMultipart {
                    conv_id: None,
                    filename: None,
                    filetype: Some("application/octet-stream"),
                    ciphertext_path: &ct,
                    ciphertext_size: 1,
                    md5_b64: "AbCdEfG=",
                },
            )
            .await
            .expect("upload_asset");
        mock.assert();
    }

    #[tokio::test]
    async fn upload_asset_413_surfaces_as_backend_rejected() {
        let server = MockServer::start();
        let dir = TempDir::new().expect("tempdir");
        let ct = write_ct(&dir, "big.enc", b"X");
        server.mock(|when, then| {
            when.method(POST).path("/v14/assets");
            then.status(413)
                .body(r#"{"code":413,"label":"client-error"}"#);
        });
        let err = http(&server.base_url())
            .upload_asset(
                14,
                "tok",
                AssetMultipart {
                    conv_id: None,
                    filename: None,
                    filetype: None,
                    ciphertext_path: &ct,
                    ciphertext_size: 1,
                    md5_b64: "abc=",
                },
            )
            .await
            .expect_err("413 must surface as error");
        assert!(
            matches!(err, KupferdrahtError::BackendRejected(_)),
            "got {err:?}"
        );
    }

    /// Step 95c: `asset_domain` / `asset_id` are the sender's
    /// `Asset.RemoteData` fields. An empty one used to hit a
    /// `debug_assert!`; one carrying `/`, a dot segment (`..`, `%2e%2e`)
    /// or `?` / `#` steered the bearer-authenticated GET to another
    /// backend path. Each is refused before any request leaves (the
    /// wasm `download_asset_bytes` shares the same guard).
    #[tokio::test]
    async fn hostile_asset_path_segments_are_refused_before_any_request() {
        let server = MockServer::start();
        let any_get = server.mock(|when, then| {
            when.method(GET);
            then.status(200).body("not-an-asset");
        });
        let dir = TempDir::new().expect("tempdir");
        let sink = dir.path().join("hostile.enc");
        let client = http(&server.base_url());
        for (domain, id) in [
            ("", "key-1"),
            ("wire.com", ""),
            ("wire.com", "../../self"),
            ("..", "self"),
            ("wire.com", "%2e%2e"),
            ("wire.com", "a/b"),
            ("wire.com", "key?x=1"),
            ("wire.com", "key#frag"),
        ] {
            let err = client
                .download_asset(14, "tok", domain, id, None, &sink)
                .await
                .expect_err("a hostile segment must be refused");
            assert!(
                matches!(err, KupferdrahtError::InvalidInput(_)),
                "download_asset({domain:?}, {id:?}): {err:?}"
            );
        }
        any_get.assert_calls(0);
    }

    #[tokio::test]
    async fn download_asset_streams_body_to_sink_path() {
        let server = MockServer::start();
        let payload = b"streamed-encrypted-bytes-up-to-megabytes-in-prod";
        let mock = server.mock(|when, then| {
            when.method(GET)
                .path("/v14/assets/wire.com/key-1")
                .header("authorization", "Bearer tok")
                .header("asset-token", "private-tok");
            then.status(200)
                .header("content-type", "application/octet-stream")
                .body(payload);
        });
        let dir = TempDir::new().expect("tempdir");
        let sink = dir.path().join("downloaded.enc");
        let n = http(&server.base_url())
            .download_asset(14, "tok", "wire.com", "key-1", Some("private-tok"), &sink)
            .await
            .expect("download_asset");
        assert_eq!(n, payload.len() as u64);
        let from_disk = std::fs::read(&sink).expect("read sink");
        assert_eq!(from_disk, payload);
        mock.assert();
    }

    #[tokio::test]
    async fn download_asset_without_token_omits_asset_token_header() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(GET)
                .path("/v14/assets/assets.example.org/key-2")
                .is_true(|r| !r.headers().contains_key("asset-token"));
            then.status(200).body("public-asset");
        });
        let dir = TempDir::new().expect("tempdir");
        let sink = dir.path().join("public.enc");
        http(&server.base_url())
            .download_asset(14, "tok", "assets.example.org", "key-2", None, &sink)
            .await
            .expect("download_asset");
        mock.assert();
    }

    #[tokio::test]
    async fn download_asset_404_surfaces_as_backend_rejected() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/v14/assets/wire.com/missing");
            then.status(404).body(r#"{"code":404,"label":"not-found"}"#);
        });
        let dir = TempDir::new().expect("tempdir");
        let sink = dir.path().join("nope.enc");
        let err = http(&server.base_url())
            .download_asset(14, "tok", "wire.com", "missing", None, &sink)
            .await
            .expect_err("404 must surface as error");
        assert!(
            matches!(err, KupferdrahtError::BackendRejected(_)),
            "got {err:?}"
        );
    }

    #[tokio::test]
    async fn delete_asset_issues_delete_on_canonical_path() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(DELETE)
                .path("/v14/assets/wire.com/key-3")
                .header("authorization", "Bearer tok");
            then.status(200);
        });
        http(&server.base_url())
            .delete_asset(14, "tok", "wire.com", "key-3")
            .await
            .expect("delete_asset");
        mock.assert();
    }
}
