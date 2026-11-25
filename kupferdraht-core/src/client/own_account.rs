// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 152 — the own account, live: read again on request, and written by
//! the client's own profile calls. One path for both: the request, then the
//! own user's row, then the `Account` the getters read. `team.update` reaches
//! only a team's admins and is not stored (`docs/observations.md` §99), so the
//! embedder reads on request and decides when.

use std::sync::{Arc, PoisonError};

use crate::avatar::AvatarSource;
use crate::error::KupferdrahtError;
use crate::storage::OwnUserRecord;
use crate::wire::dto::{SelfUser, Team};

use super::{Account, Client};

/// The version marker of a team's icon, the login's and a refresh's alike:
/// `"default"` and an empty icon are none.
pub(super) fn team_avatar_version(team: &Team) -> Option<String> {
    team.icon
        .clone()
        .filter(|icon| !icon.is_empty() && icon != "default")
}

/// The own team id of a `/self`, lowercase; an empty `team` is no team.
pub(super) fn own_team_id(own_user: &SelfUser) -> Option<String> {
    own_user
        .team
        .as_deref()
        .filter(|team| !team.is_empty())
        .map(str::to_ascii_lowercase)
}

impl Client {
    /// Read the own account and team again: `GET /self`, then
    /// `GET /teams/{tid}` when `/self` names a team. Both answers are in
    /// hand before anything is written, so a failure leaves the row and the
    /// getters as they were. matrix-rust-sdk's nearest is
    /// `Account::fetch_user_profile`.
    ///
    /// # Errors
    /// The requests' errors, `BackendRejected` for a `/self` of another user,
    /// and `Storage` from the row write.
    pub async fn refresh_account(&self) -> Result<(), KupferdrahtError> {
        let _gate = self.account_gate.lock().await;
        let token = self.current_access_token();
        let own_user = self.http.fetch_self(self.api_version, &token).await?;
        let fetched = format!(
            "{}@{}",
            own_user.qualified_id.id, own_user.qualified_id.domain
        );
        if fetched != self.account().user_id().as_str() {
            return Err(KupferdrahtError::BackendRejected(
                "refresh_account: GET /self answered another user".into(),
            ));
        }
        let team = match own_team_id(&own_user) {
            Some(id) => Some((
                id.clone(),
                self.http.get_team(self.api_version, &token, &id).await?,
            )),
            None => None,
        };
        let before = self.account();
        self.apply_own_profile(|row| fill_from_backend(row, &own_user, team.as_ref()))
            .await?;
        let after = self.account();
        if (
            before.display_name(),
            before.username(),
            before.avatar_source(),
        ) != (
            after.display_name(),
            after.username(),
            after.avatar_source(),
        ) {
            // The own `members()` row follows at the next refresh.
            self.mark_rooms_stale();
        }
        Ok(())
    }

    /// Change the own profile: `change` edits the own user's row, which is
    /// written, and the account the getters read is rebuilt from it. A failed
    /// row write leaves the account; a session without a store keeps the
    /// account alone. Callers hold `account_gate` across their request and
    /// this, so two writers cannot lose an update.
    ///
    /// # Errors
    /// `Storage` from reading or writing the row.
    pub(super) async fn apply_own_profile(
        &self,
        change: impl FnOnce(&mut OwnUserRecord),
    ) -> Result<(), KupferdrahtError> {
        let mut row = match self.store_handle() {
            Some(store) => self.current_self_row(store.as_ref()).await?,
            None => row_of(&self.account()),
        };
        change(&mut row);
        if let Some(store) = self.store_handle() {
            store.save_own_user(&row).await?;
        }
        let account = super::resume::account_from_row(&row)?;
        debug_assert_eq!(
            account.user_id(),
            self.account().user_id(),
            "the own account keeps its user"
        );
        *self.account.write().unwrap_or_else(PoisonError::into_inner) = account;
        Ok(())
    }
}

/// Every value of the row from what the backend answered.
fn fill_from_backend(row: &mut OwnUserRecord, own_user: &SelfUser, team: Option<&(String, Team)>) {
    row.avatar_source = AvatarSource::from_assets(&own_user.assets);
    row.name.clone_from(&own_user.name);
    row.username.clone_from(&own_user.handle);
    row.email.clone_from(&own_user.email);
    row.team_id = team.map(|(id, _)| id.clone());
    row.team_name = team.map(|(_, team)| team.name.clone());
    row.team_avatar_version = team.and_then(|(_, team)| team_avatar_version(team));
    row.supported_protocols
        .clone_from(&own_user.supported_protocols);
}

/// The row an account would be stored as.
pub(super) fn row_of(account: &Arc<Account>) -> OwnUserRecord {
    OwnUserRecord {
        id: account.user_id().as_str().to_string(),
        username: account.username().map(str::to_owned),
        name: account.display_name().to_owned(),
        email: account.email().map(str::to_owned),
        team_id: account.team_id().map(str::to_owned),
        team_name: account.team_name().map(str::to_owned),
        avatar_source: account.avatar_source().clone(),
        team_avatar_version: account.team_avatar_version().map(str::to_owned),
        supported_protocols: account.supported_protocols().to_vec(),
    }
}
