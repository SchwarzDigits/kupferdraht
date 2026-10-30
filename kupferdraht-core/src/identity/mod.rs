// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Core user-identity type — [`UserId`].
//!
//! [`UserId`] is the canonical Wire user id `<localpart>@<domain>`, a validated
//! string newtype (the matrix-rust-sdk `OwnedUserId` equivalent). The domain is
//! guaranteed by construction — there is no domain-less `UserId` — which closes
//! the bare/qualified seam and the nullable-domain fragility (see
//! `docs/plan/19-user-id-type.md` and `docs/finding-qualified-user-id.md`).

use std::fmt;

use crate::error::KupferdrahtError;

/// A Wire user id in its canonical qualified form `<localpart>@<domain>`.
///
/// Invariant: exactly one `@`, with a non-empty localpart and a non-empty
/// domain. Both constructors ([`UserId::new`], [`UserId::parse`]) uphold it, so
/// every `UserId` value carries a domain — there is no `Option`. This is what
/// makes "is this me?" a plain `==` and removes the nullable-domain fragility.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct UserId(String);

impl UserId {
    /// Build from already-separated parts the caller has checked.
    /// Infallible — non-empty, `@`-free parts are the caller's
    /// precondition, debug-asserted. Parts from runtime data (event
    /// JSON, message payloads, backend responses) go through
    /// [`Self::from_parts`] instead (Step 95c). Path safety is NOT
    /// this constructor's invariant (that is [`Self::parse`]'s stricter
    /// layer, Step 80): ids that become path components are refused at
    /// the materialization sites (`open_default_store` /
    /// `open_mls_provider`).
    #[must_use]
    pub fn new(localpart: &str, domain: &str) -> Self {
        debug_assert!(!localpart.is_empty(), "UserId localpart must be non-empty");
        debug_assert!(!domain.is_empty(), "UserId domain must be non-empty");
        debug_assert!(
            !localpart.contains('@'),
            "UserId localpart must not contain '@'"
        );
        debug_assert!(!domain.contains('@'), "UserId domain must not contain '@'");
        Self(format!("{localpart}@{domain}"))
    }

    /// Build from separated parts that come from runtime data (Wire's
    /// `QualifiedId { id, domain }` in an event, a mention, a response).
    ///
    /// # Errors
    /// `KupferdrahtError::InvalidInput` when a part is empty or contains `@`.
    pub fn from_parts(localpart: &str, domain: &str) -> Result<Self, KupferdrahtError> {
        let well_formed = [localpart, domain]
            .iter()
            .all(|part| !part.is_empty() && !part.contains('@'));
        if !well_formed {
            return Err(KupferdrahtError::InvalidInput(format!(
                "malformed qualified id {localpart:?} @ {domain:?}: \
                 expected non-empty parts without '@'"
            )));
        }
        Ok(Self::new(localpart, domain))
    }

    /// Parse a flat `<localpart>@<domain>` string (a DB column value, a CLI
    /// argument).
    ///
    /// # Errors
    /// `KupferdrahtError::InvalidInput` unless `s` is exactly one `@` with a
    /// non-empty localpart and a non-empty domain, both free of path
    /// metacharacters (`/`, `\`, `..`, control characters) — the id
    /// is joined into the on-disk account layout
    /// (`base/accounts/<id>`, Step 80), and the backend supplying it
    /// is not trusted with path structure.
    pub fn parse(s: &str) -> Result<Self, KupferdrahtError> {
        match s.split_once('@') {
            Some((localpart, domain))
                if !domain.contains('@')
                    && crate::storage::is_path_safe_id(localpart)
                    && crate::storage::is_path_safe_id(domain) =>
            {
                Ok(Self(s.to_string()))
            }
            _ => Err(KupferdrahtError::InvalidInput(format!(
                "malformed user id {s:?}: expected <localpart>@<domain> \
                 without path metacharacters"
            ))),
        }
    }

    /// The part before `@` — the Wire user UUID today (not type-guaranteed).
    #[must_use]
    pub fn localpart(&self) -> &str {
        self.0
            .split_once('@')
            .map_or(self.0.as_str(), |(local, _)| local)
    }

    /// The part after `@` — the Wire backend domain. Always present.
    #[must_use]
    pub fn domain(&self) -> &str {
        self.0.split_once('@').map_or("", |(_, domain)| domain)
    }

    /// The canonical qualified string `<localpart>@<domain>`.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for UserId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod path_safety_tests {
    use super::UserId;

    /// Step 80: the account id is joined into `base/accounts/<id>`
    /// (`storage/home.rs`); `parse` is the boundary. A malicious
    /// backend must not be able to smuggle path metacharacters into
    /// the on-disk layout via the login identity.
    #[test]
    fn parse_rejects_path_metacharacters() {
        for bad in [
            "a/b@example.test",
            "..@example.test",
            "uuid@..",
            "uuid@do/main",
            "uuid@do\\main",
            "uuid@dom\u{0}ain",
            "uu\nid@example.test",
        ] {
            assert!(
                UserId::parse(bad).is_err(),
                "must reject path-unsafe id {bad:?}"
            );
        }
    }

    /// Existing legitimate shapes keep parsing (federated domains
    /// with dots and hyphens included).
    #[test]
    fn parse_keeps_accepting_real_ids() {
        for good in [
            "a11ce000-0000-4000-8000-000000000001@example.test",
            "user@sub.domain-x.com",
        ] {
            assert!(UserId::parse(good).is_ok(), "must accept {good:?}");
        }
    }
}
