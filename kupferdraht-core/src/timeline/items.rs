// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Timeline read API (Step 6-II-a).
//!
//! Public types and traits exposed by `Room::messages`,
//! `Timeline::items`, and `Timeline::subscribe`. matrix-rust-sdk
//! analog: `matrix_sdk_ui::timeline::{TimelineItem, TimelineItemKind,
//! TimelineDiff}`.
//!
//! Step 6-II-a only populates the plain-text happy path:
//! `TimelineItemKind::Text { body, reply_to: None, mentions: [] }`
//! with `edited_at_ms = None`, `redacted = false`, `reactions = []`,
//! `read_by = []`. Later sub-steps fill these in:
//!
//! - 6-II-b → `reply_to`
//! - 6-II-c → `edited_at_ms`
//! - 6-II-d → `TimelineItemKind::Redacted` + `redacted` flag
//! - 6-II-e → `reactions`
//! - 6-II-f → `read_by`
//! - 6-II-g → `mentions`

use crate::error::KupferdrahtError;
use crate::storage::{ReactionRecord, TimelineEventRecord};
use crate::timeline::Timeline;

/// Pagination request for [`crate::client::Client::messages`] /
/// [`crate::rooms::Room::messages`]. `limit` caps the number of
/// returned items; `before` is the inclusive upper bound — only
/// items strictly older than the message with this id are returned.
#[derive(Debug, Clone, Default)]
pub struct MessagesOptions {
    /// Maximum number of items to return. `None` = no limit;
    /// `Some(0)` legitimately returns zero items.
    pub limit: Option<u32>,
    /// Cursor: only items strictly older than this `message_id` are
    /// returned. `None` = newest items.
    pub before: Option<String>,
}

impl MessagesOptions {
    /// Convenience: full history, oldest-first.
    #[must_use]
    pub fn all() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with_limit(mut self, limit: u32) -> Self {
        self.limit = Some(limit);
        self
    }

    #[must_use]
    pub fn before(mut self, message_id: &str) -> Self {
        self.before = Some(message_id.to_owned());
        self
    }
}

/// Delivery state of a timeline item. Kupferdraht-specific — Wire has no
/// per-event send-state machine. FFI-friendly (plain fieldless enum),
/// so the `uniffi` derive rides on the core type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum DeliveryStatus {
    /// The row is persisted and its upload is in flight in this process
    /// (Step 97a — persisted BEFORE the upload). A `Sending` row whose
    /// send is no longer in flight (cancelled, or the process restarted)
    /// reads as `Failed` and is resendable.
    Sending,
    /// Sent locally, awaiting the Wire echo.
    Submitted,
    /// Arrived via the sync stream.
    Received,
    /// An outbound send failed (the MLS upload errored). The local row is
    /// kept so the message surfaces as failed instead of vanishing; the
    /// cause is in the returned `KupferdrahtError`. Step 25a.
    Failed,
}

impl DeliveryStatus {
    /// The string persisted in `timeline_events.delivery_status`.
    pub(crate) fn as_db_str(self) -> &'static str {
        match self {
            Self::Sending => "sending",
            Self::Submitted => "submitted",
            Self::Received => "received",
            Self::Failed => "failed",
        }
    }

    /// Parse a persisted string; `None` for an unknown value.
    pub(crate) fn from_db_str(raw: &str) -> Option<Self> {
        match raw {
            "sending" => Some(Self::Sending),
            "submitted" => Some(Self::Submitted),
            "received" => Some(Self::Received),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

/// One entry in a `Room`'s timeline. Stable id is `item_id`
/// (== `GenericMessage.message_id`).
// No `Eq`: `TimelineItemKind::Location` carries `f32` coords (Step 26).
#[derive(Debug, Clone, PartialEq)]
pub struct TimelineItem {
    pub item_id: String,
    pub room_id: String,
    pub sender_user_id: String,
    pub sender_device_id: String,
    pub sent_at_ms: i64,
    pub kind: TimelineItemKind,
    /// Wall-clock at which an inbound `MessageEdit` replaced the
    /// body. `None` for unedited items. Populated in Step 6-II-c.
    pub edited_at_ms: Option<i64>,
    /// Set when an inbound `MessageDelete` retired the item.
    /// Populated in Step 6-II-d.
    pub redacted: bool,
    /// Aggregated reactions per emoji. Populated in Step 6-II-e.
    pub reactions: Vec<ReactionGroup>,
    /// User ids that confirmed `READ` on this item via Wire's
    /// `Confirmation`. Populated in Step 6-II-f.
    pub read_by: Vec<String>,
    /// User ids that confirmed `DELIVERED` on this item via Wire's
    /// `Confirmation`. Populated in Step 25b. (Wire/Kupferdraht send delivery
    /// confirmations in 1:1 only, so this is empty in group rooms.)
    pub delivered_to: Vec<String>,
    /// Whether the item was sent locally or arrived via sync.
    pub delivery_status: DeliveryStatus,
    /// Wall-clock at which this item's ephemeral timer elapses
    /// (Wire's `Ephemeral` wrapper, field 18) — `Some` only for a
    /// self-deleting message; `None` for every ordinary item. The
    /// timer starts at dispatch/send (`sent_at_ms + ttl`); the
    /// row self-redacts locally once it passes (`kind` becomes
    /// [`TimelineItemKind::Redacted`], the stamp stays so an expiry
    /// is distinguishable from a delete). Step 89.
    pub expires_at_ms: Option<i64>,
}

// No `Eq`: the `Location` variant carries `f32` coords (Step 26).
#[derive(Debug, Clone, PartialEq)]
pub enum TimelineItemKind {
    Text {
        body: String,
        /// `Text.quote.quoted_message_id` — Step 6-II-b.
        reply_to: Option<String>,
        /// `Text.mentions` — Step 6-II-g.
        mentions: Vec<Mention>,
    },
    /// Item was deleted via `MessageDelete`. Body is hidden;
    /// metadata stays so consumers can render a tombstone. Step 6-II-d.
    Redacted,
    /// File-attachment reference. Consumer calls `summary.download()`
    /// / `summary.save_to(path)` to materialise plaintext bytes.
    /// Step 11g (ADR-0011). Boxed to keep the enum's stack-size
    /// compact next to the cheap `Text { ... }` variant.
    Asset(Box<crate::timeline::AssetSummary>),
    /// A nudge / "ping" (`Knock`). `hot_knock` marks a repeat nudge.
    /// Step 26.
    Knock { hot_knock: bool },
    /// A shared map point (`Location`). Step 26.
    Location {
        latitude: f32,
        longitude: f32,
        name: Option<String>,
        zoom: Option<i32>,
    },
    /// Folded-shape case snapshot carried by one `CasePayload`
    /// message (Step 72b, ADR-0018). Consumers render a "case
    /// updated" line or hide it; the folded room state comes from
    /// `Room::case()`, not from summing timeline items. Boxed like
    /// `Asset`/`Task` to keep the enum compact.
    Case(Box<crate::timeline::CaseSnapshot>),
    /// Full state of one task (`TaskPayload`, ADR-0014). Boxed to keep
    /// the enum stack-compact next to the cheap `Text` variant. Step 60a.
    Task(Box<crate::timeline::TaskItem>),
    /// Full state of the room's task-list marker carried by one
    /// `TaskListMarker` message (Step 75, ADR-0020). Consumers render
    /// a "marked as task list" system line or hide repeats; the
    /// folded room state comes from `Room::is_task_list()`, not from
    /// summing timeline items. The rows stay visible as the audit
    /// trail of the deliberately forgeable any-member flag.
    TaskListMarker { marked: bool },
}

/// Swap a `kind = "asset"` item's degraded Text placeholder for the
/// real [`TimelineItemKind::Asset`] when its attachments-table row is
/// available — the record alone cannot carry the asset metadata.
/// Redacted items keep their tombstone. Shared by `Client::messages`,
/// the dispatch Append path, and `emit_message_update_at` (Step 65,
/// review S5 — previously only `Client::messages` reconstructed, so
/// live diffs degraded assets to Text).
pub(crate) fn reconstruct_asset_kind(
    item: &mut TimelineItem,
    attachment_rows: &[crate::storage::AttachmentRecord],
    client: &std::sync::Weak<crate::client::Client>,
) {
    if item.redacted {
        return;
    }
    if let Some(attachment) = attachment_rows
        .iter()
        .find(|a| a.message_id == item.item_id)
    {
        let summary = crate::timeline::AssetSummary::from_attachment(
            attachment.clone(),
            item.sender_user_id.clone(),
            client.clone(),
        );
        item.kind = TimelineItemKind::Asset(Box::new(summary));
    }
}

/// JSON payload packed into `timeline_events.body` for a `Location` item —
/// the column doubles as small-payload storage rather than warranting a
/// dedicated table (the coordinates are tiny and self-contained). Step 26.
#[derive(serde::Serialize, serde::Deserialize)]
struct LocationPayload {
    latitude: f32,
    longitude: f32,
    name: Option<String>,
    zoom: Option<i32>,
}

/// Serialize a location into its `timeline_events.body` form. Step 26.
pub(crate) fn location_body(
    latitude: f32,
    longitude: f32,
    name: Option<&str>,
    zoom: Option<i32>,
) -> String {
    serde_json::to_string(&LocationPayload {
        latitude,
        longitude,
        name: name.map(str::to_string),
        zoom,
    })
    .unwrap_or_default()
}

/// Parse a packed location body back into a `TimelineItemKind::Location`;
/// `None` for malformed JSON (caller degrades to a Text fallback). Step 26.
pub(crate) fn location_kind_from_body(body: &str) -> Option<TimelineItemKind> {
    let p: LocationPayload = serde_json::from_str(body).ok()?;
    Some(TimelineItemKind::Location {
        latitude: p.latitude,
        longitude: p.longitude,
        name: p.name,
        zoom: p.zoom,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ReactionGroup {
    pub emoji: String,
    pub senders: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct Mention {
    pub user_id: String,
    pub start: u32,
    pub length: u32,
}

/// Stream-shape diffs emitted to [`TimelineListener::on_update`].
/// Step 6-II-a wires only `Append`; the other variants are placeholders
/// for 6-II-c (Edit), 6-II-d (Delete), and a future Reset on resync.
// No `Eq`: variants carry `TimelineItem`, which carries `f32` (Step 26).
#[derive(Debug, Clone, PartialEq)]
pub enum TimelineDiff {
    /// A new item arrived at the tail of the timeline (outbound
    /// send or inbound receive). Carries the `room_id` so a single
    /// listener can multiplex across rooms.
    Append { room_id: String, item: TimelineItem },
    /// The item at logical index `index` in `room_id`'s timeline
    /// was replaced (Step 6-II-c edit, 6-II-e reaction toggle, …).
    UpdateAt {
        room_id: String,
        index: u32,
        item: TimelineItem,
    },
    /// The item at logical index `index` in `room_id`'s timeline
    /// was removed (Step 6-II-d delete-for-everyone).
    RemoveAt { room_id: String, index: u32 },
    /// Full timeline replacement, e.g. after a paginated history
    /// load or after `Room::leave` + re-join.
    Reset {
        room_id: String,
        items: Vec<TimelineItem>,
    },
}

/// Subscription target for `Timeline::subscribe`. Listeners observe
/// every `TimelineDiff` produced for the room they were registered
/// on (filtering happens inside `Timeline::subscribe`).
///
/// Implementors must be `Send + Sync` because the dispatch happens
/// on the tokio task that ran `send_text` or the polling-loop task
/// that drained an inbound Wire event.
pub trait TimelineListener: Send + Sync {
    fn on_update(&self, diff: TimelineDiff);
}

/// Aggregate a stream of `ReactionRecord`s into the public-API
/// `Vec<ReactionGroup>` shape (one group per distinct emoji,
/// senders deduplicated and ordered by first-occurrence). Used
/// by `Client::messages` and the receive path. Step 6-II-e.
pub(crate) fn group_reactions_by_emoji(
    rows: impl IntoIterator<Item = ReactionRecord>,
) -> Vec<ReactionGroup> {
    let mut groups: Vec<ReactionGroup> = Vec::new();
    for row in rows {
        if let Some(g) = groups.iter_mut().find(|g| g.emoji == row.emoji) {
            if !g.senders.contains(&row.sender_user_id) {
                g.senders.push(row.sender_user_id);
            }
        } else {
            groups.push(ReactionGroup {
                emoji: row.emoji,
                senders: vec![row.sender_user_id],
            });
        }
    }
    groups
}

impl TimelineItem {
    /// Project a stored row into the public-API shape. 6-II-a populates
    /// only the plain-text happy path; later sub-steps will read
    /// additional columns / aggregates.
    /// Build a fully-hydrated item from a record plus its overlay
    /// rows: mentions land in the `Text` kind, `kind = "asset"` rows
    /// are reconstructed into [`TimelineItemKind::Asset`] from the
    /// attachments table (Step 65, review S5/S10 — previously only
    /// `Client::messages` applied these overlays, so live
    /// `TimelineDiff`s degraded assets to Text and dropped mentions).
    /// `client` backs the asset summary's lazy download handle; tests
    /// pass `Weak::new()`.
    pub(crate) fn hydrated(
        record: TimelineEventRecord,
        mentions: &[crate::storage::MentionRecord],
        attachments: &[crate::storage::AttachmentRecord],
        client: &std::sync::Weak<crate::client::Client>,
    ) -> Self {
        let mut item = Self::from_record(record);
        // Asset reconstruction first: a `kind = "asset"` record
        // degrades to Text in `from_record`, and the mention overlay
        // below must not touch that placeholder.
        reconstruct_asset_kind(&mut item, attachments, client);
        if let TimelineItemKind::Text { mentions: slot, .. } = &mut item.kind {
            *slot = mentions
                .iter()
                .filter(|m| m.message_id == item.item_id)
                .map(|m| Mention {
                    user_id: m.user_id.clone(),
                    start: u32::try_from(m.start).unwrap_or(0),
                    length: u32::try_from(m.length).unwrap_or(0),
                })
                .collect();
        }
        item
    }

    pub(crate) fn from_record(record: TimelineEventRecord) -> Self {
        let redacted = record.redacted_at_ms.is_some();
        let kind = if redacted {
            TimelineItemKind::Redacted
        } else if record.kind == "knock" {
            // Step 26: the packed body is the `hot_knock` flag.
            TimelineItemKind::Knock {
                hot_knock: record.body == "true",
            }
        } else if record.kind == "location" {
            // Step 26: the packed body is the JSON coordinates; a
            // malformed payload degrades to the Text fallback below.
            location_kind_from_body(&record.body).unwrap_or_else(|| TimelineItemKind::Text {
                body: record.body.clone(),
                reply_to: record.reply_to_id.clone(),
                mentions: Vec::new(),
            })
        } else if record.kind == "task" {
            // Step 60a: the packed body is the JSON task state; a
            // malformed payload degrades to the Text fallback below.
            crate::timeline::task::task_kind_from_body(&record.body).unwrap_or_else(|| {
                TimelineItemKind::Text {
                    body: record.body.clone(),
                    reply_to: record.reply_to_id.clone(),
                    mentions: Vec::new(),
                }
            })
        } else if record.kind == "task_list_marker" {
            // Step 75: the packed body is the JSON marker state; a
            // malformed payload degrades to the Text fallback below.
            crate::timeline::task::marker_kind_from_body(&record.body).unwrap_or_else(|| {
                TimelineItemKind::Text {
                    body: record.body.clone(),
                    reply_to: record.reply_to_id.clone(),
                    mentions: Vec::new(),
                }
            })
        } else if record.kind == "case" {
            // Step 72b: the packed body is the JSON case snapshot; a
            // malformed payload degrades to the Text fallback below.
            crate::timeline::case::case_kind_from_body(&record.body).unwrap_or_else(|| {
                TimelineItemKind::Text {
                    body: record.body.clone(),
                    reply_to: record.reply_to_id.clone(),
                    mentions: Vec::new(),
                }
            })
        } else if record.kind == "text" {
            TimelineItemKind::Text {
                body: record.body,
                reply_to: record.reply_to_id.clone(),
                mentions: Vec::new(),
            }
        } else {
            // Unknown kinds round-trip through as Text with the body
            // verbatim — keeps forwards-compatibility for content
            // variants we haven't wired yet.
            TimelineItemKind::Text {
                body: record.body,
                reply_to: record.reply_to_id.clone(),
                mentions: Vec::new(),
            }
        };
        Self {
            item_id: record.message_id,
            room_id: record.room_id,
            sender_user_id: record.sender_user_id,
            sender_device_id: record.sender_device_id,
            sent_at_ms: record.sent_at_ms,
            kind,
            edited_at_ms: record.edited_at_ms,
            redacted,
            reactions: Vec::new(),
            read_by: Vec::new(),
            delivered_to: Vec::new(),
            delivery_status: delivery_status_from_record(&record.delivery_status),
            expires_at_ms: record.expires_at_ms,
        }
    }
}

/// Map a persisted `delivery_status` string onto the enum. Unknown
/// values (a DB written by a newer Kupferdraht — the V6 migration anticipates
/// "acknowledged" / "failed", both outbound-side) degrade to `Submitted`
/// with a warning rather than failing the whole room read.
fn delivery_status_from_record(raw: &str) -> DeliveryStatus {
    DeliveryStatus::from_db_str(raw).unwrap_or_else(|| {
        tracing::warn!(
            delivery_status = raw,
            "unknown delivery_status in store; treating as submitted"
        );
        DeliveryStatus::Submitted
    })
}

impl Timeline {
    /// Snapshot of the persisted timeline for this room, oldest-first.
    /// Step 6-II-a — convenience wrapper around
    /// [`Client::messages`] with `MessagesOptions::all()`.
    /// matrix-rust-sdk analog: `Timeline::items`.
    ///
    /// # Errors
    /// Same as [`Client::messages`].
    pub async fn items(&self) -> Result<Vec<TimelineItem>, KupferdrahtError> {
        self.client
            .messages(&self.room_id, &MessagesOptions::all())
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_record(message_id: &str, body: &str, sent_at_ms: i64) -> TimelineEventRecord {
        TimelineEventRecord {
            message_id: message_id.into(),
            room_id: "room@wire.com".into(),
            sender_user_id: "alice@wire.com".into(),
            sender_device_id: "deviceA".into(),
            kind: "text".into(),
            body: body.into(),
            sent_at_ms,
            delivery_status: "submitted".into(),
            reply_to_id: None,
            edited_at_ms: None,
            redacted_at_ms: None,
            expires_at_ms: None,
            ephemeral_ttl_ms: None,
        }
    }

    #[test]
    fn from_record_parses_task_body_into_task_kind() {
        // Step 60a: kind `task` packs the full task state as JSON.
        let item = crate::timeline::TaskItem {
            task_id: "tid-1".into(),
            title: "Prepare deploy".into(),
            description: None,
            assignee: Some("aaaa-bbbb@wire.com".into()),
            due_at_ms: Some(1_752_000_000_000),
            status: crate::timeline::TaskStatus::Open,
            priority: None,
            sort_key: None,
        };
        let mut rec = sample_record("msg-task", "", 7000);
        rec.kind = "task".into();
        rec.body = crate::timeline::task::task_body(&item);
        let parsed = TimelineItem::from_record(rec);
        match parsed.kind {
            TimelineItemKind::Task(t) => assert_eq!(*t, item),
            other => panic!("expected Task kind, got {other:?}"),
        }
    }

    #[test]
    fn from_record_degrades_malformed_task_body_to_text() {
        // Step 60a: a garbled task body must not lose the row — it
        // degrades to Text verbatim (the Location precedent).
        let mut rec = sample_record("msg-task-bad", "{broken", 7000);
        rec.kind = "task".into();
        let parsed = TimelineItem::from_record(rec);
        match parsed.kind {
            TimelineItemKind::Text { body, .. } => assert_eq!(body, "{broken"),
            other => panic!("expected Text degrade, got {other:?}"),
        }
    }

    #[test]
    fn from_record_carries_expires_at_ms_through_live_and_tombstoned_rows() {
        // Step 89: the ephemeral stamp is a decoration on the item,
        // present on the live Text and on the tombstone alike.
        let mut live = sample_record("msg-eph", "blink", 1000);
        live.expires_at_ms = Some(61_000);
        let item = TimelineItem::from_record(live);
        assert_eq!(item.expires_at_ms, Some(61_000));
        assert!(matches!(item.kind, TimelineItemKind::Text { .. }));

        let mut expired = sample_record("msg-eph-2", "", 1000);
        expired.expires_at_ms = Some(61_000);
        expired.redacted_at_ms = Some(61_000);
        let item = TimelineItem::from_record(expired);
        assert_eq!(
            item.expires_at_ms,
            Some(61_000),
            "stamp survives the tombstone"
        );
        assert!(matches!(item.kind, TimelineItemKind::Redacted));

        let plain = TimelineItem::from_record(sample_record("msg-plain", "hi", 1000));
        assert_eq!(plain.expires_at_ms, None);
    }

    #[test]
    fn from_record_emits_redacted_kind_when_redacted_at_ms_is_set() {
        let mut rec = sample_record("msg-deleted", "should not surface", 5000);
        rec.redacted_at_ms = Some(6000);
        let item = TimelineItem::from_record(rec);
        assert!(item.redacted, "redacted flag must be true");
        assert!(matches!(item.kind, TimelineItemKind::Redacted));
    }

    #[test]
    fn from_record_propagates_reply_to_id_into_text_kind() {
        let mut rec = sample_record("msg-reply", "re: hi", 2000);
        rec.reply_to_id = Some("msg-parent".into());
        let item = TimelineItem::from_record(rec);
        match item.kind {
            TimelineItemKind::Text { reply_to, .. } => {
                assert_eq!(reply_to.as_deref(), Some("msg-parent"));
            }
            other => panic!("text record must produce Text, got {other:?}"),
        }
    }

    #[test]
    fn from_record_maps_text_into_text_kind() {
        let item = TimelineItem::from_record(sample_record("msg1", "hi", 1000));
        assert_eq!(item.item_id, "msg1");
        assert_eq!(item.sent_at_ms, 1000);
        assert!(!item.redacted);
        assert!(item.edited_at_ms.is_none());
        match item.kind {
            TimelineItemKind::Text {
                body,
                reply_to,
                mentions,
            } => {
                assert_eq!(body, "hi");
                assert!(reply_to.is_none());
                assert!(mentions.is_empty());
            }
            other => panic!("text record must not produce non-Text kind, got {other:?}"),
        }
    }

    #[test]
    fn from_record_parses_knock_body_into_knock_kind() {
        for (raw, hot) in [("true", true), ("false", false)] {
            let mut rec = sample_record("msg-knock", raw, 1000);
            rec.kind = "knock".into();
            let item = TimelineItem::from_record(rec);
            match item.kind {
                TimelineItemKind::Knock { hot_knock } => assert_eq!(hot_knock, hot),
                other => panic!("knock record must produce Knock, got {other:?}"),
            }
        }
    }

    #[test]
    fn from_record_parses_location_json_body_into_location_kind() {
        let body = location_body(52.52, 13.405, Some("Berlin"), Some(12));
        let mut rec = sample_record("msg-loc", &body, 1000);
        rec.kind = "location".into();
        let item = TimelineItem::from_record(rec);
        match item.kind {
            TimelineItemKind::Location {
                latitude,
                longitude,
                name,
                zoom,
            } => {
                assert!((latitude - 52.52).abs() < f32::EPSILON);
                assert!((longitude - 13.405).abs() < f32::EPSILON);
                assert_eq!(name.as_deref(), Some("Berlin"));
                assert_eq!(zoom, Some(12));
            }
            other => panic!("location record must produce Location, got {other:?}"),
        }
    }

    #[test]
    fn from_record_degrades_malformed_location_body_to_text() {
        let mut rec = sample_record("msg-bad-loc", "{not valid json", 1000);
        rec.kind = "location".into();
        let item = TimelineItem::from_record(rec);
        match item.kind {
            TimelineItemKind::Text { body, .. } => assert_eq!(body, "{not valid json"),
            other => panic!("malformed location body must degrade to Text, got {other:?}"),
        }
    }

    #[test]
    fn location_body_round_trips_without_optional_fields() {
        let body = location_body(0.0, 0.0, None, None);
        let mut rec = sample_record("msg-loc2", &body, 1000);
        rec.kind = "location".into();
        let item = TimelineItem::from_record(rec);
        match item.kind {
            TimelineItemKind::Location { name, zoom, .. } => {
                assert!(name.is_none());
                assert!(zoom.is_none());
            }
            other => panic!("location record must produce Location, got {other:?}"),
        }
    }

    #[test]
    fn delivery_status_round_trips_both_db_strings() {
        for status in [DeliveryStatus::Submitted, DeliveryStatus::Received] {
            assert_eq!(
                DeliveryStatus::from_db_str(status.as_db_str()),
                Some(status),
                "round-trip must be lossless for {status:?}"
            );
        }
    }

    #[test]
    fn from_db_str_returns_none_for_unknown_value() {
        assert_eq!(DeliveryStatus::from_db_str("acknowledged"), None);
        assert_eq!(DeliveryStatus::from_db_str(""), None);
    }

    #[test]
    fn from_record_maps_submitted_and_received_to_enum() {
        let item = TimelineItem::from_record(sample_record("m1", "hi", 1));
        assert_eq!(item.delivery_status, DeliveryStatus::Submitted);

        let mut rec = sample_record("m2", "hi", 2);
        rec.delivery_status = "received".into();
        let item = TimelineItem::from_record(rec);
        assert_eq!(item.delivery_status, DeliveryStatus::Received);
    }

    #[test]
    fn from_record_falls_back_to_submitted_for_unknown_status() {
        // Forward-compat: a DB written by a newer Kupferdraht (the V6
        // migration anticipates "acknowledged" / "failed", both
        // outbound-side) must not fail the room read — degrade to
        // Submitted instead.
        let mut rec = sample_record("m3", "hi", 3);
        rec.delivery_status = "acknowledged".into();
        let item = TimelineItem::from_record(rec);
        assert_eq!(item.delivery_status, DeliveryStatus::Submitted);
    }

    #[test]
    fn messages_options_builder_chains() {
        let opts = MessagesOptions::all().with_limit(20).before("msg5");
        assert_eq!(opts.limit, Some(20));
        assert_eq!(opts.before.as_deref(), Some("msg5"));
    }

    #[test]
    fn messages_options_all_carries_no_limit() {
        assert_eq!(MessagesOptions::all().limit, None);
    }

    #[test]
    fn append_diff_carries_room_id_and_item() {
        let item = TimelineItem::from_record(sample_record("m", "x", 0));
        let diff = TimelineDiff::Append {
            room_id: "r@wire.com".into(),
            item: item.clone(),
        };
        match diff {
            TimelineDiff::Append { room_id, item: got } => {
                assert_eq!(room_id, "r@wire.com");
                assert_eq!(got, item);
            }
            _ => panic!("expected Append"),
        }
    }

    #[test]
    fn hydrated_reconstructs_asset_kind_from_attachment_rows() {
        // Step 65 (review S5): a `kind = "asset"` row must come back
        // as an Asset item when its attachment row is supplied — the
        // live-diff paths previously degraded it to Text.
        let record = TimelineEventRecord {
            message_id: "m1".into(),
            room_id: "r1".into(),
            sender_user_id: "alice@example.test".into(),
            kind: "asset".into(),
            body: "file.pdf".into(),
            delivery_status: "received".into(),
            ..TimelineEventRecord::default()
        };
        let attachment = crate::storage::AttachmentRecord {
            message_id: "m1".into(),
            room_id: "r1".into(),
            mime_type: "application/pdf".into(),
            filename: Some("file.pdf".into()),
            size_bytes: 123,
            ..crate::storage::AttachmentRecord::default()
        };
        let item = TimelineItem::hydrated(
            record,
            &[],
            &[attachment],
            &std::sync::Weak::<crate::client::Client>::new(),
        );
        let TimelineItemKind::Asset(summary) = item.kind else {
            panic!("expected Asset kind, got {:?}", item.kind);
        };
        assert_eq!(summary.mime_type, "application/pdf");
    }

    #[test]
    fn hydrated_carries_mentions_into_the_text_item() {
        // Step 65 (review S10): mentions were persisted but the live
        // Append diff was built with `mentions: []` — subscribers only
        // saw them after a full reload.
        let record = TimelineEventRecord {
            message_id: "m2".into(),
            room_id: "r1".into(),
            sender_user_id: "alice@example.test".into(),
            kind: "text".into(),
            body: "hi @bob".into(),
            delivery_status: "received".into(),
            ..TimelineEventRecord::default()
        };
        let mentions = [crate::storage::MentionRecord {
            message_id: "m2".into(),
            room_id: "r1".into(),
            user_id: "bob-uuid@example.test".into(),
            start: 3,
            length: 4,
        }];
        let item = TimelineItem::hydrated(
            record,
            &mentions,
            &[],
            &std::sync::Weak::<crate::client::Client>::new(),
        );
        let TimelineItemKind::Text { mentions, .. } = item.kind else {
            panic!("expected Text kind, got {:?}", item.kind);
        };
        assert_eq!(mentions.len(), 1);
        assert_eq!(mentions[0].user_id, "bob-uuid@example.test");
    }
}
