// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `Client` — @-mention resolution for outbound message bodies.
//!
//! Split out of [`super`] in Step 29: the `@<handle>` scanner
//! ([`scan_at_handles`]) and the directory-resolution methods
//! ([`Client::resolve_mentions_in_body`], `resolve_qualified_handles`)
//! that turn handles into qualified user-ids with UTF-16 offsets for
//! Wire's `Mention { start, length }`. Step 6-II-g.

#![allow(clippy::doc_markdown)]

use std::collections::HashMap;

use crate::error::KupferdrahtError;

use super::Client;

impl Client {
    /// Scan `body` for `@<handle>` tokens, resolve each one to a
    /// qualified user-id via `POST /v14/list-users`, and return the
    /// list of [`crate::timeline::Mention`] entries with UTF-16
    /// offsets into `body`. Handles that don't resolve are left
    /// alone (no mention emitted, body untouched). Step 6-II-g.
    ///
    /// Token rule: `@` followed by one or more characters from
    /// `[A-Za-z0-9_.-]` (Wire's handle alphabet, per the
    /// 400 invalid-handle live test). The token boundary on the
    /// right is the first non-handle character; on the left, the
    /// `@` must be either at start-of-body or preceded by
    /// whitespace.
    ///
    /// **Silent-no-op modes** (caller sees an empty `Vec`, no
    /// `Err`):
    /// - The body contains no `@`-tokens.
    /// - The account has no `domain()` on file (defensive corner
    ///   case — corrupted / metadata-only resume DB). Rationale +
    ///   warn-log in [`Self::resolve_qualified_handles`].
    /// - One or more handles do not resolve via Wire's directory.
    ///   Resolved ones are returned; unresolved ones are skipped.
    ///
    /// # Errors
    /// Network / backend errors from `list-users` propagate.
    pub async fn resolve_mentions_in_body(
        &self,
        body: &str,
    ) -> Result<Vec<crate::timeline::Mention>, KupferdrahtError> {
        let candidates = scan_at_handles(body);
        if candidates.is_empty() {
            return Ok(Vec::new());
        }
        let handles: Vec<String> = candidates.iter().map(|c| c.handle.clone()).collect();
        let qualified = self.resolve_qualified_handles(&handles).await?;
        let mut out = Vec::new();
        for c in candidates {
            if let Some(user_id) = qualified.get(&c.handle) {
                out.push(crate::timeline::Mention {
                    user_id: user_id.clone(),
                    start: c.utf16_start,
                    length: c.utf16_length,
                });
            }
        }
        Ok(out)
    }

    /// Crate-private: resolve a list of handles to qualified user
    /// ids via `POST /v14/list-users` (qualified-handles shape).
    /// Returns a `HashMap<handle_lowercase, qualified_user_id>`;
    /// handles Wire couldn't resolve are absent from the map.
    ///
    /// The account's own domain — carried in [`UserId`], so always
    /// present since Step 19 — qualifies the handles for Wire's
    /// `qualified_handles` query.
    async fn resolve_qualified_handles(
        &self,
        handles: &[String],
    ) -> Result<HashMap<String, String>, KupferdrahtError> {
        // Wire's list-users accepts qualified_handles; we need
        // to know which Wire backend domain to qualify against.
        // Use the caller's own domain — on a single-backend
        // deployment every handle lives on the user's home backend.
        let home_domain = self.account.domain();
        let qualified_handles: Vec<crate::wire::dto::QualifiedHandle> = handles
            .iter()
            .map(|h| crate::wire::dto::QualifiedHandle {
                handle: h.clone(),
                domain: home_domain.to_string(),
            })
            .collect();
        let resp = self
            .http
            .list_users_by_handles(
                self.api_version,
                &self.current_access_token(),
                &qualified_handles,
            )
            .await?;
        let mut out = HashMap::new();
        for profile in resp.found {
            if let Some(handle) = &profile.handle {
                out.insert(handle.to_lowercase(), profile.qualified_id.to_string());
            }
        }
        Ok(out)
    }
}

/// One `@<handle>` token discovered in a `send` body. Step 6-II-g.
#[derive(Debug, Clone, PartialEq, Eq)]
struct AtHandleCandidate {
    /// Handle without the leading `@`, lowercased so the
    /// resolver's `HashMap` keys match Wire's case-insensitive
    /// comparisons.
    handle: String,
    /// UTF-16 code-unit offset of the `@` character into the
    /// body. Wire's mentions use UTF-16 offsets to match what
    /// iOS / Android / Web clients emit (`NSString` /
    /// `java.lang.String` index in UTF-16).
    utf16_start: u32,
    /// UTF-16 code-unit length of the entire `@<handle>` span,
    /// including the `@`.
    utf16_length: u32,
}

/// Scan `body` for `@<handle>` tokens. Returns one
/// `AtHandleCandidate` per match, with UTF-16 offsets ready for
/// Wire's `Mention { start, length }`. Step 6-II-g.
///
/// Token rule: an `@` either at byte 0 or preceded by ASCII
/// whitespace, followed by one or more characters from
/// `[A-Za-z0-9_.-]` (Wire's handle alphabet, per the
/// `400 invalid-handle` live test). The match consumes greedily
/// until the first non-handle byte.
fn scan_at_handles(body: &str) -> Vec<AtHandleCandidate> {
    let mut out = Vec::new();
    let bytes = body.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'@' {
            let preceded_ok = i == 0
                || bytes
                    .get(i - 1)
                    .copied()
                    .is_some_and(|b| b == b' ' || b == b'\t' || b == b'\n');
            if preceded_ok {
                let mut j = i + 1;
                while j < bytes.len() && is_handle_byte(bytes[j]) {
                    j += 1;
                }
                if j > i + 1 {
                    let utf16_start = u32::try_from(body[..i].encode_utf16().count()).unwrap_or(0);
                    let utf16_length =
                        u32::try_from(body[i..j].encode_utf16().count()).unwrap_or(0);
                    let handle = body[i + 1..j].to_lowercase();
                    out.push(AtHandleCandidate {
                        handle,
                        utf16_start,
                        utf16_length,
                    });
                    i = j;
                    continue;
                }
            }
        }
        i += 1;
    }
    out
}

#[inline]
fn is_handle_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'.' || b == b'-'
}

#[cfg(test)]
mod scan_at_handles_tests {
    use super::scan_at_handles;

    #[test]
    fn finds_simple_handle_at_start() {
        let m = scan_at_handles("@alice hi");
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].handle, "alice");
        assert_eq!(m[0].utf16_start, 0);
        assert_eq!(m[0].utf16_length, 6);
    }

    #[test]
    fn finds_handle_after_space() {
        let m = scan_at_handles("hi @bob!");
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].handle, "bob");
        assert_eq!(m[0].utf16_start, 3);
        assert_eq!(m[0].utf16_length, 4);
    }

    #[test]
    fn ignores_at_after_word_char() {
        // `foo@bar.com` is an email-ish token, not a mention.
        let m = scan_at_handles("mail foo@bar");
        assert!(m.is_empty(), "got {m:?}");
    }

    #[test]
    fn handles_emoji_before_token_with_correct_utf16_offset() {
        // 👍 = 2 UTF-16 code units. Body: "👍 @alice"
        let body = "\u{1F44D} @alice";
        let m = scan_at_handles(body);
        assert_eq!(m.len(), 1);
        // 👍 (2 utf16) + space (1) = 3
        assert_eq!(m[0].utf16_start, 3);
        assert_eq!(m[0].utf16_length, 6);
    }

    #[test]
    fn lowercases_resolved_handle_for_case_insensitive_lookup() {
        let m = scan_at_handles("@AliceB test");
        assert_eq!(m[0].handle, "aliceb");
    }

    #[test]
    fn empty_body_returns_empty() {
        assert!(scan_at_handles("").is_empty());
    }

    #[test]
    fn lone_at_sign_is_not_a_handle() {
        assert!(scan_at_handles("hi @ there").is_empty());
        assert!(scan_at_handles("@").is_empty());
    }
}
