// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! FFI wrappers for the timeline read API (Step 20c-3).
//!
//! `TimelineItem` / `TimelineItemKind` are **mirrored** here rather than
//! derived on the core types: the core `TimelineItemKind::Asset` holds a
//! `Box<AssetSummary>`, and `AssetSummary` carries crate-private security
//! material (`sha256`, `otr_key_ct`, a `Weak<Client>`) plus behaviour
//! (`download` / `save_to`) — neither of which can be a `uniffi::Record`.
//! The mirror reshapes the asset variant to a flat [`AssetMeta`] record
//! (the public metadata only); the download behaviour lands with the rest
//! of the asset surface in 20c-4.
//!
//! The FFI-shaped leaves (`ReactionGroup`, `Mention`) keep their derives
//! on the core type and are reused directly.

use std::sync::Arc;

use super::Client;
use super::sync::{Subscription, TimelineListener, adapt_timeline_listener};
use crate::error::KupferdrahtError;
use crate::timeline::{
    AssetSource, AssetSummary, FoldedTask as CoreFoldedTask, Mention, MessageHandle,
    MessagesOptions, ResendSummary as CoreResendSummary, TaskDraft as CoreTaskDraft,
    TaskItem as CoreTaskItem, TaskPatch as CoreTaskPatch, TaskPriority as CoreTaskPriority,
    TaskStatus as CoreTaskStatus, Timeline as CoreTimeline, TimelineDiff as CoreDiff,
    TimelineItem as CoreItem, TimelineItemKind as CoreKind,
};

/// The hard error a [`ResendSummary`] pass stopped at. Step 79.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FailedResend {
    pub message_id: String,
    pub error: String,
}

/// Outcome of `Timeline::resend_failed` — mirrored because the core
/// type carries a `(String, String)` tuple `UniFFI` can't marshal
/// (the `JoinSummary` precedent). Step 79.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ResendSummary {
    /// Rows re-driven successfully, oldest first.
    pub resent: Vec<MessageHandle>,
    /// Failed rows `resend` does not cover (task / case / marker
    /// snapshots) — left `Failed`; re-send the current state instead.
    pub skipped: Vec<String>,
    /// The hard error the pass stopped at; `None` when every attempted
    /// row went through.
    pub stopped_at: Option<FailedResend>,
    /// Failed rows behind `stopped_at` that were not attempted.
    pub remaining: u32,
}

impl From<&CoreResendSummary> for ResendSummary {
    fn from(summary: &CoreResendSummary) -> Self {
        Self {
            resent: summary.resent.clone(),
            skipped: summary.skipped.clone(),
            stopped_at: summary
                .stopped_at
                .as_ref()
                .map(|(message_id, error)| FailedResend {
                    message_id: message_id.clone(),
                    error: error.clone(),
                }),
            remaining: summary.remaining,
        }
    }
}

/// Public metadata of an asset attached to a message — the read-only view
/// embedded in `TimelineItemKind::Asset` / `MessageBody::Asset`.
///
/// To fetch the bytes, look the asset up as a downloadable [`Asset`]
/// object via `Timeline.asset(message_id)`; the timeline item itself
/// carries only this metadata (object references can't sit in enum
/// variants).
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct AssetMeta {
    pub message_id: String,
    pub mime_type: String,
    pub filename: Option<String>,
    pub size_bytes: u64,
    pub sender_user_id: String,
    pub room_id: String,
    pub asset_id: String,
    pub asset_token: Option<String>,
    pub asset_domain: String,
    pub encryption_algo: String,
    pub cache_path: Option<String>,
}

impl std::fmt::Debug for AssetMeta {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Step 101: secrets redacted (`crate::redact`).
        f.debug_struct("AssetMeta")
            .field("message_id", &self.message_id)
            .field("mime_type", &self.mime_type)
            .field("filename", &self.filename)
            .field("size_bytes", &self.size_bytes)
            .field("sender_user_id", &self.sender_user_id)
            .field("room_id", &self.room_id)
            .field("asset_id", &self.asset_id)
            .field(
                "asset_token",
                &crate::redact::hidden_option(self.asset_token.as_ref()),
            )
            .field("asset_domain", &self.asset_domain)
            .field("encryption_algo", &self.encryption_algo)
            .field("cache_path", &self.cache_path)
            .finish()
    }
}

impl From<&AssetSummary> for AssetMeta {
    fn from(a: &AssetSummary) -> Self {
        Self {
            message_id: a.message_id.clone(),
            mime_type: a.mime_type.clone(),
            filename: a.filename.clone(),
            size_bytes: a.size_bytes,
            sender_user_id: a.sender_user_id.clone(),
            room_id: a.room_id.clone(),
            asset_id: a.asset_id.clone(),
            asset_token: a.asset_token.clone(),
            asset_domain: a.asset_domain.clone(),
            encryption_algo: a.encryption_algo.clone(),
            cache_path: a.cache_path.clone(),
        }
    }
}

/// Task lifecycle state (ADR-0014). Unknown wire values already
/// degrade to `Open` in core — this mirror only ever sees the four
/// known states. Step 60a.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum TaskStatus {
    Open,
    InProgress,
    Done,
    Cancelled,
}

impl From<CoreTaskStatus> for TaskStatus {
    fn from(s: CoreTaskStatus) -> Self {
        match s {
            CoreTaskStatus::Open => Self::Open,
            CoreTaskStatus::InProgress => Self::InProgress,
            CoreTaskStatus::Done => Self::Done,
            CoreTaskStatus::Cancelled => Self::Cancelled,
        }
    }
}

/// Optional task priority; absent = no priority. Step 60a.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum TaskPriority {
    Low,
    Medium,
    High,
    Urgent,
}

impl From<CoreTaskPriority> for TaskPriority {
    fn from(p: CoreTaskPriority) -> Self {
        match p {
            CoreTaskPriority::Low => Self::Low,
            CoreTaskPriority::Medium => Self::Medium,
            CoreTaskPriority::High => Self::High,
            CoreTaskPriority::Urgent => Self::Urgent,
        }
    }
}

fn priority_to_core(p: TaskPriority) -> CoreTaskPriority {
    match p {
        TaskPriority::Low => CoreTaskPriority::Low,
        TaskPriority::Medium => CoreTaskPriority::Medium,
        TaskPriority::High => CoreTaskPriority::High,
        TaskPriority::Urgent => CoreTaskPriority::Urgent,
    }
}

fn status_to_core(s: TaskStatus) -> CoreTaskStatus {
    match s {
        TaskStatus::Open => CoreTaskStatus::Open,
        TaskStatus::InProgress => CoreTaskStatus::InProgress,
        TaskStatus::Done => CoreTaskStatus::Done,
        TaskStatus::Cancelled => CoreTaskStatus::Cancelled,
    }
}

/// A partial change to one task (Step 60b) — every set field is
/// applied onto the current folded state; unset fields keep their
/// value. Set-only: clearing an optional field is not expressible
/// (see the core `TaskPatch` doc).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct TaskPatch {
    pub title: Option<String>,
    pub description: Option<String>,
    /// Qualified user id (`<uuid>@<domain>`).
    pub assignee: Option<String>,
    pub due_at_ms: Option<i64>,
    pub priority: Option<TaskPriority>,
    /// Any transition is legal — `Done → Open` reopens.
    pub status: Option<TaskStatus>,
    pub sort_key: Option<String>,
}

impl TaskPatch {
    fn into_core(self) -> CoreTaskPatch {
        CoreTaskPatch {
            title: self.title,
            description: self.description,
            assignee: self.assignee,
            due_at_ms: self.due_at_ms,
            priority: self.priority.map(priority_to_core),
            status: self.status.map(status_to_core),
            sort_key: self.sort_key,
        }
    }
}

/// One task after folding its event log (Step 60b): the latest full
/// state plus fold metadata (creator, timestamps, carrying message).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FoldedTask {
    pub task: TaskMeta,
    /// Sender of the task's first message (qualified user id).
    pub creator_user_id: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    /// `message_id` of the message that carried the current state.
    pub latest_message_id: String,
}

impl From<&CoreFoldedTask> for FoldedTask {
    fn from(f: &CoreFoldedTask) -> Self {
        Self {
            task: TaskMeta::from(&f.item),
            creator_user_id: f.creator_user_id.clone(),
            created_at_ms: f.created_at_ms,
            updated_at_ms: f.updated_at_ms,
            latest_message_id: f.latest_message_id.clone(),
        }
    }
}

/// One task with the room it lives in — the element of the
/// cross-room views `Client::my_tasks` / `Client::delegated_tasks`
/// (Step 60c, ADR-0014).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct RoomTask {
    /// Qualified room id (`<uuid>@<domain>`).
    pub room_id: String,
    pub task: FoldedTask,
}

impl From<&crate::timeline::RoomTask> for RoomTask {
    fn from(t: &crate::timeline::RoomTask) -> Self {
        Self {
            room_id: t.room_id.clone(),
            task: FoldedTask::from(&t.task),
        }
    }
}

/// Full state of one task — the read-only view embedded in
/// `TimelineItemKind::Task` / `MessageBody::Task` (ADR-0014,
/// Step 60a). Named alongside [`AssetMeta`].
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct TaskMeta {
    /// Stable identity (UUID) — deliberately NOT the `message_id`.
    pub task_id: String,
    pub title: String,
    pub description: Option<String>,
    /// Qualified user id (`<uuid>@<domain>`).
    pub assignee: Option<String>,
    pub due_at_ms: Option<i64>,
    pub status: TaskStatus,
    pub priority: Option<TaskPriority>,
    /// Lexicographic fractional index — consumed in Step 60c.
    pub sort_key: Option<String>,
}

impl From<&CoreTaskItem> for TaskMeta {
    fn from(t: &CoreTaskItem) -> Self {
        Self {
            task_id: t.task_id.clone(),
            title: t.title.clone(),
            description: t.description.clone(),
            assignee: t.assignee.clone(),
            due_at_ms: t.due_at_ms,
            status: t.status.into(),
            priority: t.priority.map(TaskPriority::from),
            sort_key: t.sort_key.clone(),
        }
    }
}

/// Consumer-facing input for a new task (ADR-0014, Step 60a). The
/// library generates the stable `task_id` and stamps `status: Open`.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct TaskDraft {
    pub title: String,
    /// Free text; links are plain text inside it.
    pub description: Option<String>,
    /// Qualified user id (`<uuid>@<domain>`) of the assignee.
    pub assignee: Option<String>,
    pub due_at_ms: Option<i64>,
    pub priority: Option<TaskPriority>,
}

impl TaskDraft {
    fn into_core(self) -> CoreTaskDraft {
        CoreTaskDraft {
            title: self.title,
            description: self.description,
            assignee: self.assignee,
            due_at_ms: self.due_at_ms,
            priority: self.priority.map(priority_to_core),
        }
    }
}

/// The content of a timeline item.
// No `Eq`: the `Location` variant carries `f32` coords (Step 26).
#[derive(Debug, Clone, PartialEq, uniffi::Enum)]
pub enum TimelineItemKind {
    Text {
        body: String,
        reply_to: Option<String>,
        mentions: Vec<crate::timeline::Mention>,
    },
    Redacted,
    Asset {
        asset: AssetMeta,
    },
    /// A nudge / "ping". Step 26.
    Knock {
        hot_knock: bool,
    },
    /// A shared map point. Step 26.
    Location {
        latitude: f32,
        longitude: f32,
        name: Option<String>,
        zoom: Option<i32>,
    },
    /// Full state of one task (ADR-0014). Step 60a.
    Task {
        task: TaskMeta,
    },
    /// Case snapshot carried by this message (ADR-0018). Step 72b.
    /// `CaseSnapshot` is itself a `uniffi::Record`.
    Case {
        case: crate::timeline::CaseSnapshot,
    },
    /// Task-list-marker state carried by this message (ADR-0020,
    /// Step 75) — render a system line or hide repeats; the folded
    /// room state is `Room::is_task_list()`.
    TaskListMarker {
        marked: bool,
    },
}

impl From<&CoreKind> for TimelineItemKind {
    fn from(kind: &CoreKind) -> Self {
        match kind {
            CoreKind::Text {
                body,
                reply_to,
                mentions,
            } => Self::Text {
                body: body.clone(),
                reply_to: reply_to.clone(),
                mentions: mentions.clone(),
            },
            CoreKind::Redacted => Self::Redacted,
            CoreKind::Asset(summary) => Self::Asset {
                asset: AssetMeta::from(summary.as_ref()),
            },
            CoreKind::Knock { hot_knock } => Self::Knock {
                hot_knock: *hot_knock,
            },
            CoreKind::Location {
                latitude,
                longitude,
                name,
                zoom,
            } => Self::Location {
                latitude: *latitude,
                longitude: *longitude,
                name: name.clone(),
                zoom: *zoom,
            },
            CoreKind::Task(item) => Self::Task {
                task: TaskMeta::from(item.as_ref()),
            },
            CoreKind::Case(snapshot) => Self::Case {
                case: snapshot.as_ref().clone(),
            },
            CoreKind::TaskListMarker { marked } => Self::TaskListMarker { marked: *marked },
        }
    }
}

/// One entry in a room's timeline.
// No `Eq`: `TimelineItemKind::Location` carries `f32` coords (Step 26).
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct TimelineItem {
    pub item_id: String,
    pub room_id: String,
    pub sender_user_id: String,
    pub sender_device_id: String,
    pub sent_at_ms: i64,
    pub kind: TimelineItemKind,
    pub edited_at_ms: Option<i64>,
    pub redacted: bool,
    pub reactions: Vec<crate::timeline::ReactionGroup>,
    pub read_by: Vec<String>,
    pub delivered_to: Vec<String>,
    pub delivery_status: crate::timeline::DeliveryStatus,
    /// Wall-clock (ms since Unix epoch) at which this item's ephemeral
    /// timer elapses — set only for a self-deleting message; the row
    /// self-redacts locally when it passes and keeps the stamp. Step 89.
    pub expires_at_ms: Option<i64>,
}

impl From<&CoreItem> for TimelineItem {
    fn from(item: &CoreItem) -> Self {
        Self {
            item_id: item.item_id.clone(),
            room_id: item.room_id.clone(),
            sender_user_id: item.sender_user_id.clone(),
            sender_device_id: item.sender_device_id.clone(),
            sent_at_ms: item.sent_at_ms,
            kind: TimelineItemKind::from(&item.kind),
            edited_at_ms: item.edited_at_ms,
            redacted: item.redacted,
            reactions: item.reactions.clone(),
            read_by: item.read_by.clone(),
            delivered_to: item.delivered_to.clone(),
            delivery_status: item.delivery_status,
            expires_at_ms: item.expires_at_ms,
        }
    }
}

/// A stream-shaped change to a room's timeline, delivered to a
/// [`TimelineListener`](super::sync::TimelineListener).
// No `Eq`: variants carry `TimelineItem`, which carries `f32` (Step 26).
#[derive(Debug, Clone, PartialEq, uniffi::Enum)]
pub enum TimelineDiff {
    Append {
        room_id: String,
        item: TimelineItem,
    },
    UpdateAt {
        room_id: String,
        index: u32,
        item: TimelineItem,
    },
    RemoveAt {
        room_id: String,
        index: u32,
    },
    Reset {
        room_id: String,
        items: Vec<TimelineItem>,
    },
}

impl From<&CoreDiff> for TimelineDiff {
    fn from(diff: &CoreDiff) -> Self {
        match diff {
            CoreDiff::Append { room_id, item } => Self::Append {
                room_id: room_id.clone(),
                item: TimelineItem::from(item),
            },
            CoreDiff::UpdateAt {
                room_id,
                index,
                item,
            } => Self::UpdateAt {
                room_id: room_id.clone(),
                index: *index,
                item: TimelineItem::from(item),
            },
            CoreDiff::RemoveAt { room_id, index } => Self::RemoveAt {
                room_id: room_id.clone(),
                index: *index,
            },
            CoreDiff::Reset { room_id, items } => Self::Reset {
                room_id: room_id.clone(),
                items: items.iter().map(TimelineItem::from).collect(),
            },
        }
    }
}

pub(super) fn map_items(items: Vec<CoreItem>) -> Vec<TimelineItem> {
    items
        .into_iter()
        .map(|item| TimelineItem::from(&item))
        .collect()
}

/// Build a core `MessagesOptions` from the flat FFI arguments.
pub(super) fn build_options(limit: Option<u32>, before: Option<String>) -> MessagesOptions {
    MessagesOptions { limit, before }
}

/// A room timeline — the FFI wrapper around the native
/// [`kupferdraht::Timeline`](crate::timeline::Timeline).
#[derive(uniffi::Object)]
pub struct Timeline {
    inner: Arc<CoreTimeline>,
}

impl Timeline {
    pub(super) fn wrap(inner: Arc<CoreTimeline>) -> Arc<Self> {
        Arc::new(Self { inner })
    }
}

#[uniffi::export]
impl Timeline {
    /// The room id this timeline is bound to — the same string the
    /// owning `Room::id()` returned.
    #[must_use]
    pub fn room_id(&self) -> String {
        self.inner.room_id().to_owned()
    }

    /// Register a listener for this room's timeline diffs. Drop the
    /// returned `Subscription` (or call `unsubscribe`) to stop.
    #[must_use]
    pub fn subscribe(&self, listener: Arc<dyn TimelineListener>) -> Arc<Subscription> {
        Subscription::wrap(self.inner.subscribe(adapt_timeline_listener(listener)))
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl Timeline {
    /// All items in this room's timeline, oldest-first.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn items(&self) -> Result<Vec<TimelineItem>, KupferdrahtError> {
        Ok(map_items(self.inner.items().await?))
    }

    /// Send a plain-text message.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn send_text(&self, body: String) -> Result<MessageHandle, KupferdrahtError> {
        self.inner.send_text(body).await
    }

    /// Send a self-deleting text (Wire's `Ephemeral` wrapper): official
    /// clients render a countdown of `ttl` and delete their copy
    /// locally; Kupferdraht's own copy self-redacts after `ttl` too. `ttl`
    /// must be between 1 s and 365 days. Step 89.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn send_ephemeral_text(
        &self,
        body: String,
        ttl: std::time::Duration,
    ) -> Result<MessageHandle, KupferdrahtError> {
        self.inner.send_ephemeral_text(body, ttl).await
    }

    /// Re-drive one failed send from its persisted row under the same
    /// `message_id` (the `SendHandle::unwedge` shape). Step 79.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn resend(&self, message_id: String) -> Result<MessageHandle, KupferdrahtError> {
        self.inner.resend(message_id).await
    }

    /// Re-drive every failed send of this room, oldest first, stopping
    /// at the first hard error; rows behind it stay `Failed`. Step 79.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn resend_failed(&self) -> Result<ResendSummary, KupferdrahtError> {
        Ok(ResendSummary::from(&self.inner.resend_failed().await?))
    }

    /// Send a `Knock` (nudge). `hot_knock` is Wire's double-tap flag;
    /// plain knocks pass `false`. Step 26.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn send_knock(&self, hot_knock: bool) -> Result<MessageHandle, KupferdrahtError> {
        self.inner.send_knock(hot_knock).await
    }

    /// Share a `Location` (map point). `name` is an optional label,
    /// `zoom` an optional map zoom level. Step 26.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn send_location(
        &self,
        latitude: f32,
        longitude: f32,
        name: Option<String>,
        zoom: Option<i32>,
    ) -> Result<MessageHandle, KupferdrahtError> {
        self.inner
            .send_location(latitude, longitude, name, zoom)
            .await
    }

    /// Send a task into this room (ADR-0014, Step 60a). `with_fallback`
    /// selects the encoding mode: `Some(true)` (normal chats) also
    /// sends a human-readable `Text` fallback official Wire clients
    /// render; `Some(false)` (dedicated task-list rooms) sends the
    /// bare payload. `None` derives the default from the room's
    /// task-list marker (Step 75, ADR-0020): marked rooms suppress
    /// the fallback. An explicit choice always wins.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn send_task(
        &self,
        draft: TaskDraft,
        with_fallback: Option<bool>,
    ) -> Result<MessageHandle, KupferdrahtError> {
        self.inner.send_task(draft.into_core(), with_fallback).await
    }

    /// Change one task's state (Step 60b): applies the set-only `patch`
    /// onto the current folded state and sends the resulting full
    /// state as a new task message (`Done → Open` reopens). Fails with
    /// `InvalidInput` for an unknown `task_id`. `with_fallback`
    /// follows `send_task`'s contract — `None` derives from the
    /// room's task-list marker (ADR-0020).
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn update_task(
        &self,
        task_id: String,
        patch: TaskPatch,
        with_fallback: Option<bool>,
    ) -> Result<MessageHandle, KupferdrahtError> {
        self.inner
            .update_task(&task_id, patch.into_core(), with_fallback)
            .await
    }

    /// Move a task in this room's manual order (Step 60c): computes a
    /// fractional `sort_key` strictly between the `after`/`before`
    /// neighbors (`None` = the corresponding end; both `None` = move
    /// to the front — the bootstrap when no task carries a key yet)
    /// and sends it as an ordinary full-state update — a move touches
    /// exactly one task. A referenced neighbor must already carry a
    /// `sort_key`. `with_fallback` follows `send_task`'s contract —
    /// `None` derives from the room's task-list marker (ADR-0020).
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn move_task(
        &self,
        task_id: String,
        after: Option<String>,
        before: Option<String>,
        with_fallback: Option<bool>,
    ) -> Result<MessageHandle, KupferdrahtError> {
        self.inner
            .move_task(&task_id, after, before, with_fallback)
            .await
    }

    /// Send a typing `started` notification for this room. Kupferdraht
    /// auto-stops after ~11 s and dedupes repeat calls, so call this
    /// freely on keystrokes.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn send_typing_started(&self) -> Result<(), KupferdrahtError> {
        self.inner.send_typing_started().await
    }

    /// Send a typing `stopped` notification for this room (a no-op when
    /// nothing is in flight).
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn send_typing_stopped(&self) -> Result<(), KupferdrahtError> {
        self.inner.send_typing_stopped().await
    }

    /// Like `send_text`, but probes for local MLS state first and joins
    /// the room (external commit) when it is absent, then sends.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn send_text_auto_join(
        &self,
        body: String,
    ) -> Result<MessageHandle, KupferdrahtError> {
        self.inner.send_text_auto_join(body).await
    }

    /// Send a text message carrying `@`-mentions (UTF-16 offsets).
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn send_text_with_mentions(
        &self,
        body: String,
        mentions: Vec<Mention>,
    ) -> Result<MessageHandle, KupferdrahtError> {
        self.inner.send_text_with_mentions(body, mentions).await
    }

    /// Send a reply quoting `parent_id`.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn send_reply(
        &self,
        parent_id: String,
        body: String,
    ) -> Result<MessageHandle, KupferdrahtError> {
        self.inner.send_reply(parent_id, body).await
    }

    /// Edit a previously-sent message's body.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn edit(
        &self,
        item_id: String,
        new_body: String,
    ) -> Result<MessageHandle, KupferdrahtError> {
        self.inner.edit(item_id, new_body).await
    }

    /// Delete-for-everyone the message keyed by `item_id`.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn redact(&self, item_id: &str) -> Result<MessageHandle, KupferdrahtError> {
        self.inner.redact(item_id).await
    }

    /// Toggle an emoji reaction on a message.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn toggle_reaction(
        &self,
        item_id: String,
        emoji: String,
    ) -> Result<MessageHandle, KupferdrahtError> {
        self.inner.toggle_reaction(item_id, emoji).await
    }

    /// Send a `READ` receipt for a message. `None` when the account's
    /// receipt mode is off (no receipt emitted).
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn mark_as_read(
        &self,
        item_id: String,
    ) -> Result<Option<MessageHandle>, KupferdrahtError> {
        self.inner.mark_as_read(item_id).await
    }

    /// Encrypt + upload a file and send the MLS asset reference.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn send_file(
        &self,
        path: &str,
        mime_type: &str,
    ) -> Result<MessageHandle, KupferdrahtError> {
        self.inner
            .send_file(std::path::Path::new(path), mime_type)
            .await
    }

    /// Encrypt + upload a file from a streaming [`AssetSource`] and send the
    /// MLS asset reference — the streaming counterpart of [`Self::send_file`]
    /// (no filesystem path; e.g. an Android `content://` stream). `filename`
    /// is the display name.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn send_file_stream(
        &self,
        source: Arc<dyn AssetSource>,
        filename: &str,
        mime_type: &str,
    ) -> Result<MessageHandle, KupferdrahtError> {
        self.inner
            .send_file_stream(source, filename, mime_type)
            .await
    }

    /// Look up the downloadable asset for `message_id` in this room, or
    /// `None` if the message has no attachment / isn't synced locally.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn asset(&self, message_id: &str) -> Result<Option<Arc<Asset>>, KupferdrahtError> {
        Ok(self.inner.asset(message_id).await?.map(Asset::wrap))
    }
}

/// A downloadable asset attached to a message — the FFI wrapper around
/// the native [`AssetSummary`](crate::timeline::AssetSummary). Carries
/// the security material off the public surface; consumers reach the
/// plaintext via [`Asset::download`] / [`Asset::save_to`] and the public
/// metadata via [`Asset::metadata`].
#[derive(uniffi::Object)]
pub struct Asset {
    inner: AssetSummary,
}

impl Asset {
    fn wrap(inner: AssetSummary) -> Arc<Self> {
        Arc::new(Self { inner })
    }
}

#[uniffi::export]
impl Asset {
    /// The asset's public metadata.
    #[must_use]
    pub fn metadata(&self) -> AssetMeta {
        AssetMeta::from(&self.inner)
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl Asset {
    /// Download + decrypt the asset, returning the plaintext bytes.
    /// Capped at 64 MB — use `save_to` for larger assets.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn download(&self) -> Result<Vec<u8>, KupferdrahtError> {
        self.inner.download().await
    }

    /// Download + decrypt the asset, streaming the plaintext to a path.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn save_to(&self, dest_path: &str) -> Result<(), KupferdrahtError> {
        self.inner.save_to(std::path::Path::new(dest_path)).await
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl Client {
    /// Paginated read of a room's persisted timeline (local-only, no
    /// network). `limit` of `None` returns everything; `before`
    /// truncates at that message id.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn messages(
        &self,
        room_id: &str,
        limit: Option<u32>,
        before: Option<String>,
    ) -> Result<Vec<TimelineItem>, KupferdrahtError> {
        let opts = build_options(limit, before);
        Ok(map_items(self.inner.messages(room_id, &opts).await?))
    }

    /// Cross-room "my open work" view (Step 60c): tasks assigned to
    /// this account with status `Open` or `InProgress`, across every
    /// room in the local replica, in the ADR-0014 ordering.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn my_tasks(&self) -> Result<Vec<RoomTask>, KupferdrahtError> {
        Ok(self
            .inner
            .my_tasks()
            .await?
            .iter()
            .map(RoomTask::from)
            .collect())
    }

    /// Cross-room "what I delegated" view (Step 60c): tasks this
    /// account created and assigned to someone else — any status, so
    /// progress (`done`) stays visible.
    ///
    /// # Errors
    /// See `KupferdrahtError`.
    pub async fn delegated_tasks(&self) -> Result<Vec<RoomTask>, KupferdrahtError> {
        Ok(self
            .inner
            .delegated_tasks()
            .await?
            .iter()
            .map(RoomTask::from)
            .collect())
    }
}
