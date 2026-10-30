// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Monotonic wall-clock helper used throughout `kupferdraht-core`.
//!
//! A single canonical source for "milliseconds since UNIX epoch" so
//! every module — and the `kupferdraht-cli` consumer — reads from the same
//! function instead of duplicating the `SystemTime` boilerplate.

#[cfg(not(target_arch = "wasm32"))]
use std::time::{SystemTime, UNIX_EPOCH};

/// Returns the current wall-clock time as milliseconds since the UNIX epoch.
///
/// Saturates at `i64::MAX` on overflow (not reachable in practice before
/// year 292 million). Returns `0` if the system clock is set before the
/// epoch, which should never happen on a real device.
#[cfg(not(target_arch = "wasm32"))]
#[must_use]
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// wasm: `SystemTime::now()` panics on `wasm32-unknown-unknown` (no platform
/// clock), so read the browser wall clock via `js_sys::Date::now()` — an
/// `f64` of milliseconds since the UNIX epoch — clamped into `i64` range.
///
/// The cast is bounds-checked by the preceding `clamp`, so truncation/precision
/// loss is intentional and harmless (ms resolution needs 44 bits; `f64`'s 52-bit
/// mantissa is exact here until year ~287396).
#[cfg(target_arch = "wasm32")]
#[must_use]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]
pub fn now_ms() -> i64 {
    js_sys::Date::now().clamp(0.0, i64::MAX as f64) as i64
}

/// Sleep for `duration` (W4). `tokio::time::sleep` needs the tokio timer driver,
/// which does not exist in the browser (no runtime) — there it hangs forever —
/// so on wasm this drives the browser's own timer via `gloo-timers`.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) async fn sleep(duration: std::time::Duration) {
    tokio::time::sleep(duration).await;
}

#[cfg(target_arch = "wasm32")]
#[allow(clippy::cast_possible_truncation)]
pub(crate) async fn sleep(duration: std::time::Duration) {
    // `gloo-timers` takes whole milliseconds as `u32`; saturate on overflow
    // (any real backoff is far below `u32::MAX` ms ≈ 49 days).
    let millis = u32::try_from(duration.as_millis()).unwrap_or(u32::MAX);
    gloo_timers::future::TimeoutFuture::new(millis).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_ms_is_positive() {
        assert!(now_ms() > 0);
    }

    #[test]
    fn now_ms_is_after_project_epoch() {
        // 2024-01-01 00:00:00 UTC in ms — any real clock is past this.
        assert!(now_ms() > 1_704_067_200_000);
    }

    #[test]
    fn now_ms_is_monotonic() {
        let a = now_ms();
        let b = now_ms();
        assert!(b >= a);
    }
}
