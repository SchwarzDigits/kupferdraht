// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

use crate::error::KupferdrahtError;

/// API versions Kupferdraht knows how to speak. Per ADR-0008 Kupferdraht targets
/// v14 directly — no v5 compatibility, no dual-version support.
pub const CLIENT_SUPPORTED: &[u32] = &[14];

/// Pick the highest API version supported by both client and server.
///
/// # Errors
/// `KupferdrahtError::Environment` if the server and client support sets
/// do not intersect.
pub fn select_api_version(server_supported: &[u32]) -> Result<u32, KupferdrahtError> {
    CLIENT_SUPPORTED
        .iter()
        .rev()
        .find(|v| server_supported.contains(v))
        .copied()
        .ok_or_else(|| {
            KupferdrahtError::Environment(format!(
                "no API version overlap: client={CLIENT_SUPPORTED:?} server={server_supported:?}"
            ))
        })
}
