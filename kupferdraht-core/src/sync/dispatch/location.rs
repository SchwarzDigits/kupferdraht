// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Inbound `Location` (shared map point) dispatch — Step 26.

use std::sync::Arc;

use tracing::warn;

use crate::client::Client;
use crate::error::KupferdrahtError;
use crate::sync::event_handler::MessageBody;
use crate::sync::service::SyncService;

impl SyncService {
    /// Step 26: dispatch an inbound `Location` (shared map point).
    pub(crate) async fn dispatch_inbound_location(
        &self,
        client: &Arc<Client>,
        room_id: &str,
        sender_user_id: &str,
        sender_device_id: Option<&str>,
        message_id: &str,
        location: crate::wire::proto::Location,
    ) -> Result<(), KupferdrahtError> {
        // Non-finite coordinates from a buggy/hostile remote do not survive
        // the JSON persistence round-trip; discard the message gracefully
        // (CLAUDE.md: malformed content → log + drop) rather than corrupt the
        // stored row.
        if !location.latitude.is_finite() || !location.longitude.is_finite() {
            warn!(
                message_id,
                "discarding inbound Location with non-finite coordinates"
            );
            return Ok(());
        }
        self.dispatch_inbound_simple(
            client,
            room_id,
            sender_user_id,
            sender_device_id,
            message_id,
            MessageBody::Location {
                latitude: location.latitude,
                longitude: location.longitude,
                name: location.name,
                zoom: location.zoom,
            },
        )
        .await?;
        Ok(())
    }
}
