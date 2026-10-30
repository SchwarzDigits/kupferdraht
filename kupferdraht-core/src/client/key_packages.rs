// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Key-package pool replenishment (Step 63, `open.md` gap-B.7).
//!
//! Every peer that adds this device to a conversation *claims* one of
//! its uploaded key packages; the pool starts at
//! [`INITIAL_KEY_PACKAGE_COUNT`] (login upload) and only ever
//! shrinks. At zero the device can no longer be added to anything
//! new — while existing rooms keep working, which is why nobody
//! notices in time. This module adds the self-healing top-up: an
//! explicit [`Client::replenish_key_packages`] plus a 24-h-throttled
//! automatic check on the sync surface (`sync_once`, which every
//! polling loop variant converges on).

use tracing::{debug, info, warn};

use crate::client::Client;
use crate::encryption::INITIAL_KEY_PACKAGE_COUNT;
use crate::error::KupferdrahtError;
use crate::time::now_ms;

/// Top-up target — the pool size the login upload establishes.
const KEY_PACKAGE_TARGET: usize = INITIAL_KEY_PACKAGE_COUNT;
/// Below this the pool is topped back up to [`KEY_PACKAGE_TARGET`]
/// (the halving heuristic Wire clients use). `KEY_PACKAGE_MIN` itself
/// still counts as healthy.
const KEY_PACKAGE_MIN: usize = INITIAL_KEY_PACKAGE_COUNT / 2;
/// Throttle for the automatic sync-surface check.
const CHECK_INTERVAL_MS: i64 = 24 * 60 * 60 * 1000;

impl Client {
    /// Remaining unclaimed key packages for this device, straight from
    /// `GET /v{N}/mls/key-packages/self/{client}/count`.
    ///
    /// # Errors
    /// * `KupferdrahtError::InvalidInput` — no registered device.
    /// * `Network` / `BackendRejected` / `Encoding` from the call.
    pub async fn key_package_count(&self) -> Result<u64, KupferdrahtError> {
        let device_id = self.device_id().map(str::to_string).ok_or_else(|| {
            KupferdrahtError::InvalidInput("key_package_count: no registered device".into())
        })?;
        self.http
            .fetch_key_package_count(
                self.api_version,
                &self.current_access_token(),
                &device_id,
                u16::from(crate::encryption::MLS_CIPHERSUITE_FOR_CLAIM),
            )
            .await
    }

    /// Check the server-side key-package pool for this device and top
    /// it back up to `INITIAL_KEY_PACKAGE_COUNT` (100) when it has
    /// fallen below half of that. Returns the number of packages
    /// uploaded — `0` means the pool was healthy.
    ///
    /// Called automatically (at most once per 24 h) from the sync
    /// surface; call it directly to force a top-up (e.g.
    /// `kupferdraht-cli key-packages --replenish`).
    ///
    /// # Errors
    /// * `KupferdrahtError::InvalidInput` — no registered device.
    /// * `Network` / `BackendRejected` — count or upload call failed.
    /// * MLS errors from key-package generation.
    ///
    /// # Panics
    /// Panics when the computed top-up size leaves `(0, TARGET]` —
    /// arithmetically unreachable (`count < MIN < TARGET` at that
    /// point); the assertion pins the invariant.
    pub async fn replenish_key_packages(&self) -> Result<u32, KupferdrahtError> {
        let device_id = self.device_id().map(str::to_string).ok_or_else(|| {
            KupferdrahtError::InvalidInput("replenish_key_packages: no registered device".into())
        })?;
        let access_token = self.current_access_token();
        let count = self
            .http
            .fetch_key_package_count(
                self.api_version,
                &access_token,
                &device_id,
                u16::from(crate::encryption::MLS_CIPHERSUITE_FOR_CLAIM),
            )
            .await?;
        let count = usize::try_from(count).unwrap_or(usize::MAX);
        if count >= KEY_PACKAGE_MIN {
            debug!(count, "key-package pool healthy; no top-up");
            return Ok(0);
        }
        let needed = KEY_PACKAGE_TARGET - count;
        assert!(
            needed > 0 && needed <= KEY_PACKAGE_TARGET,
            "top-up size must stay within (0, TARGET]"
        );
        let packages = {
            let mls = self.mls_client_required("replenish_key_packages")?;
            let _gate = self.mls_write_gate().await;
            let guard = crate::encryption::lock_mls(mls);
            guard.generate_key_packages(needed)?
        };
        self.http
            .upload_key_packages(self.api_version, &access_token, &device_id, &packages)
            .await?;
        info!(uploaded = needed, "key-package pool replenished");
        Ok(u32::try_from(needed).unwrap_or(u32::MAX))
    }

    /// The throttled sync-surface hook: at most one
    /// [`Self::replenish_key_packages`] attempt per 24 h, stamped in
    /// `sync_state.last_key_package_check_at_ms` **before** the
    /// attempt so a persistently failing endpoint is not hammered
    /// every iteration. Never fails — a key-package problem must not
    /// break sync (warn-and-continue, ADR-0012 posture).
    pub(crate) async fn maybe_replenish_key_packages(&self) {
        let Some(store) = self.store_handle() else {
            return;
        };
        let state = match store.load_sync_state().await {
            Ok(state) => state.unwrap_or_default(),
            Err(e) => {
                warn!(error = %e, "key-package check: sync_state read failed; skipping");
                return;
            }
        };
        let now = now_ms();
        if state
            .last_key_package_check_at_ms
            .is_some_and(|last| now.saturating_sub(last) < CHECK_INTERVAL_MS)
        {
            return;
        }
        let mut stamped = state;
        stamped.last_key_package_check_at_ms = Some(now);
        if let Err(e) = store.save_sync_state(&stamped).await {
            warn!(error = %e, "key-package check: stamp write failed; skipping");
            return;
        }
        match self.replenish_key_packages().await {
            Ok(0) => {}
            Ok(uploaded) => info!(uploaded, "key-package pool auto-replenished"),
            Err(e) => {
                warn!(error = %e, "key-package replenishment failed; retrying in 24 h");
            }
        }
    }
}
