// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 95a (ADR-0027) — who an inbound application message is
//! attributed to.
//!
//! The leaf openmls authenticated as the signer is the sender; the
//! backend-written envelope (`qualified_from` / `from_client`) is a
//! cross-check only — on a mismatch the message still goes out under
//! the MLS sender, and a `warn!` records both claims. A leaf whose
//! credential is not Basic (an e2ei X.509 certificate) yields no
//! authenticated identity, so its message is dropped with a `warn!`
//! (Step 135, ADR-0027 decision 2 as amended), like a malformed Basic
//! leaf.

use tracing::warn;

use crate::encryption::{LeafIdentity, LeafIdentityError};

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
/// "drop it": a non-Basic leaf (Step 135) or a malformed Basic one.
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
            warn!(
                message_id,
                "non-Basic (X.509) leaf credential — nothing authenticates the sender, \
                 message dropped (ADR-0027 d.2, Step 135)"
            );
            None
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

    /// Step 135: a message signed by a non-Basic (X.509) leaf is dropped,
    /// however complete the envelope — nothing authenticated the envelope.
    #[test]
    fn a_non_basic_leaf_drops_the_message_despite_a_full_envelope() {
        let sender = attribute_sender(
            &envelope(Some("bob@example.test"), Some("bobx509")),
            Err(LeafIdentityError::Unsupported),
            "m1",
        );
        assert_eq!(sender, None);
    }

    #[test]
    fn a_non_basic_leaf_without_an_envelope_sender_drops_too() {
        let sender = attribute_sender(
            &envelope(None, None),
            Err(LeafIdentityError::Unsupported),
            "m1",
        );
        assert_eq!(sender, None);
    }

    /// The drop is logged at `warn!` (not `debug!`) with the message id and
    /// the credential class, so an operator sees what vanished.
    ///
    /// Step 158: the capture runs in a process of its own. `tracing` caches a
    /// callsite's interest for the whole process, and while the capture is
    /// the only registered dispatcher, a parallel test that reaches the
    /// `warn!` first settles it with that thread's dispatcher — none — so
    /// the capture saw nothing in about one run of fourteen.
    #[test]
    fn a_dropped_non_basic_leaf_is_logged_at_warn_with_the_message_id() {
        let run = std::process::Command::new(std::env::current_exe().expect("the test binary"))
            .args([
                "--exact",
                "sync::sender::tests::the_drop_warning_captured_alone",
                "--ignored",
                "--test-threads=1",
            ])
            .output()
            .expect("the test binary runs");
        let stdout = String::from_utf8_lossy(&run.stdout);
        assert!(
            run.status.success() && stdout.contains("1 passed"),
            "the capture, alone in its process: {stdout}{}",
            String::from_utf8_lossy(&run.stderr)
        );
    }

    #[test]
    #[ignore = "runs in a process of its own, started by a_dropped_non_basic_leaf_is_logged_at_warn_with_the_message_id"]
    fn the_drop_warning_captured_alone() {
        let capture = CaptureSubscriber::default();
        let events = std::sync::Arc::clone(&capture.events);
        tracing::subscriber::with_default(capture, || {
            attribute_sender(
                &envelope(Some("bob@example.test"), Some("bobx509")),
                Err(LeafIdentityError::Unsupported),
                "m-dropped",
            );
        });
        let events = events.lock().expect("capture lock");
        let warns: Vec<&(tracing::Level, String)> = events
            .iter()
            .filter(|(level, _)| *level == tracing::Level::WARN)
            .collect();
        assert_eq!(warns.len(), 1, "exactly one warn event, got {events:?}");
        let rendered = &warns[0].1;
        assert!(
            rendered.contains("message_id=\"m-dropped\""),
            "the warn names the message id: {rendered}"
        );
        assert!(
            rendered.contains("X.509") || rendered.contains("non-Basic"),
            "the warn names the credential class: {rendered}"
        );
    }

    /// Minimal scoped `tracing::Subscriber`: records `(level, rendered
    /// fields)` of every event emitted while it is the default.
    #[derive(Default)]
    struct CaptureSubscriber {
        events: std::sync::Arc<std::sync::Mutex<Vec<(tracing::Level, String)>>>,
    }

    impl tracing::Subscriber for CaptureSubscriber {
        fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
            true
        }
        fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
            tracing::span::Id::from_u64(1)
        }
        fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
        fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
        fn event(&self, event: &tracing::Event<'_>) {
            let mut rendered = String::new();
            event.record(&mut FieldRenderer(&mut rendered));
            self.events
                .lock()
                .expect("capture lock")
                .push((*event.metadata().level(), rendered));
        }
        fn enter(&self, _: &tracing::span::Id) {}
        fn exit(&self, _: &tracing::span::Id) {}
    }

    struct FieldRenderer<'a>(&'a mut String);

    impl tracing::field::Visit for FieldRenderer<'_> {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            use std::fmt::Write as _;
            let _ = write!(self.0, "{}={value:?} ", field.name());
        }
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
