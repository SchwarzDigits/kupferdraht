// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 95a (ADR-0027) — the one parse path for a leaf's Wire identity.
//!
//! Wire packs `{uuid}:{client_id}@{domain}` into a leaf's
//! `BasicCredential.identity` (mls-deviations W-4). The sender of an
//! inbound application message is the leaf openmls authenticated, so
//! this module turns that leaf's credential into the qualified user id
//! and the device id Kupferdraht attributes the message to. A non-Basic
//! (e2ei X.509) credential is *unsupported* — its sender keeps the
//! envelope attribution until the X.509 follow-up step; a Basic
//! credential that is not exactly three non-empty parts is *malformed*.
//! The build half ([`wire_identity`]) lives here too, so the shape is
//! written and read in one place.

use openmls::prelude::{Credential, CredentialType};

use crate::error::KupferdrahtError;

/// The Wire identity carried by a leaf's `BasicCredential`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LeafIdentity {
    /// Qualified user id, `{uuid}@{domain}`.
    pub(crate) user_id: String,
    /// Wire `client_id` of the device — the same string the API uses.
    pub(crate) device_id: String,
}

/// Why a leaf credential yields no [`LeafIdentity`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LeafIdentityError {
    /// Not a `BasicCredential` (an e2ei X.509 credential) — attributed
    /// by the envelope until X.509 parsing is verified (ADR-0027 d.2).
    Unsupported,
    /// A `BasicCredential` whose identity is not W-4; the reason names
    /// the defect without echoing unbounded peer bytes.
    Malformed(&'static str),
}

/// Upper bound on a leaf identity: a UUID, a client id and a domain fit
/// well within it.
const MAX_IDENTITY_LEN: usize = 512;

/// Parse the Wire identity out of a leaf credential (ADR-0027 d.1).
///
/// # Errors
/// [`LeafIdentityError::Unsupported`] for a non-Basic credential,
/// [`LeafIdentityError::Malformed`] for a Basic credential that is not
/// exactly `{uuid}:{client_id}@{domain}` with three non-empty parts, is
/// longer than 512 bytes, or contains control characters.
pub(crate) fn parse_leaf_identity(
    credential: &Credential,
) -> Result<LeafIdentity, LeafIdentityError> {
    if credential.credential_type() != CredentialType::Basic {
        return Err(LeafIdentityError::Unsupported);
    }
    let raw = credential.serialized_content();
    // The identity is chosen by whoever built the key package (under
    // Basic credentials, possibly an injected device): bounded and free
    // of control characters before it reaches rows, logs and terminals.
    if raw.len() > MAX_IDENTITY_LEN {
        return Err(LeafIdentityError::Malformed("identity is too long"));
    }
    let identity = std::str::from_utf8(raw)
        .map_err(|_| LeafIdentityError::Malformed("identity is not UTF-8"))?;
    if identity.chars().any(char::is_control) {
        return Err(LeafIdentityError::Malformed(
            "identity contains control characters",
        ));
    }
    let (uuid, rest) = identity
        .split_once(':')
        .ok_or(LeafIdentityError::Malformed(
            "identity lacks the ':' separator",
        ))?;
    let (client, domain) = rest.split_once('@').ok_or(LeafIdentityError::Malformed(
        "identity lacks the '@' separator",
    ))?;
    if [uuid, client, domain]
        .iter()
        .any(|part| part.is_empty() || part.contains([':', '@']))
    {
        return Err(LeafIdentityError::Malformed(
            "identity is not exactly three non-empty parts",
        ));
    }
    Ok(LeafIdentity {
        user_id: crate::identity::UserId::new(uuid, domain)
            .as_str()
            .to_string(),
        device_id: client.to_string(),
    })
}

/// Build this device's W-4 identity bytes from its qualified user id
/// and Wire `client_id` — the inverse of [`parse_leaf_identity`].
///
/// # Errors
/// `KupferdrahtError::InvalidInput` when `user_id` is not `<uuid>@<domain>`.
pub(crate) fn wire_identity(user_id: &str, client_id: &str) -> Result<Vec<u8>, KupferdrahtError> {
    let (uuid, domain) = user_id.split_once('@').ok_or_else(|| {
        KupferdrahtError::InvalidInput(format!(
            "user_id must be qualified '<uuid>@<domain>' format (got: '{user_id}'); \
             see docs/library.md §Identifiers"
        ))
    })?;
    Ok(format!("{uuid}:{client_id}@{domain}").into_bytes())
}

/// Map a leaf's identity defect onto the crate error for callers that
/// treat any non-W-4 leaf as a hard failure (member lists, own leaf).
pub(crate) fn identity_error(context: &str, e: &LeafIdentityError) -> KupferdrahtError {
    KupferdrahtError::Encoding(format!("{context}: leaf credential identity {e:?}"))
}

#[cfg(test)]
mod tests {
    use super::{LeafIdentity, LeafIdentityError, parse_leaf_identity, wire_identity};
    use openmls::prelude::{BasicCredential, Credential, CredentialType};

    fn basic(identity: &[u8]) -> Credential {
        BasicCredential::new(identity.to_vec()).into()
    }

    fn assert_malformed(identity: &[u8]) {
        assert!(
            matches!(
                parse_leaf_identity(&basic(identity)),
                Err(LeafIdentityError::Malformed(_))
            ),
            "{:?} must be malformed",
            String::from_utf8_lossy(identity)
        );
    }

    /// Review #55: the identity is peer-chosen — an oversized one or one
    /// with control characters (terminal escapes) never reaches rows,
    /// logs or terminals.
    #[test]
    fn oversized_or_control_character_identities_are_malformed() {
        let long = format!("{}:dev@example.test", "a".repeat(600));
        assert_malformed(long.as_bytes());
        assert_malformed(b"bob-uuid:dev\x1b[2J@example.test");
        assert_malformed(b"bob-uuid:dev@example.test\n");
    }

    #[test]
    fn parses_the_wire_identity_into_user_and_device() {
        let parsed = parse_leaf_identity(&basic(b"bob-uuid:bobdev01@example.test"));
        assert_eq!(
            parsed,
            Ok(LeafIdentity {
                user_id: "bob-uuid@example.test".into(),
                device_id: "bobdev01".into(),
            })
        );
    }

    #[test]
    fn keeps_the_parts_verbatim() {
        // The client id is the API's own string (same source as the
        // envelope's `from_client`) — no case or zero-padding games.
        let parsed = parse_leaf_identity(&basic(b"Bob-UUID:0ab1@Example.test")).expect("W-4");
        assert_eq!(parsed.user_id, "Bob-UUID@Example.test");
        assert_eq!(parsed.device_id, "0ab1");
    }

    #[test]
    fn rejects_anything_but_three_non_empty_parts() {
        for bad in [
            &b"no-colon@example.test"[..],
            b"uuid:no-at-sign",
            b":client@example.test",
            b"uuid:@example.test",
            b"uuid:client@",
            b"uuid:client:extra@example.test",
            b"uuid:client@example.test@extra",
            b"",
        ] {
            assert_malformed(bad);
        }
    }

    #[test]
    fn rejects_non_utf8_identity() {
        assert_malformed(&[0xFF, 0xFE, b':', b'c', b'@', b'd']);
    }

    #[test]
    fn wire_identity_round_trips_through_the_parser() {
        let bytes = wire_identity("a11ce000@wire.com", "a11ce00000000001").expect("build");
        assert_eq!(
            parse_leaf_identity(&basic(&bytes)),
            Ok(LeafIdentity {
                user_id: "a11ce000@wire.com".into(),
                device_id: "a11ce00000000001".into(),
            })
        );
    }

    #[test]
    fn reports_x509_as_unsupported_not_malformed() {
        let x509 = Credential::new(CredentialType::X509, vec![0x30, 0x82, 0x01, 0x0a]);
        assert_eq!(
            parse_leaf_identity(&x509),
            Err(LeafIdentityError::Unsupported)
        );
    }
}
