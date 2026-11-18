// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Inbound ping dispatch — Step 26.

use std::sync::Arc;

use crate::client::Client;
use crate::error::KupferdrahtError;
use crate::sync::event_handler::MessageBody;
use crate::sync::service::SyncService;

impl SyncService {
    /// Step 26: dispatch an inbound ping (Wire's `Knock`).
    pub(crate) async fn dispatch_inbound_ping(
        &self,
        client: &Arc<Client>,
        origin: super::InboundOrigin<'_>,
        ping: crate::wire::proto::Knock,
    ) -> Result<(), KupferdrahtError> {
        self.dispatch_inbound_simple(
            client,
            origin,
            MessageBody::Ping {
                repeated: ping.hot_knock,
            },
        )
        .await?;
        Ok(())
    }
}
