// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Inbound `Knock` (nudge) dispatch — Step 26.

use std::sync::Arc;

use crate::client::Client;
use crate::error::KupferdrahtError;
use crate::sync::event_handler::MessageBody;
use crate::sync::service::SyncService;

impl SyncService {
    /// Step 26: dispatch an inbound `Knock` (nudge).
    pub(crate) async fn dispatch_inbound_knock(
        &self,
        client: &Arc<Client>,
        room_id: &str,
        sender_user_id: &str,
        sender_device_id: Option<&str>,
        message_id: &str,
        knock: crate::wire::proto::Knock,
    ) -> Result<(), KupferdrahtError> {
        self.dispatch_inbound_simple(
            client,
            room_id,
            sender_user_id,
            sender_device_id,
            message_id,
            MessageBody::Knock {
                hot_knock: knock.hot_knock,
            },
        )
        .await?;
        Ok(())
    }
}
