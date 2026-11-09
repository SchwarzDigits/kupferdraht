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
        origin: super::InboundOrigin<'_>,
        knock: crate::wire::proto::Knock,
    ) -> Result<(), KupferdrahtError> {
        self.dispatch_inbound_simple(
            client,
            origin,
            MessageBody::Knock {
                hot_knock: knock.hot_knock,
            },
        )
        .await?;
        Ok(())
    }
}
