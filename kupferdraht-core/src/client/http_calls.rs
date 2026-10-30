// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `Client` — crate-scoped Wire transport wrappers.
//!
//! The thin `self.http` forwarding slice of the `Client` runtime
//! (Step 29): crate-private helpers that wrap a single Wire endpoint
//! each (`http_send`, `http_list_clients`, `http_fetch_notifications`,
//! `http_upload_asset`, …) so the timeline / device / sync modules do
//! not reach into `self.http` / `self.api_version` directly.

#![allow(clippy::doc_markdown)]

use crate::error::KupferdrahtError;

use super::Client;

impl Client {
    /// Crate-scoped HTTP send for `POST /v14/mls/messages`. Wraps the
    /// transport call so the timeline module doesn't need direct
    /// access to `self.http` / `self.access_token`. A successful upload
    /// notes a sent message (Step 122 — the browser's sync loop drains on
    /// it, so `onSync` follows an own send).
    pub(crate) async fn http_send(
        &self,
        api_version: u32,
        ciphertext: &[u8],
    ) -> Result<(), KupferdrahtError> {
        self.http
            .send_mls_message(api_version, &self.current_access_token(), ciphertext)
            .await?;
        self.sent.note();
        Ok(())
    }

    /// Step 122: the signal of a successful message upload.
    #[cfg(any(target_arch = "wasm32", test))]
    pub(crate) fn sent_signal(&self) -> &super::sent_signal::SentSignal {
        &self.sent
    }

    /// Crate-scoped HTTP helper — `GET /v14/clients`. Used by Step 1d's
    /// `Client::devices()`. Wraps the transport call so the device
    /// module doesn't need direct access to `self.http` /
    /// `self.api_version`.
    pub(crate) async fn http_list_clients(
        &self,
        access_token: &str,
    ) -> Result<Vec<crate::wire::dto::WireClient>, KupferdrahtError> {
        self.http.list_clients(self.api_version, access_token).await
    }

    /// Crate-scoped HTTP helper — `DELETE /v14/clients/{id}`. Used by
    /// Step 1d's `Client::delete_device()`.
    pub(crate) async fn http_delete_client(
        &self,
        access_token: &str,
        client_id: &str,
        password: &str,
    ) -> Result<(), KupferdrahtError> {
        self.http
            .delete_client(self.api_version, access_token, client_id, Some(password))
            .await
    }

    /// Crate-scoped HTTP helper — `PUT /v14/clients/{id}` with a
    /// `label` body. Used by Step 1d's `Client::rename_device()`.
    pub(crate) async fn http_update_client_label(
        &self,
        access_token: &str,
        client_id: &str,
        label: &str,
    ) -> Result<(), KupferdrahtError> {
        self.http
            .update_client_label(self.api_version, access_token, client_id, label)
            .await
    }

    /// Crate-scoped REST drain — `GET /v14/notifications`. Used by
    /// `SyncService::drain_pending` for the pull-mode alternative
    /// to the long-lived WebSocket. Note: takes `access_token` from
    /// the call site so a future token-refresh path can hand in a
    /// fresh token without mutating `self.access_token`.
    pub(crate) async fn http_fetch_notifications(
        &self,
        api_version: u32,
        access_token: &str,
        device_id: &str,
        since: Option<&str>,
        size: u32,
    ) -> Result<crate::wire::http::notifications::FetchedNotifications, KupferdrahtError> {
        self.http
            .fetch_notifications(api_version, access_token, device_id, since, size)
            .await
    }

    /// Crate-scoped wrapper for `GET /v14/feature-configs` driven from
    /// `sync::dispatch::dispatch_feature_config_update`. Wire ships
    /// the `feature-config.update` event without the new value, so
    /// the dispatcher must re-fetch the whole-team response.
    pub(crate) async fn http_fetch_feature_configs(
        &self,
    ) -> Result<crate::wire::http::feature_configs::RawFeatureConfigs, KupferdrahtError> {
        self.http
            .fetch_feature_configs(self.api_version, &self.current_access_token())
            .await
    }

    /// Crate-scoped wrapper for `GET /v14/properties/{key}` driven
    /// from `sync::dispatch::dispatch_user_property_set`. Wire ships
    /// the `user.properties-set` event with the affected `key` only,
    /// so the dispatcher refetches the value to upsert.
    pub(crate) async fn http_fetch_property_value(
        &self,
        key: &str,
    ) -> Result<String, KupferdrahtError> {
        self.http
            .fetch_property_value(self.api_version, &self.current_access_token(), key)
            .await
    }

    /// wasm (W7): `POST /v14/assets` with the ciphertext held in memory.
    #[cfg(target_arch = "wasm32")]
    pub(crate) async fn http_upload_asset_bytes(
        &self,
        access_token: &str,
        conv_id: Option<(&str, &str)>,
        filename: Option<&str>,
        filetype: Option<&str>,
        ciphertext: &[u8],
        md5_b64: &str,
    ) -> Result<crate::wire::http::assets::AssetUploadResponse, KupferdrahtError> {
        self.http
            .upload_asset_bytes(
                self.api_version,
                access_token,
                conv_id,
                filename,
                filetype,
                ciphertext,
                md5_b64,
            )
            .await
    }

    /// wasm (W7): `GET /v14/assets/{domain}/{key}` buffered into memory.
    #[cfg(target_arch = "wasm32")]
    pub(crate) async fn http_download_asset_bytes(
        &self,
        access_token: &str,
        asset_domain: &str,
        asset_id: &str,
        asset_token: Option<&str>,
    ) -> Result<Vec<u8>, KupferdrahtError> {
        self.http
            .download_asset_bytes(
                self.api_version,
                access_token,
                asset_domain,
                asset_id,
                asset_token,
            )
            .await
    }

    /// Crate-scoped wrapper for `POST /v14/assets` driven from
    /// Step 11f's `timeline::asset_send` orchestrator. Streams the
    /// ciphertext tempfile as the multipart body's second part.
    pub(crate) async fn http_upload_asset(
        &self,
        access_token: &str,
        params: crate::wire::http::assets::AssetMultipart<'_>,
    ) -> Result<crate::wire::http::assets::AssetUploadResponse, KupferdrahtError> {
        self.http
            .upload_asset(self.api_version, access_token, params)
            .await
    }

    /// Crate-scoped wrapper for `GET /v14/assets/{domain}/{key}`
    /// driven from Step 11g's `AssetSummary::download` /
    /// `AssetSummary::save_to`. Streams the response body into the
    /// caller-supplied sink path.
    pub(crate) async fn http_download_asset(
        &self,
        access_token: &str,
        asset_domain: &str,
        asset_id: &str,
        asset_token: Option<&str>,
        sink_path: &std::path::Path,
    ) -> Result<u64, KupferdrahtError> {
        self.http
            .download_asset(
                self.api_version,
                access_token,
                asset_domain,
                asset_id,
                asset_token,
                sink_path,
            )
            .await
    }
}
