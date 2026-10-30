// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 95a (ADR-0027) — who an inbound application message is
//! attributed to.
//!
//! The leaf openmls authenticated as the signer is the sender; the
//! backend-written envelope (`qualified_from` / `from_client`) is a
//! cross-check only — on a mismatch the message still goes out under
//! the MLS sender, and a `warn!` records both claims. An X.509
//! (non-Basic) leaf keeps the envelope attribution until X.509 parsing
//! is verified; a malformed Basic leaf drops the message.

use tracing::{debug, warn};

use crate::encryption::{LeafIdentity, LeafIdentityError};

/// Stand-in for an envelope that names no sender — reachable only on
/// the X.509 path, where the envelope is the attribution source.
const UNKNOWN_SENDER: &str = "unknown@unknown";

/// What the backend-written envelope claims about the sender.
pub(super) struct EnvelopeSender<'a> {
    pub(super) user_id: Option<&'a str>,
    pub(super) device_id: Option<&'a str>,
}

/// The sender an inbound application message is dispatched under.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct InboundSender {
    pub(super) user_id: String,
    pub(super) device_id: Option<String>,
}

/// Attribute an inbound application message per ADR-0027. `None` means
/// "drop it" (a malformed Basic leaf).
pub(super) fn attribute_sender(
    envelope: &EnvelopeSender<'_>,
    leaf: Result<LeafIdentity, LeafIdentityError>,
    message_id: &str,
) -> Option<InboundSender> {
    match leaf {
        Ok(identity) => {
            warn_on_envelope_mismatch(envelope, &identity, message_id);
            Some(InboundSender {
                user_id: identity.user_id,
                device_id: Some(identity.device_id),
            })
        }
        Err(LeafIdentityError::Unsupported) => {
            debug!(
                message_id,
                "non-Basic (X.509) leaf credential — envelope attribution (ADR-0027 d.2)"
            );
            Some(InboundSender {
                user_id: envelope.user_id.unwrap_or(UNKNOWN_SENDER).to_string(),
                device_id: envelope.device_id.map(str::to_string),
            })
        }
        Err(LeafIdentityError::Malformed(reason)) => {
            warn!(
                message_id,
                reason, "malformed Basic leaf credential — message dropped (ADR-0027)"
            );
            None
        }
    }
}

/// Log when the envelope names a different user or device than the
/// authenticated leaf — a misbehaving backend's fingerprint. An
/// envelope that names nobody is not a mismatch.
fn warn_on_envelope_mismatch(
    envelope: &EnvelopeSender<'_>,
    identity: &LeafIdentity,
    message_id: &str,
) {
    let user_differs = envelope.user_id.is_some_and(|u| u != identity.user_id);
    let device_differs = envelope.device_id.is_some_and(|d| d != identity.device_id);
    if user_differs || device_differs {
        warn!(
            message_id,
            envelope_user = envelope.user_id.unwrap_or_default(),
            envelope_device = envelope.device_id.unwrap_or_default(),
            mls_user = %identity.user_id,
            mls_device = %identity.device_id,
            "envelope sender mismatch — dispatching under the MLS-authenticated sender (ADR-0027)"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{EnvelopeSender, InboundSender, attribute_sender};
    use crate::encryption::{LeafIdentity, LeafIdentityError};

    fn bob() -> LeafIdentity {
        LeafIdentity {
            user_id: "bob@example.test".into(),
            device_id: "bobdev".into(),
        }
    }

    fn envelope<'a>(user: Option<&'a str>, device: Option<&'a str>) -> EnvelopeSender<'a> {
        EnvelopeSender {
            user_id: user,
            device_id: device,
        }
    }

    #[test]
    fn the_leaf_wins_over_a_lying_envelope() {
        let sender = attribute_sender(
            &envelope(Some("carol@example.test"), Some("carolphone")),
            Ok(bob()),
            "m1",
        );
        assert_eq!(
            sender,
            Some(InboundSender {
                user_id: "bob@example.test".into(),
                device_id: Some("bobdev".into()),
            })
        );
    }

    #[test]
    fn x509_falls_back_to_the_envelope() {
        let sender = attribute_sender(
            &envelope(Some("bob@example.test"), Some("bobx509")),
            Err(LeafIdentityError::Unsupported),
            "m1",
        );
        assert_eq!(
            sender,
            Some(InboundSender {
                user_id: "bob@example.test".into(),
                device_id: Some("bobx509".into()),
            })
        );
    }

    #[test]
    fn x509_without_envelope_sender_keeps_the_legacy_placeholder() {
        let sender = attribute_sender(
            &envelope(None, None),
            Err(LeafIdentityError::Unsupported),
            "m1",
        );
        assert_eq!(
            sender.map(|s| s.user_id),
            Some("unknown@unknown".to_string())
        );
    }

    #[test]
    fn a_malformed_basic_leaf_drops_the_message() {
        let sender = attribute_sender(
            &envelope(Some("bob@example.test"), Some("bobdev")),
            Err(LeafIdentityError::Malformed(
                "identity lacks the ':' separator",
            )),
            "m1",
        );
        assert_eq!(sender, None);
    }
}
