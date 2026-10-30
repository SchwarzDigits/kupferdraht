// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Inbound `Reaction` dispatch — Step 6-II-e.

use std::sync::Arc;

use tracing::{info, warn};

use crate::client::Client;
use crate::error::KupferdrahtError;
use crate::sync::service::SyncService;
use crate::time::now_ms;

impl SyncService {
    /// Apply an inbound `Reaction` to the parent row's reaction
    /// set. Step 6-II-e.
    ///
    /// Wire's payload is the SENDER's full set of emojis after the
    /// toggle, comma-separated; receivers parse the CSV and replace
    /// the sender's stored set in full. Empty `emoji` clears all of
    /// the sender's reactions on the parent.
    ///
    /// The parent lookup is chain-aware: matches against the row's
    /// stable PK (`message_id`) or the chain pointer
    /// (`current_edit_event_id`). Reactions are stored under the
    /// stable PK so per-message aggregation stays consistent across
    /// edit chains.
    pub(crate) async fn dispatch_inbound_reaction(
        &self,
        client: &Arc<Client>,
        room_id: &str,
        sender_user_id: &str,
        reaction_event_id: &str,
        reaction: crate::wire::proto::Reaction,
    ) -> Result<(), KupferdrahtError> {
        let target = reaction.message_id;
        let emojis = parse_reaction_emoji_csv(&reaction.emoji.unwrap_or_default());
        info!(
            reaction_event_id,
            target,
            emoji_count = emojis.len(),
            "Reaction received"
        );
        let Some(store) = client.store_handle() else {
            return Ok(());
        };
        // Reactions are stored under the row's stable PK; Wire-Web
        // may target any chain link.
        let Some(canonical_id) = client.canonical_message_id(room_id, &target).await? else {
            warn!(
                reaction_event_id,
                target, "Reaction had no effect (parent missing locally)"
            );
            return Ok(());
        };
        store
            .replace_user_reactions(room_id, &canonical_id, sender_user_id, &emojis, now_ms())
            .await?;
        client.emit_message_update_at(room_id, &canonical_id).await
    }
}

/// Parse the comma-separated emoji string carried in
/// `Reaction.emoji`. Empty strings, single trailing commas, and
/// stray whitespace are tolerated — Wire counterparts emit slightly
/// varied shapes here, and the parse is meant to read all of them
/// rather than reject on whitespace. Step 6-II-e.
fn parse_reaction_emoji_csv(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod reaction_csv_tests {
    use super::parse_reaction_emoji_csv;

    #[test]
    fn empty_string_means_no_reactions() {
        assert!(parse_reaction_emoji_csv("").is_empty());
    }

    #[test]
    fn single_emoji_round_trips() {
        assert_eq!(parse_reaction_emoji_csv("👍"), vec!["👍".to_string()]);
    }

    #[test]
    fn multi_emoji_csv_splits_and_trims() {
        assert_eq!(
            parse_reaction_emoji_csv("👍, ❤️ ,🎉"),
            vec!["👍".to_string(), "❤️".to_string(), "🎉".to_string()]
        );
    }

    #[test]
    fn trailing_or_double_commas_are_dropped() {
        assert_eq!(
            parse_reaction_emoji_csv("👍,,❤️,"),
            vec!["👍".to_string(), "❤️".to_string()]
        );
    }
}
