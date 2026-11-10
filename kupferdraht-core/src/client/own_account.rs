// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 152 — the own account, live: read again on request, and written by
//! the client's own profile calls. One path for both: the request, then the
//! `self_user` row, then the `Account` the getters read. `team.update` reaches
//! only a team's admins and is not stored (`docs/observations.md` §99), so the
//! embedder reads on request and decides when.

use std::sync::{Arc, PoisonError};

use crate::avatar::AvatarKeys;
use crate::error::KupferdrahtError;
use crate::storage::SelfUserRecord;
use crate::wire::dto::{SelfUser, Team};

use super::{Account, Client};

/// The team the login and a refresh name the same way: `"default"` and an
/// empty icon are none.
pub(super) fn team_icon_key(team: &Team) -> Option<String> {
    team.icon
        .clone()
        .filter(|icon| !icon.is_empty() && icon != "default")
}

/// The own team id of a `/self`, lowercase; an empty `team` is no team.
pub(super) fn own_team_id(self_user: &SelfUser) -> Option<String> {
    self_user
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
        let self_user = self.http.fetch_self(self.api_version, &token).await?;
        let fetched = format!(
            "{}@{}",
            self_user.qualified_id.id, self_user.qualified_id.domain
        );
        if fetched != self.account().user_id().as_str() {
            return Err(KupferdrahtError::BackendRejected(
                "refresh_account: GET /self answered another user".into(),
            ));
        }
        let team = match own_team_id(&self_user) {
            Some(id) => Some((
                id.clone(),
                self.http.get_team(self.api_version, &token, &id).await?,
            )),
            None => None,
        };
        let before = self.account();
        self.apply_own_profile(|row| fill_from_backend(row, &self_user, team.as_ref()))
            .await?;
        let after = self.account();
        if (before.display_name(), before.handle(), before.avatar_keys())
            != (after.display_name(), after.handle(), after.avatar_keys())
        {
            // The own `members()` row follows at the next refresh.
            self.mark_rooms_stale();
        }
        Ok(())
    }

    /// Change the own profile: `change` edits the `self_user` row, which is
    /// written, and the account the getters read is rebuilt from it. A failed
    /// row write leaves the account; a session without a store keeps the
    /// account alone. Callers hold `account_gate` across their request and
    /// this, so two writers cannot lose an update.
    ///
    /// # Errors
    /// `Storage` from reading or writing the row.
    pub(super) async fn apply_own_profile(
        &self,
        change: impl FnOnce(&mut SelfUserRecord),
    ) -> Result<(), KupferdrahtError> {
        let mut row = match self.store_handle() {
            Some(store) => self.current_self_row(store.as_ref()).await?,
            None => row_of(&self.account()),
        };
        change(&mut row);
        if let Some(store) = self.store_handle() {
            store.save_self_user(&row).await?;
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
fn fill_from_backend(
    row: &mut SelfUserRecord,
    self_user: &SelfUser,
    team: Option<&(String, Team)>,
) {
    let avatar = AvatarKeys::from_assets(&self_user.assets);
    row.name.clone_from(&self_user.name);
    row.handle.clone_from(&self_user.handle);
    row.email.clone_from(&self_user.email);
    row.avatar_thumbnail_key = avatar.thumbnail;
    row.avatar_full_key = avatar.full;
    row.team_id = team.map(|(id, _)| id.clone());
    row.team_name = team.map(|(_, team)| team.name.clone());
    row.team_avatar_key = team.and_then(|(_, team)| team_icon_key(team));
}

/// The row an account would be stored as.
pub(super) fn row_of(account: &Arc<Account>) -> SelfUserRecord {
    SelfUserRecord {
        id: account.user_id().as_str().to_string(),
        handle: account.handle().map(str::to_owned),
        name: account.display_name().to_owned(),
        email: account.email().map(str::to_owned),
        team_id: account.team_id().map(str::to_owned),
        team_name: account.team_name().map(str::to_owned),
        avatar_thumbnail_key: account.avatar_keys().thumbnail.clone(),
        avatar_full_key: account.avatar_keys().full.clone(),
        team_avatar_key: account.team_avatar_key().map(str::to_owned),
    }
}
