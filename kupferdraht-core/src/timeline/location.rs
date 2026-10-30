// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Shared map points (`Location` `GenericMessage`) — Step 26, split
//! out of the former monolithic `timeline/send.rs` by Step 30.
//!
//! Coordinates are persisted as a JSON `timeline_events.body` of kind
//! `location` (`timeline::location_body`); non-finite coordinates are
//! rejected at the consumer boundary because they do not survive the
//! JSON round-trip.

use crate::client::Client;
use crate::error::KupferdrahtError;
use crate::timeline::{MessageHandle, Timeline};

impl Timeline {
    /// Share a `Location` (map point) with this room. `name` is an
    /// optional human label, `zoom` an optional map zoom level. Step 26.
    ///
    /// # Errors
    /// Same downstream errors as [`Self::send_text`] (group resolve,
    /// encode, MLS encrypt, transport, storage).
    pub async fn send_location(
        &self,
        latitude: f32,
        longitude: f32,
        name: Option<String>,
        zoom: Option<i32>,
    ) -> Result<MessageHandle, KupferdrahtError> {
        self.client
            .send_location(&self.room_id, latitude, longitude, name.as_deref(), zoom)
            .await
    }
}

impl Client {
    /// See [`Timeline::send_location`] for the caller-facing contract. Step 26.
    pub(crate) async fn send_location(
        &self,
        room_id: &str,
        latitude: f32,
        longitude: f32,
        name: Option<&str>,
        zoom: Option<i32>,
    ) -> Result<MessageHandle, KupferdrahtError> {
        debug_assert!(
            !room_id.is_empty(),
            "send_location: room_id must be non-empty"
        );
        // Non-finite coordinates do not survive the JSON persistence
        // round-trip (serde_json emits `null` for NaN/Inf, which fails to
        // decode back into f32). Reject them at the consumer boundary.
        if !latitude.is_finite() || !longitude.is_finite() {
            return Err(KupferdrahtError::InvalidInput(format!(
                "send_location: coordinates must be finite (lat={latitude}, lon={longitude})"
            )));
        }
        let group_id = self.resolve_room_group_id(room_id, "send_location").await?;
        let (message_id, plaintext) =
            crate::wire::proto::build_location_generic_message(latitude, longitude, name, zoom)?;
        // Step 97a: the row is written as `Sending` before the upload.
        let body = crate::timeline::location_body(latitude, longitude, name, zoom);
        let sent_at_ms = self
            .send_content_row(
                room_id,
                &group_id,
                "send_location",
                crate::timeline::ContentRow {
                    message_id: &message_id,
                    plaintext: &plaintext,
                    kind: "location",
                    body: &body,
                },
            )
            .await?;
        Ok(MessageHandle {
            message_id,
            sent_at_ms,
        })
    }
}
