// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Inbound `Ephemeral` (self-deleting wrapper, field 18) dispatch —
//! Step 89.
//!
//! Receive-side toleration in the ADR-0011 sense: an official client's
//! timed text lands as an ordinary text row decorated with
//! `expires_at_ms`, and Kupferdraht runs the local delete timer itself
//! (`timeline/ephemeral.rs`). Kupferdraht declares only the `Text`
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

use super::mentions_to_records;
use super::text::inbound_text_message;

/// A peer's `expire_after_millis` within Wire's range. Peer data, never
/// a panic: a non-positive TTL expires now (the sweep retires it); one
/// above the range gets the longest timer (Step 95c — `i64::MAX`
/// overflowed the expiry stamp). Out-of-range values are logged, like
/// every other malformed peer value of 95c.
fn clamped_ttl_ms(raw_ms: i64, message_id: &str) -> i64 {
    let ttl_ms = raw_ms.clamp(0, crate::timeline::ephemeral::EPHEMERAL_TTL_MAX_MS);
    if ttl_ms != raw_ms {
        warn!(
            message_id,
            raw_ms, ttl_ms, "inbound ephemeral TTL outside Wire's range; clamped"
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
    pub(crate) async fn dispatch_inbound_ephemeral(
        &self,
        client: &Arc<Client>,
        room_id: &str,
        sender_user_id: &str,
        sender_device_id: Option<&str>,
        message_id: &str,
        ephemeral: crate::wire::proto::Ephemeral,
    ) -> Result<(), KupferdrahtError> {
        debug_assert!(
            !message_id.is_empty(),
            "dispatch_inbound_ephemeral: message_id must be non-empty"
        );
        let ttl_ms = clamped_ttl_ms(ephemeral.expire_after_millis, message_id);
        match ephemeral.content {
            Some(ephemeral::Content::Text(text)) => {
                let mention_records = mentions_to_records(room_id, message_id, &text.mentions);
                let message = inbound_text_message(
                    room_id,
                    sender_user_id,
                    sender_device_id,
                    message_id,
                    text,
                    Some(ttl_ms),
                );
                self.dispatch_inbound_text_message(client, message, mention_records)
                    .await
            }
            None => {
                warn!(
                    message_id,
                    ttl_ms,
                    "Ephemeral with an undeclared inner arm (image/knock/asset/location) — \
                     discarded at the content layer"
                );
                Ok(())
            }
        }
    }
}
