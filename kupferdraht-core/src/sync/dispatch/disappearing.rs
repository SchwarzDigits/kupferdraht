// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Inbound `Ephemeral` (self-deleting wrapper, field 18) dispatch —
//! Step 89.
//!
//! Receive-side toleration in the ADR-0011 sense: an official client's
//! timed text lands as an ordinary text row decorated with
//! `expires_at_ms`, and Kupferdraht runs the local delete timer itself
//! (`timeline/disappearing.rs`). Kupferdraht declares only the `Text`
//! inner arm; any other inner content decodes to `content = None` and
//! is discarded here at the content
//! layer — never at the envelope layer, never as an error — so the
//! rest of the frame still dispatches.

use std::sync::Arc;

use tracing::warn;

use crate::client::Client;
use crate::error::KupferdrahtError;
use crate::sync::service::SyncService;
use crate::wire::proto::ephemeral;

use super::text::inbound_text_message;
use super::{InboundOrigin, mentions_to_records};

/// A peer's `expire_after_millis` within Wire's range. Peer data, never
/// a panic: a non-positive TTL expires now (the sweep retires it); one
/// above the range gets the longest timer (Step 95c — `i64::MAX`
/// overflowed the expiry stamp). Out-of-range values are logged, like
/// every other malformed peer value of 95c.
fn clamped_ttl_ms(raw_ms: i64, message_id: &str) -> i64 {
    let ttl_ms = raw_ms.clamp(0, crate::timeline::disappearing::DISAPPEARING_TTL_MAX_MS);
    if ttl_ms != raw_ms {
        warn!(
            message_id,
            raw_ms, ttl_ms, "inbound disappearing TTL outside Wire's range; clamped"
        );
    }
    ttl_ms
}

impl SyncService {
    /// Step 89: dispatch an inbound `Ephemeral` wrapper. `Text` inside
    /// lands as an ordinary text row decorated with `expires_at_ms`
    /// (timer start = now); any other inner arm is logged and
    /// discarded at the content layer (tolerated-unknown, never an
    /// error).
    pub(crate) async fn dispatch_inbound_disappearing(
        &self,
        client: &Arc<Client>,
        origin: InboundOrigin<'_>,
        disappearing: crate::wire::proto::Ephemeral,
    ) -> Result<(), KupferdrahtError> {
        let InboundOrigin {
            room_id,
            message_id,
            ..
        } = origin;
        debug_assert!(
            !message_id.is_empty(),
            "dispatch_inbound_disappearing: message_id must be non-empty"
        );
        let ttl_ms = clamped_ttl_ms(disappearing.expire_after_millis, message_id);
        match disappearing.content {
            Some(ephemeral::Content::Text(text)) => {
                let mention_records = mentions_to_records(room_id, message_id, &text.mentions);
                let message = inbound_text_message(origin, text, Some(ttl_ms));
                self.dispatch_inbound_text_message(client, message, mention_records)
                    .await
            }
            None => {
                warn!(
                    message_id,
                    ttl_ms,
                    "a disappearing message with an undeclared inner arm (image/ping/attachment/location) — \
                     discarded at the content layer"
                );
                Ok(())
            }
        }
    }
}
