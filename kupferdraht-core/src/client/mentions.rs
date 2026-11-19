// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `Client` — @-mention resolution for outbound message bodies.
//!
//! Split out of [`super`] in Step 29: the `@<username>` scanner
//! ([`scan_at_usernames`]) and the directory-resolution methods
//! ([`Client::resolve_mentions_in_body`], `resolve_usernames`)
//! that turn usernames into user ids with UTF-16 offsets for
//! Wire's `Mention { start, length }`. Step 6-II-g.

#![allow(clippy::doc_markdown)]

use std::collections::HashMap;

use crate::error::KupferdrahtError;

use super::Client;

impl Client {
    /// Scan `body` for `@<username>` tokens, resolve each one to a
    /// user id via `POST /v14/list-users`, and return the
    /// list of [`crate::timeline::Mention`] entries with UTF-16
    /// offsets into `body`. Usernames that don't resolve are left
    /// alone (no mention emitted, body untouched). Step 6-II-g.
    ///
    /// Token rule: `@` followed by one or more characters from
    /// `[A-Za-z0-9_.-]` (the backend's username alphabet, per the
    /// 400 invalid-handle live test). The token boundary on the
    /// right is the first character outside it; on the left, the
    /// `@` must be either at start-of-body or preceded by
    /// whitespace.
    ///
    /// **Silent-no-op modes** (caller sees an empty `Vec`, no
    /// `Err`):
    /// - The body contains no `@`-tokens.
    /// - The account has no `domain()` on file (defensive corner
    ///   case — corrupted / metadata-only resume DB). Rationale +
    ///   warn-log in [`Self::resolve_usernames`].
    /// - One or more usernames do not resolve via the backend's
    ///   directory. Resolved ones are returned; unresolved ones are
    ///   skipped.
    ///
    /// # Errors
    /// Network / backend errors from `list-users` propagate.
    pub async fn resolve_mentions_in_body(
        &self,
        body: &str,
    ) -> Result<Vec<crate::timeline::Mention>, KupferdrahtError> {
        let candidates = scan_at_usernames(body);
        if candidates.is_empty() {
            return Ok(Vec::new());
        }
        let usernames: Vec<String> = candidates.iter().map(|c| c.username.clone()).collect();
        let user_ids = self.resolve_usernames(&usernames).await?;
        let mut out = Vec::new();
        for c in candidates {
            if let Some(user_id) = user_ids.get(&c.username) {
                out.push(crate::timeline::Mention {
                    user_id: user_id.clone(),
                    start: c.utf16_start,
                    length: c.utf16_length,
                });
            }
        }
        Ok(out)
    }

    /// Crate-private: resolve a list of usernames to user ids via
    /// `POST /v14/list-users`. Returns a
    /// `HashMap<username_lowercase, user_id>`; usernames the backend
    /// couldn't resolve are absent from the map.
    ///
    /// The account's own domain — carried in [`UserId`], so always
    /// present since Step 19 — is the domain every username is
    /// looked up on.
    async fn resolve_usernames(
        &self,
        usernames: &[String],
    ) -> Result<HashMap<String, String>, KupferdrahtError> {
        // The lookup names a domain per username. Use the caller's
        // own domain — on a single-backend deployment every username
        // lives on the user's home backend.
        let account = self.account();
        let home_domain = account.domain();
        let lookup: Vec<crate::wire::dto::QualifiedHandle> = usernames
            .iter()
            .map(|username| crate::wire::dto::QualifiedHandle {
                handle: username.clone(),
                domain: home_domain.to_string(),
            })
            .collect();
        let resp = self
            .http
            .list_users_by_handles(self.api_version, &self.current_access_token(), &lookup)
            .await?;
        let mut out = HashMap::new();
        for profile in resp.found {
            if let Some(username) = &profile.handle {
                out.insert(username.to_lowercase(), profile.qualified_id.to_string());
            }
        }
        Ok(out)
    }
}

/// One `@<username>` token discovered in a `send` body. Step 6-II-g.
#[derive(Debug, Clone, PartialEq, Eq)]
struct AtUsernameCandidate {
    /// Username without the leading `@`, lowercased so the
    /// resolver's `HashMap` keys match the backend's case-insensitive
    /// comparisons.
    username: String,
    /// UTF-16 code-unit offset of the `@` character into the
    /// body. Wire's mentions use UTF-16 offsets to match what
    /// iOS / Android / Web clients emit (`NSString` /
    /// `java.lang.String` index in UTF-16).
    utf16_start: u32,
    /// UTF-16 code-unit length of the entire `@<username>` span,
    /// including the `@`.
    utf16_length: u32,
}

/// Scan `body` for `@<username>` tokens. Returns one
/// `AtUsernameCandidate` per match, with UTF-16 offsets ready for
/// Wire's `Mention { start, length }`. Step 6-II-g.
///
/// Token rule: an `@` either at byte 0 or preceded by ASCII
/// whitespace, followed by one or more characters from
/// `[A-Za-z0-9_.-]` (the backend's username alphabet, per the
/// `400 invalid-handle` live test). The match consumes greedily
/// until the first byte outside it.
fn scan_at_usernames(body: &str) -> Vec<AtUsernameCandidate> {
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
                while j < bytes.len() && is_username_byte(bytes[j]) {
                    j += 1;
                }
                if j > i + 1 {
                    let utf16_start = u32::try_from(body[..i].encode_utf16().count()).unwrap_or(0);
                    let utf16_length =
                        u32::try_from(body[i..j].encode_utf16().count()).unwrap_or(0);
                    let username = body[i + 1..j].to_lowercase();
                    out.push(AtUsernameCandidate {
                        username,
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
fn is_username_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'.' || b == b'-'
}

#[cfg(test)]
mod scan_at_usernames_tests {
    use super::scan_at_usernames;

    #[test]
    fn finds_simple_username_at_start() {
        let m = scan_at_usernames("@alice hi");
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].username, "alice");
        assert_eq!(m[0].utf16_start, 0);
        assert_eq!(m[0].utf16_length, 6);
    }

    #[test]
    fn finds_username_after_space() {
        let m = scan_at_usernames("hi @bob!");
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].username, "bob");
        assert_eq!(m[0].utf16_start, 3);
        assert_eq!(m[0].utf16_length, 4);
    }

    #[test]
    fn ignores_at_after_word_char() {
        // `foo@bar.com` is an email-ish token, not a mention.
        let m = scan_at_usernames("mail foo@bar");
        assert!(m.is_empty(), "got {m:?}");
    }

    #[test]
    fn handles_emoji_before_token_with_correct_utf16_offset() {
        // 👍 = 2 UTF-16 code units. Body: "👍 @alice"
        let body = "\u{1F44D} @alice";
        let m = scan_at_usernames(body);
        assert_eq!(m.len(), 1);
        // 👍 (2 utf16) + space (1) = 3
        assert_eq!(m[0].utf16_start, 3);
        assert_eq!(m[0].utf16_length, 6);
    }

    #[test]
    fn lowercases_resolved_username_for_case_insensitive_lookup() {
        let m = scan_at_usernames("@AliceB test");
        assert_eq!(m[0].username, "aliceb");
    }

    #[test]
    fn empty_body_returns_empty() {
        assert!(scan_at_usernames("").is_empty());
    }

    #[test]
    fn lone_at_sign_is_not_a_username() {
        assert!(scan_at_usernames("hi @ there").is_empty());
        assert!(scan_at_usernames("@").is_empty());
    }
}
