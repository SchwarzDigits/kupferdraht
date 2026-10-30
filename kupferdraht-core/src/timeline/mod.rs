// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `Timeline` — matrix-rust-sdk-shaped per-room send / receive
//! surface.
//!
//! - Step 4 wired the outgoing-text path (`Timeline::send_text`).
//! - Step 5 / 5d wired incoming-event ingestion + persistence via
//!   `SyncService`.
//! - Step 6-II-a wires the **read** API: `Room::messages`,
//!   `Timeline::items`, `Timeline::subscribe`, plus the
//!   `TimelineItem` / `TimelineDiff` / `TimelineListener` types
//!   that bind everything together.
//!
//! A `Timeline` is produced via `Room::timeline()` and holds an
//! `Arc<Client>` plus the owning room id so `send_text` and the
//! read accessors can drive the same persistence layer without
//! re-walking the room cache.
//!
//! Step 30 decomposes the per-room surface by **interaction**: the
//! `Timeline` + `MessageHandle` types and their core accessors live
//! here; each message type / interaction owns a sibling child module
//! (`text`, `reaction`, `confirmation`, `delete`, `knock`,
//! `location`, `typing`), and the read-side lookups live in
//! `lookup`. Each child module holds that interaction's `Timeline`
//! facade next to its `Client` orchestrator — the same pattern
//! `asset_send` established.

use std::sync::Arc;

use crate::client::Client;

mod asset_receive;
pub(crate) mod asset_send;
pub(crate) mod case;
mod confirmation;
mod delete;
pub(crate) mod ephemeral;
mod items;
mod knock;
mod location;
mod lookup;
mod own_target;
mod pipeline;
mod reaction;
mod resend;
mod resend_payload;
mod send_state;
mod subscription;
pub(crate) mod task;
mod text;
mod typing;

pub use asset_receive::AssetSummary;
pub use asset_send::AssetSource;
pub use case::{CasePatch, CasePriority, CaseSnapshot, CaseState};
pub(crate) use items::group_reactions_by_emoji;
pub(crate) use items::location_body;
pub(crate) use items::location_kind_from_body;
pub(crate) use items::reconstruct_asset_kind;
pub use items::{
    DeliveryStatus, Mention, MessagesOptions, ReactionGroup, TimelineDiff, TimelineItem,
    TimelineItemKind, TimelineListener,
};
pub use resend::ResendSummary;
pub(crate) use send_state::{ContentRow, InFlightSends};
pub use subscription::Subscription;
pub(crate) use subscription::{ListenerRegistry, Unregister};
pub use task::{FoldedTask, RoomTask, TaskDraft, TaskItem, TaskPatch, TaskPriority, TaskStatus};

/// A handle to a freshly-submitted message. Mirrors
/// matrix-rust-sdk's `Timeline::send` return — it does not (yet)
/// expose live edit / redact handles, just the ids needed to
/// correlate the event with later sync activity.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct MessageHandle {
    /// `GenericMessage.message_id` — UUID v4 picked client-side at
    /// send time. Same id Wire echoes back on the sync stream and
    /// the same id the recipient's client surfaces in quote / edit /
    /// reply references.
    pub message_id: String,
    /// Wall-clock at which Kupferdraht finished assembling the row, in
    /// milliseconds since Unix epoch. Independent from Wire's own
    /// `time` field (which is the server's receive time).
    pub sent_at_ms: i64,
}

/// Per-room send surface.
///
/// Cheap to clone (carries an `Arc<Client>` + the room id by
/// value); typically held briefly, sent through, and dropped.
#[derive(Clone)]
pub struct Timeline {
    client: Arc<Client>,
    room_id: String,
}

impl Timeline {
    /// Construct from a `Client` + room id. Crate-private — callers
    /// reach this through `Room::timeline()`.
    pub(crate) fn new(client: Arc<Client>, room_id: String) -> Arc<Self> {
        Arc::new(Self { client, room_id })
    }

    /// Crate-scoped accessor for the held `Arc<Client>`. Used by
    /// sibling timeline child modules to delegate the send paths
    /// into the Client orchestrator.
    pub(crate) fn client_handle(&self) -> &Arc<Client> {
        &self.client
    }

    /// The room id this `Timeline` is bound to. Same string the
    /// owning `Room::id()` returned at construction time.
    #[must_use]
    pub fn room_id(&self) -> &str {
        &self.room_id
    }
}
