// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `Client` — timeline read + listener fan-out.
//!
//! Split out of [`super`] in Step 29: the paginated history reader
//! ([`Client::messages`]) with its row-pagination + overlay free
//! helpers ([`paginate_timeline_rows`], [`apply_timeline_overlays`]),
//! plus timeline-listener registration ([`Client::add_timeline_listener`])
//! and dispatch (`dispatch_timeline_diff`). Step 6-II / Step 14f.

#![allow(clippy::doc_markdown)]

use std::sync::Arc;

use crate::error::KupferdrahtError;

use super::Client;

/// Apply `MessagesOptions` to a freshly-loaded row set: truncate
/// older than `before` (exclusive), then trim to the newest `limit`
/// rows. `limit == None` means no limit. Used by `Client::messages`.
fn paginate_timeline_rows(
    mut rows: Vec<crate::storage::TimelineEventRecord>,
    opts: &crate::timeline::MessagesOptions,
) -> Vec<crate::storage::TimelineEventRecord> {
    if let Some(cursor) = opts.before.as_deref()
        && let Some(idx) = rows.iter().position(|r| r.message_id == cursor)
    {
        rows.truncate(idx);
    }
    if let Some(limit) = opts.limit
        && rows.len() > limit as usize
    {
        let drop_n = rows.len() - limit as usize;
        rows.drain(..drop_n);
    }
    rows
}

/// Layer reactions / read-receipts / mentions onto an already-built
/// `TimelineItem`. Called per item by `Client::messages` after the
/// per-room bulk loads.
fn apply_timeline_overlays(
    item: &mut crate::timeline::TimelineItem,
    reaction_rows: &[crate::storage::ReactionRecord],
    receipt_rows: &[crate::storage::ReadReceiptRecord],
    delivery_rows: &[crate::storage::DeliveryReceiptRecord],
    mention_rows: &[crate::storage::MentionRecord],
) {
    let item_reactions = reaction_rows
        .iter()
        .filter(|r| r.message_id == item.item_id)
        .cloned();
    item.reactions = crate::timeline::group_reactions_by_emoji(item_reactions);
    let mut readers: Vec<String> = receipt_rows
        .iter()
        .filter(|r| r.message_id == item.item_id)
        .map(|r| r.reader_user_id.clone())
        .collect();
    readers.sort();
    readers.dedup();
    item.read_by = readers;
    let mut confirmers: Vec<String> = delivery_rows
        .iter()
        .filter(|r| r.message_id == item.item_id)
        .map(|r| r.confirmer_user_id.clone())
        .collect();
    confirmers.sort();
    confirmers.dedup();
    item.delivered_to = confirmers;
    let mentions: Vec<crate::timeline::Mention> = mention_rows
        .iter()
        .filter(|m| m.message_id == item.item_id)
        .map(|m| crate::timeline::Mention {
            user_id: m.user_id.clone(),
            start: u32::try_from(m.start).unwrap_or(0),
            length: u32::try_from(m.length).unwrap_or(0),
        })
        .collect();
    // Mentions only attach to Text variants — Redacted items have no body.
    if let crate::timeline::TimelineItemKind::Text {
        body,
        reply_to,
        mentions: existing,
    } = &mut item.kind
    {
        let _ = existing;
        item.kind = crate::timeline::TimelineItemKind::Text {
            body: std::mem::take(body),
            reply_to: reply_to.take(),
            mentions,
        };
    }
}

// Step 65: the asset-kind reconstruction (Step 35) moved to
// `timeline::items` so the dispatch Append path and
// `emit_message_update_at` share it — this module (and its tests
// below) use the shared implementation.
use crate::timeline::reconstruct_asset_kind;

impl Client {
    /// Per-room unread counts in one grouped query (Step 67) — the
    /// room-list companion to [`crate::rooms::Room::unread_count`],
    /// built for list UIs (no per-room n+1). Rooms without unread
    /// messages are absent from the map; treat missing as `0`.
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` on read failure.
    pub async fn unread_counts(
        &self,
    ) -> Result<std::collections::HashMap<String, u32>, KupferdrahtError> {
        let Some(store) = self.store_handle() else {
            return Ok(std::collections::HashMap::new());
        };
        let self_id = self
            .user_id()
            .map(|u| u.as_str().to_string())
            .ok_or_else(|| {
                KupferdrahtError::InvalidInput("unread_counts: no authenticated user".into())
            })?;
        store.unread_counts(&self_id).await
    }

    /// Fold the room's persisted case rows into the current case
    /// state (Step 72b, ADR-0018): admin-authorized snapshots
    /// last-writer-win by `revision`, non-admin rows contribute
    /// display-only `extensions`; redacted rows drop out. Fold-on-read
    /// — no materialized cache to go stale, no redaction rebuild hook.
    /// The authorization flag was captured at receive time and rides
    /// the row body (`sender_was_admin`).
    pub(crate) async fn folded_case(
        &self,
        room_id: &str,
    ) -> Result<Option<crate::timeline::CaseSnapshot>, KupferdrahtError> {
        let store = self.store_handle().ok_or_else(|| {
            KupferdrahtError::InvalidInput(
                "case: no Store attached — set ClientBuilder::base_dir(...) first".into(),
            )
        })?;
        let rows = store.load_timeline_events(room_id).await?;
        let mut state: Option<crate::timeline::CaseSnapshot> = None;
        for row in &rows {
            if row.kind != "case" || row.redacted_at_ms.is_some() {
                continue;
            }
            let Some((snapshot, sender_was_admin)) =
                crate::timeline::case::parse_case_body(&row.body)
            else {
                continue;
            };
            state = crate::timeline::case::apply_case_event(state, &snapshot, sender_was_admin);
        }
        Ok(state)
    }

    /// Read paginated message history for a room from the local
    /// `timeline_events` store. Step 6-II-a. matrix-rust-sdk analog:
    /// `Room::messages` (Kupferdraht's equivalent reads only from the
    /// already-persisted local cache; pagination across server-side
    /// history lands when Wire's notifications backfill is wired in
    /// a later step).
    ///
    /// `opts.limit == None` is "no limit" (return everything).
    /// `opts.before == Some(message_id)` returns only items strictly
    /// older than the message with that id; `None` returns the
    /// newest items. Returned `Vec` is **oldest-first**, matching
    /// matrix-rust-sdk's `MessagesResult.chunk` shape.
    ///
    /// # Errors
    /// * `InvalidInput` if no `Store` is attached.
    /// * `Storage` from the SQL read.
    pub async fn messages(
        &self,
        room_id: &str,
        opts: &crate::timeline::MessagesOptions,
    ) -> Result<Vec<crate::timeline::TimelineItem>, KupferdrahtError> {
        let store = self.store_handle().ok_or_else(|| {
            KupferdrahtError::InvalidInput(
                "messages: no Store attached — set ClientBuilder::base_dir(...) first".into(),
            )
        })?;
        let rows = paginate_timeline_rows(store.load_timeline_events(room_id).await?, opts);
        let mut items: Vec<crate::timeline::TimelineItem> = rows
            .into_iter()
            .map(crate::timeline::TimelineItem::from_record)
            .collect();
        // 6-II-e/f/g: load all overlay row-sets for the room with
        // one DB call each, then attach per item below.
        let reaction_rows = store.load_reactions_for_room(room_id).await?;
        let receipt_rows = store.load_read_receipts_for_room(room_id).await?;
        let delivery_rows = store.load_delivery_receipts_for_room(room_id).await?;
        let mention_rows = store.load_mentions_for_room(room_id).await?;
        // Asset rows persist `kind = "asset"` + a row in the separate
        // `attachments` table; `from_record` (pure) cannot reach it and degrades
        // them to Text. Reconstruct the Asset kind here from the attachment,
        // mirroring how the other overlays hydrate the item. Step 35.
        let attachment_rows = store.list_attachments_for_room(room_id).await?;
        let weak = self.weak();
        for item in &mut items {
            apply_timeline_overlays(
                item,
                &reaction_rows,
                &receipt_rows,
                &delivery_rows,
                &mention_rows,
            );
            reconstruct_asset_kind(item, &attachment_rows, &weak);
        }
        // Step 97a: an abandoned `Sending` row reads as `Failed`.
        self.settle_items(&mut items);
        Ok(items)
    }

    /// Register a timeline listener. Listener fires for every
    /// `TimelineDiff` the client emits — outbound sends and inbound
    /// receives, on every room. The listener is responsible for
    /// filtering by `diff.room_id` if it cares about a single room
    /// (`Timeline::subscribe` wraps this with a per-room filter).
    /// matrix-rust-sdk analog: a flavor of `Timeline::subscribe`.
    ///
    /// Step 14f — returns a [`crate::timeline::Subscription`] handle
    /// whose `Drop` impl removes the listener from the registry, or
    /// whose `unsubscribe()` method makes the action explicit at the
    /// call site. Holding the `Subscription` keeps the listener
    /// alive; discarding it (without binding) drops it immediately
    /// and silently unregisters — the `#[must_use]` attribute on
    /// `Subscription` warns about this.
    ///
    /// # Panics
    /// Panics if the internal listener-registry mutex has been
    /// poisoned by a previous registration panic. Practically
    /// unreachable.
    pub fn add_timeline_listener(
        &self,
        listener: Arc<dyn crate::timeline::TimelineListener>,
    ) -> crate::timeline::Subscription {
        let id = self
            .timeline_listener_next_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.timeline_listeners
            .lock()
            .expect("timeline_listeners mutex poisoned")
            .insert(id, listener);
        let weak: std::sync::Weak<crate::timeline::ListenerRegistry> =
            Arc::downgrade(&self.timeline_listeners);
        crate::timeline::Subscription { id, registry: weak }
    }

    /// Crate-private dispatch helper. Snapshots the listener map's
    /// values, then invokes `on_update(diff.clone())` on each. Same
    /// snapshot-and-dispatch shape as `SyncService::notify_message`;
    /// the mutex is released before the listener callbacks run so
    /// a listener that triggers a re-entrant
    /// `add_timeline_listener` / `Subscription::drop` does not
    /// deadlock against itself (Step 14f contract).
    pub(crate) fn dispatch_timeline_diff(&self, diff: &crate::timeline::TimelineDiff) {
        // Step 109 (ADR-0037): inside a drained page, held until it commits.
        if crate::sync::outbox::hold(crate::sync::outbox::Held::Timeline(diff.clone())).is_some() {
            self.emit_timeline_diff(diff);
        }
    }

    /// Hand `diff` to the timeline listeners now.
    pub(crate) fn emit_timeline_diff(&self, diff: &crate::timeline::TimelineDiff) {
        let snapshot: Vec<Arc<dyn crate::timeline::TimelineListener>> =
            match self.timeline_listeners.lock() {
                Ok(g) => g.values().cloned().collect(),
                Err(_) => return,
            };
        for listener in &snapshot {
            listener.on_update(diff.clone());
        }
    }
}

#[cfg(test)]
mod paginate_timeline_rows_tests {
    use super::paginate_timeline_rows;
    use crate::storage::TimelineEventRecord;
    use crate::timeline::MessagesOptions;

    fn rows(ids: &[&str]) -> Vec<TimelineEventRecord> {
        ids.iter()
            .map(|id| TimelineEventRecord {
                message_id: (*id).into(),
                room_id: "room@wire.com".into(),
                sender_user_id: "alice@wire.com".into(),
                sender_device_id: "deviceA".into(),
                kind: "text".into(),
                body: "x".into(),
                sent_at_ms: 0,
                delivery_status: "received".into(),
                reply_to_id: None,
                edited_at_ms: None,
                redacted_at_ms: None,
                expires_at_ms: None,
                ephemeral_ttl_ms: None,
            })
            .collect()
    }

    fn ids(rows: &[TimelineEventRecord]) -> Vec<&str> {
        rows.iter().map(|r| r.message_id.as_str()).collect()
    }

    #[test]
    fn limit_none_returns_all_rows() {
        let out = paginate_timeline_rows(rows(&["a", "b", "c"]), &MessagesOptions::all());
        assert_eq!(ids(&out), ["a", "b", "c"]);
    }

    #[test]
    fn limit_some_zero_returns_no_rows() {
        // `Some(0)` is a legitimate "zero items" request — the old
        // 0-as-no-limit sentinel is gone.
        let opts = MessagesOptions::all().with_limit(0);
        let out = paginate_timeline_rows(rows(&["a", "b"]), &opts);
        assert!(out.is_empty(), "Some(0) must return no rows, got {out:?}");
    }

    #[test]
    fn limit_caps_to_newest_rows() {
        let opts = MessagesOptions::all().with_limit(2);
        let out = paginate_timeline_rows(rows(&["a", "b", "c"]), &opts);
        assert_eq!(ids(&out), ["b", "c"], "newest N survive, oldest dropped");
    }

    #[test]
    fn before_cursor_truncates_then_limit_applies() {
        let opts = MessagesOptions::all().with_limit(2).before("d");
        let out = paginate_timeline_rows(rows(&["a", "b", "c", "d"]), &opts);
        assert_eq!(ids(&out), ["b", "c"]);
    }
}

#[cfg(test)]
mod reconstruct_asset_kind_tests {
    use super::reconstruct_asset_kind;
    use crate::storage::{AttachmentRecord, TimelineEventRecord};
    use crate::timeline::{TimelineItem, TimelineItemKind};
    use std::sync::Weak;

    fn asset_record(message_id: &str) -> TimelineEventRecord {
        TimelineEventRecord {
            message_id: message_id.into(),
            room_id: "room@wire.com".into(),
            sender_user_id: "alice@wire.com".into(),
            sender_device_id: "deviceA".into(),
            kind: "asset".into(),
            body: String::new(),
            sent_at_ms: 0,
            delivery_status: "received".into(),
            reply_to_id: None,
            edited_at_ms: None,
            redacted_at_ms: None,
            expires_at_ms: None,
            ephemeral_ttl_ms: None,
        }
    }

    fn attachment(message_id: &str) -> AttachmentRecord {
        AttachmentRecord {
            message_id: message_id.into(),
            room_id: "room@wire.com".into(),
            mime_type: "image/png".into(),
            filename: Some("cat.png".into()),
            size_bytes: 1234,
            sha256: vec![1, 2, 3],
            otr_key_ct: vec![4, 5, 6],
            asset_id: "asset-abc".into(),
            asset_token: Some("tok".into()),
            asset_domain: "wire.com".into(),
            encryption_algo: "AES_GCM".into(),
            cache_path: None,
            created_at_ms: 0,
        }
    }

    #[test]
    fn asset_row_with_attachment_becomes_asset_kind() {
        let mut item = TimelineItem::from_record(asset_record("m-asset"));
        // from_record degrades an asset row to Text (the fallback).
        assert!(matches!(item.kind, TimelineItemKind::Text { .. }));
        reconstruct_asset_kind(&mut item, &[attachment("m-asset")], &Weak::new());
        match &item.kind {
            TimelineItemKind::Asset(summary) => {
                assert_eq!(summary.mime_type, "image/png");
                assert_eq!(summary.filename.as_deref(), Some("cat.png"));
                assert_eq!(summary.size_bytes, 1234);
                assert_eq!(summary.asset_id, "asset-abc");
            }
            other => panic!("expected Asset, got {other:?}"),
        }
    }

    #[test]
    fn text_row_without_attachment_stays_text() {
        let mut rec = asset_record("m-text");
        rec.kind = "text".into();
        rec.body = "hello".into();
        let mut item = TimelineItem::from_record(rec);
        reconstruct_asset_kind(&mut item, &[], &Weak::new());
        match &item.kind {
            TimelineItemKind::Text { body, .. } => assert_eq!(body, "hello"),
            other => panic!("expected Text, got {other:?}"),
        }
    }

    #[test]
    fn redacted_asset_row_stays_redacted() {
        let mut rec = asset_record("m-redacted");
        rec.redacted_at_ms = Some(99);
        let mut item = TimelineItem::from_record(rec);
        assert!(matches!(item.kind, TimelineItemKind::Redacted));
        // A redacted row must not resurrect even with a matching attachment.
        reconstruct_asset_kind(&mut item, &[attachment("m-redacted")], &Weak::new());
        assert!(matches!(item.kind, TimelineItemKind::Redacted));
    }
}
