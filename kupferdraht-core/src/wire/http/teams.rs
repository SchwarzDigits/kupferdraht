// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Team HTTP surface: `GET /v{N}/teams/{tid}`, the name of the user's own
//! team for the account snapshot at login (Step 143).

use tracing::debug;

use crate::error::KupferdrahtError;
use crate::wire::dto::Team;

use super::HttpClient;

impl HttpClient {
    /// `GET /v{N}/teams/{tid}`. A plain member reads the own team; another
    /// team and an unknown id answer `404 no-team` (live-confirmed, see
    /// `docs/wire/api-notes.md`).
    ///
    /// # Errors
    /// * `InvalidInput` when `team_id` is no single URL path segment; no
    ///   request goes out.
    /// * `BackendRejected` on a non-2xx status, `Network` for a transport
    ///   failure, and a decoding error for a body that is no `Team`.
    pub async fn get_team(
        &self,
        api_version: u32,
        access_token: &str,
        team_id: &str,
    ) -> Result<Team, KupferdrahtError> {
        debug_assert!(!access_token.is_empty(), "access_token must be non-empty");
        let team_id = Self::path_segment(team_id, "get_team team_id")?;
        let url = self.url(&format!("/v{api_version}/teams/{team_id}"))?;
        debug!(%url, "GET teams/{{tid}}");
        let req = self.client.get(url).bearer_auth(access_token);
        self.send_and_decode::<Team>(req, "GET /teams/{tid}").await
    }
}
