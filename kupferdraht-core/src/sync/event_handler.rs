// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `EventHandler` — consumer-facing trait the `SyncService` invokes
//! once it has decrypted, validated, and dispatched an inbound
//! event.
//!
//! Mirrors `matrix_sdk::Client::add_event_handler`'s shape: the
//! consumer registers an implementation, the SDK calls back into
//! it from the sync loop, and the consumer renders / persists the
//! result however it likes.
//!
//! The trait is `Send + Sync` so a single handler can live in an
//! `Arc<dyn EventHandler>` shared between threads — the sync loop
//! runs on tokio and the consumer's render path may run on a UI
//! thread.

/// Consumer notification surface for the internal `SyncService`.
/// Step 5c only invokes the three callbacks below; future steps may
/// extend the trait with more (typing indicators, read receipts,
/// member-add events, …) but will gain default-implementation no-op
/// methods so existing handlers don't break.
pub trait EventHandler: Send + Sync {
    /// A new application message arrived in a room. Includes both
    /// peer-sent and self-sent messages — Wire echoes back every
    /// device's own send via the event stream. Consumers that want
    /// to dedupe self-messages compare `message.sender_user_id`
    /// against `Client::account()::qualified_user_id()`.
    fn on_message(&self, message: Message);

    /// Kupferdraht has been invited to a new MLS group via a
    /// `conversation.mls-welcome` event. The local group is already
    /// joined and persisted by the time this fires; the consumer
    /// just needs to refresh its room list.
    ///
    /// Fired by the sync drain's Welcome absorption since Step 69
    /// (declared as a Step-5c stub before that).
    fn on_room_invited(&self, room_id: String);

    /// Frame parse / decrypt / persist failure inside the sync loop.
    /// The loop continues running after firing this — the consumer
    /// gets to surface the error in its UI without losing future
    /// events.
    fn on_sync_error(&self, description: String);

    /// The set of users typing in a room changed — an inbound
    /// `conversation.typing` started/stopped event. `event` carries the
    /// *current* set (render it verbatim; an empty set means "nobody is
    /// typing"). Step 21. Default no-op so handlers predating typing
    /// support keep compiling.
    ///
    /// **Transport caveat:** Wire delivers `conversation.typing` as a
    /// *transient* event over the live push socket only — either
    /// dialect `SyncTransport::WebSocket` negotiates (`/v14/events`,
    /// or the legacy `/await` route where a `transient: true` frame
    /// dispatches without advancing the cursor — Step 73, ADR-0030).
    /// It is never stored in the `/v14/notifications` pull queue, so
    /// polling consumers never see it. The send path
    /// (`Timeline::send_typing_*`) works over REST on any transport.
    fn on_typing(&self, _event: TypingEvent) {}
}

/// The current set of users typing in a room, delivered to
/// [`EventHandler::on_typing`] whenever an inbound `conversation.typing`
/// started/stopped event changes it.
///
/// Wire delivers discrete started/stopped signals; Kupferdraht derives this
/// "who is typing now" view client-side (Step 21). Ephemeral — never
/// persisted, never echoed back to the sender.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypingEvent {
    /// Qualified room id (`<uuid>@<domain>`).
    pub room_id: String,
    /// Qualified user ids (`<uuid>@<domain>`) currently typing in the
    /// room, sorted for a stable order. Empty when the last typist
    /// stopped — the indicator should clear.
    pub typing_users: Vec<String>,
}

/// One inbound application message decrypted from an MLS
/// `PrivateMessage`.
// No `Eq`: `MessageBody::Location` carries `f32` coords (Step 26).
#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    /// Qualified room id (`<uuid>@<domain>`).
    pub room_id: String,
    /// Qualified user id of the sender — the leaf openmls authenticated
    /// as the signer (Step 95a, ADR-0027), never the backend-written
    /// envelope. Exception: a signer with a non-Basic (e2ei X.509)
    /// credential is still attributed by the envelope's
    /// `qualified_from` until X.509 identity parsing lands.
    pub sender_user_id: String,
    /// Wire `client_id` of the sending device, from the same
    /// authenticated leaf. `None` only on the X.509 path when the
    /// envelope names no device.
    pub sender_device_id: Option<String>,
    /// Decoded `GenericMessage` content. Step 5c emits only
    /// [`MessageBody::Text`]; unknown protobuf oneof variants are
    /// logged + acked but not surfaced as a distinct
    /// `MessageBody` so consumers don't need to handle them.
    pub body: MessageBody,
    /// Wall-clock at which Kupferdraht finished decrypt + dispatch, in
    /// milliseconds since Unix epoch. Independent from Wire's
    /// `time` ISO string on the source event.
    pub received_at_ms: i64,
    /// `GenericMessage.message_id` — the UUID v4 picked by the
    /// sender. Stable across "same message arrives twice" races so
    /// the consumer can dedupe.
    pub message_id: String,
    /// `Text.quote.quoted_message_id` if this body is a reply.
    /// `None` for a plain (non-reply) text. Step 6-II-b.
    pub reply_to_id: Option<String>,
    /// Wall-clock at which this message's ephemeral timer elapses —
    /// `Some` only when the sender wrapped the body in Wire's
    /// `Ephemeral` (field 18). Kupferdraht starts the countdown at dispatch
    /// (`received_at_ms + ttl`) — a headless client has no viewport,
    /// so this is never later than an official client's
    /// view-triggered start; Kupferdraht tombstones its local row when it
    /// passes. `None` for every ordinary message. Step 89.
    pub expires_at_ms: Option<i64>,
}

/// Decoded body of a [`Message`].
// No `Eq`: the `Location` variant carries `f32` coords (Step 26).
#[derive(Debug, Clone, PartialEq)]
pub enum MessageBody {
    /// Plain-text message body. Wire's `Text.content` Protobuf
    /// field, UTF-8 already.
    Text(String),
    /// File-attachment reference. Wire's `Asset` Protobuf variant
    /// (Step 11g). Consumers call `summary.download().await` to
    /// materialise the plaintext bytes. Boxed to keep the enum's
    /// stack-size compact — `AssetSummary` carries ~200 bytes of
    /// metadata + envelope state.
    Asset(Box<crate::timeline::AssetSummary>),
    /// A nudge / "ping" (`Knock`). `hot_knock` marks a repeat. Step 26.
    Knock { hot_knock: bool },
    /// A shared map point (`Location`). Step 26.
    Location {
        latitude: f32,
        longitude: f32,
        name: Option<String>,
        zoom: Option<i32>,
    },
    /// Full case snapshot (`CasePayload`, ADR-0018). `sender_authorized`
    /// reports whether the sender was a conversation administrator at
    /// receive time — consumers should render unauthorized updates
    /// as attempts, not as case state (the fold ignores their core).
    /// Step 72b.
    Case {
        snapshot: Box<crate::timeline::CaseSnapshot>,
        sender_authorized: bool,
    },
    /// Full state of one task (`TaskPayload`, ADR-0014). Boxed to keep
    /// the enum stack-compact next to the cheap variants. Step 60a.
    Task(Box<crate::timeline::TaskItem>),
    /// Full state of the room's task-list marker (`TaskListMarker`,
    /// ADR-0020). The room-level fold is `Room::is_task_list()`; this
    /// row is the audit trail of the deliberately forgeable
    /// any-member flag — consumers render a system line or hide
    /// repeats themselves. Step 75.
    TaskListMarker { marked: bool },
}
