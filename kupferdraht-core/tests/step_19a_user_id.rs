// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 19a — the core `UserId` type.
//!
//! TDD (red): written before the implementation. They pin the contract of
//! `kupferdraht::identity::UserId` — the canonical `<localpart>@<domain>` form, a
//! validating `parse`, structural `localpart()` / `domain()` accessors, and
//! value equality / hashing. Expected to fail until 19a is implemented.

use std::collections::HashSet;

use kupferdraht::KupferdrahtError;
use kupferdraht::identity::UserId;

const UUID: &str = "c0ffee01-0000-4000-8000-000000000001";
const DOMAIN: &str = "wire.com";

#[test]
fn new_builds_the_canonical_qualified_form() {
    let uid = UserId::new(UUID, DOMAIN);
    assert_eq!(uid.as_str(), format!("{UUID}@{DOMAIN}"));
}

#[test]
fn parse_round_trips_a_qualified_string() {
    let s = format!("{UUID}@{DOMAIN}");
    let uid = UserId::parse(&s).expect("a well-formed qualified id must parse");
    assert_eq!(uid.as_str(), s);
}

#[test]
fn accessors_slice_localpart_and_domain() {
    let uid = UserId::parse(&format!("{UUID}@{DOMAIN}")).expect("parse");
    assert_eq!(uid.localpart(), UUID, "localpart is the part before @");
    assert_eq!(uid.domain(), DOMAIN, "domain is the part after @");
}

#[test]
fn display_is_the_canonical_string() {
    let uid = UserId::new(UUID, DOMAIN);
    assert_eq!(uid.to_string(), format!("{UUID}@{DOMAIN}"));
}

#[test]
fn new_and_parse_produce_equal_values() {
    let built = UserId::new(UUID, DOMAIN);
    let parsed = UserId::parse(&format!("{UUID}@{DOMAIN}")).expect("parse");
    assert_eq!(built, parsed);
}

#[test]
fn parse_rejects_malformed_input() {
    for bad in [
        "no-at-sign",
        "",
        "@only-domain.test",
        "only-localpart@",
        "a@b@c",
    ] {
        let result = UserId::parse(bad);
        assert!(
            matches!(result, Err(KupferdrahtError::InvalidInput(_))),
            "{bad:?} must be rejected as InvalidInput"
        );
    }
}

#[test]
fn equal_ids_hash_equal_and_differ_by_domain() {
    let mut set = HashSet::new();
    set.insert(UserId::new(UUID, DOMAIN));
    assert!(
        set.contains(&UserId::parse(&format!("{UUID}@{DOMAIN}")).expect("parse")),
        "equal UserIds must hash-collide — usable as a HashSet/HashMap key"
    );
    assert!(
        !set.contains(&UserId::new(UUID, "other.test")),
        "a different domain is a different UserId"
    );
}
