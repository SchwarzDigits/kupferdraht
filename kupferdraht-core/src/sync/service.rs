// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `SyncService` — the shared per-event dispatch pipeline plus the
//! pull-mode drain.
#![allow(clippy::doc_markdown)]
//!
//! `drain_pending` walks `GET /v14/notifications` page-by-page behind
//! the persisted cursor. Since Step 62 the same
//! `dispatch_notification` pipeline also serves the `/v14/events`
//! WebSocket push loop (`sync/push.rs`) — one decrypt/dispatch path,
//! two transports.
//!
//! Per-event pipeline:
//!
//! * `conversation.mls-message-add` → base64-decode `data` →
//!   `MlsClient::process_inbound_message` → if Application,
//!   `prost::Message::decode` as `GenericMessage` → on Text,
//!   fire `on_message` + persist a `timeline_events` row with
//!   `delivery_status='received'`.
//! * `conversation.mls-welcome` → absorb via
//!   `MlsClient::absorb_welcome` (Step 69): join + persist the
//!   group at the Welcome's epoch, then fire `on_room_invited`.
//!   Failures warn + skip — and can never trigger room recovery
//!   (the heal filter is keyed to `mls-message-add`).
//! * Other types → log + skip.
//!
//! Errors during single-event dispatch fold into `on_sync_error`
//! and the drain continues. Transport / cursor failures abort
//! `drain_pending` immediately and surface to the caller.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex as StdMutex, Weak};

use base64::Engine as _;
use prost::Message as _;
use tracing::{debug, warn};

use crate::client::Client;
use crate::encryption::{DecodedMessage, LeafIdentity, LeafIdentityError};
use crate::error::KupferdrahtError;
use crate::sync::event_handler::{EventHandler, Message, TypingEvent};
use crate::sync::follow_ups::FollowUp;
use crate::sync::outbox::{self, Held};
use crate::sync::sender::{EnvelopeSender, InboundSender, attribute_sender};
use crate::wire::proto::{GenericMessage, generic_message};

type EventHandlers = Arc<StdMutex<Vec<Arc<dyn EventHandler>>>>;

/// Per-`Client` event-drain handle. Cheap to construct; the actual
/// `GET /v14/notifications` call happens inside `drain_pending`.
pub(crate) struct SyncService {
    pub(super) client: Weak<Client>,
    handlers: EventHandlers,
    /// Rooms already resynced during the current drain (Step 38). An inbound
    /// out-of-order message heals its room by resyncing to the current epoch;
    /// this dedups so a stale backlog (many old-epoch frames) triggers at most
    /// one resync per room per drain instead of one per frame. Cleared at the
    /// start of each `drain_pending`.
    resynced: StdMutex<HashSet<String>>,
    /// Network work recorded during a dispatch cycle and run after it,
    /// outside the drain lock (Step 97d, ADR-0031 — `follow_ups.rs`).
    pub(super) follow_ups: StdMutex<Vec<FollowUp>>,
    /// Per room: the abandonment clock of a deferral — the group epoch
    /// the pending commit was staged at and when the drain first stopped
    /// before a message awaiting it (`drain.rs`). A clock outlives its
    /// commit only until the room defers again at another epoch.
    pub(super) deferrals: StdMutex<HashMap<String, crate::sync::drain::DeferralClock>>,
}

/// What dispatching one notification came to (Step 97d).
pub(super) enum NotificationOutcome {
    /// Every event handled; the count of dispatched events. The cursor
    /// advanced past the notification.
    Dispatched(u32),
    /// An MLS message of `room_id` awaits an own pending commit: the
    /// notification was not finished and the cursor stays before it.
    Deferred { room_id: String },
}

/// What dispatching one event came to.
enum EventOutcome {
    Handled,
    Deferred { room_id: String },
}

/// Why one event failed (Step 97e, ADR-0032).
enum EventError {
    /// Storing its data in `kupferdraht.db` failed: the notification's MLS
    /// changes roll back and it is replayed later.
    Persist(KupferdrahtError),
    /// Anything else (undecryptable, malformed, …): skipped, maybe healed.
    Other(KupferdrahtError),
}

impl EventError {
    /// A failure of the event's MLS step itself (decrypt / merge): a
    /// storage error is transient since every MLS operation is one
    /// transaction — retried like a failed persist (Step 106, ADR-0034
    /// d.2); anything else is the message's own fault.
    fn from_mls_step(e: KupferdrahtError) -> Self {
        if matches!(e, KupferdrahtError::Storage(_)) {
            Self::Persist(e)
        } else {
            Self::Other(e)
        }
    }

    /// A failure after the event's MLS step: a storage error is a
    /// persistence failure, anything else is not.
    fn after_mls(e: KupferdrahtError) -> Self {
        if matches!(e, KupferdrahtError::Storage(_)) {
            Self::Persist(e)
        } else {
            Self::Other(e)
        }
    }
}

/// What decrypting one `mls-message-add` came to.
enum Decrypted {
    Application(Vec<u8>, Result<LeafIdentity, LeafIdentityError>),
    /// A handshake, a proposal or a stale message — nothing to surface.
    Nothing,
    AwaitsOwnCommit,
}

impl SyncService {
    pub(crate) fn new(client: Weak<Client>, handlers: EventHandlers) -> Arc<Self> {
        Arc::new(Self {
            client,
            handlers,
            resynced: StdMutex::new(HashSet::new()),
            follow_ups: StdMutex::new(Vec::new()),
            deferrals: StdMutex::new(HashMap::new()),
        })
    }

    /// End-of-cycle maintenance shared by the REST drain and both push
    /// dialects (one cycle = one drain or one push frame): refresh a room
    /// list the cycle marked stale (Step 118), flush the batched
    /// external-removal commits (Step 74) and tombstone every ephemeral
    /// row whose timer elapsed (Step 89). All three are infallible toward
    /// the caller — a failed refresh stays marked for the next pass, a
    /// failed flush retries next cycle, a failed refresh or sweep is
    /// reported through `on_sync_error`.
    pub(super) async fn end_dispatch_cycle(&self, client: &Arc<Client>) {
        if let Err(e) = client.refresh_rooms_if_stale().await {
            warn!(error = %e, "room list refresh failed; retrying next pass");
            self.notify_error(&format!("room list refresh: {e}"));
        }
        self.run_follow_ups(client).await;
        self.flush_removal_commits(client).await;
        if let Err(e) = client.expire_due_ephemeral_messages().await {
            warn!(error = %e, "ephemeral expiry sweep failed; retrying next cycle");
            self.notify_error(&format!("ephemeral expiry sweep: {e}"));
        }
    }

    /// Reset the per-cycle room-recovery dedup (see [`Self::recover_room`]).
    /// One cycle = one REST drain (`drain_pending`) or one push frame
    /// (Step 62b) — the push loop resets per frame because a long-lived
    /// connection must be able to heal the same room again hours later,
    /// which a connection-scoped dedup would suppress.
    pub(super) fn begin_dispatch_cycle(&self) {
        self.resynced.lock().expect("resynced lock").clear();
    }

    /// The event-dispatch half of [`Self::dispatch_notification`],
    /// WITHOUT the cursor advance — the legacy `/await` transport
    /// dispatches `transient: true` frames through this (they are
    /// live-only and must never move `last_event_id`, ADR-0030 d.4;
    /// its durable frames only trigger [`Self::drain_pending`]).
    /// Dispatch one transient frame as its own cycle, without a cursor advance
    /// (ADR-0030 d.4 — transient events are never stored, so no drain can ever
    /// return them).
    ///
    /// The one dispatch path reachable from outside `sync`: Step 115b's browser
    /// push socket needs it, and `NotificationOutcome` stays private because
    /// this returns the event count instead.
    ///
    /// # Errors
    /// Whatever dispatching the notification's events returns.
    pub(crate) async fn dispatch_transient_notification(
        &self,
        client: &Arc<Client>,
        notif: &crate::wire::notification_dto::QueuedNotification,
    ) -> Result<u32, KupferdrahtError> {
        debug_assert!(notif.transient, "only transient frames dispatch directly");
        self.begin_dispatch_cycle();
        // Transient events (typing) are never MLS messages — never deferred.
        let count = match self.dispatch_notification_events(client, notif).await? {
            NotificationOutcome::Dispatched(count) => count,
            NotificationOutcome::Deferred { .. } => 0,
        };
        // Step 74 / Step 89: every dispatch cycle ends with the removal flush and
        // the ephemeral sweep (a no-op scan on typing).
        self.end_dispatch_cycle(client).await;
        Ok(count)
    }

    pub(super) async fn dispatch_notification_events(
        &self,
        client: &Arc<Client>,
        notif: &crate::wire::notification_dto::QueuedNotification,
    ) -> Result<NotificationOutcome, KupferdrahtError> {
        let mut count: u32 = 0;
        for ev in extract_notification_events(&notif.payload) {
            match self.dispatch_event(client, &ev).await {
                Ok(EventOutcome::Handled) => count += 1,
                Ok(EventOutcome::Deferred { room_id }) => {
                    return Ok(NotificationOutcome::Deferred { room_id });
                }
                Err(EventError::Persist(e)) => return Err(e),
                Err(EventError::Other(e)) => {
                    self.note_failed_event(&notif.id, &ev, &e);
                    count += 1;
                }
            }
        }
        Ok(NotificationOutcome::Dispatched(count))
    }

    /// A single event failed: report it and, where the failure signals
    /// broken local MLS state, queue the room's recovery.
    fn note_failed_event(&self, notif_id: &str, ev: &serde_json::Value, e: &KupferdrahtError) {
        if matches!(e, KupferdrahtError::MlsOutOfOrder) {
            if let Ok(env) = extract_mls_envelope(ev) {
                // ADR-0034 d.1: a later epoch alone proves nothing — the
                // backend confirms the gap before the room heals.
                self.queue_room_recovery(FollowUp::HealRoomIfBehind(env.room_id));
            } else {
                warn!(notif_id, "drain: out-of-order event without a room id");
                self.notify_error("drain: out-of-order event without a room id");
            }
            return;
        }
        warn!(error = %e, "drain: event dispatch failed; skipping");
        self.notify_error(&format!("drain: {e}"));
        // Step 64 (review S1), narrowed after the re-check and by Step
        // 106 (ADR-0034 d.2): an `mls-message-add` triggers room recovery
        // only on the one POSITIVE broken-local-state signal left —
        // `InvalidInput`, no local group (the group loads before the
        // ciphertext parses, so a garbage frame in a healthy group
        // surfaces as `Encoding`/`Generic` instead). A storage failure is
        // transient and retried, never healed. Healing on every class
        // would let any member discard+rejoin a healthy group once per
        // drain by posting garbage.
        if matches!(e, KupferdrahtError::InvalidInput(_))
            && ev.get("type").and_then(serde_json::Value::as_str)
                == Some("conversation.mls-message-add")
            && let Ok(env) = extract_mls_envelope(ev)
        {
            self.queue_room_recovery(FollowUp::HealRoom(env.room_id));
        }
    }

    /// Heal a room whose inbound message arrived out-of-order (its advancing
    /// commit was missed) by resyncing to the current epoch — recorded
    /// here, run as a follow-up after the drain lock is released (Step
    /// 97d; the resync is network). Deduped per cycle: the first
    /// out-of-order frame for a room queues the resync, later frames for
    /// the same room are no-ops — they're stale relative to the epoch the
    /// resync rejoins and would only churn. Step 38.
    fn queue_room_recovery(&self, heal: FollowUp) {
        let room_id = match &heal {
            FollowUp::HealRoom(room_id) | FollowUp::HealRoomIfBehind(room_id) => room_id.clone(),
            _ => {
                debug_assert!(false, "queue_room_recovery takes a heal follow-up");
                return;
            }
        };
        {
            let mut set = self.resynced.lock().expect("resynced lock");
            if !set.insert(room_id) {
                return;
            }
        }
        self.queue_follow_up(heal);
    }

    /// Dispatch a single Wire `Event` JSON object. Returns `Err` for
    /// frame-shape / decrypt failures; the caller folds that into
    /// `on_sync_error`.
    async fn dispatch_event(
        &self,
        client: &Arc<Client>,
        ev: &serde_json::Value,
    ) -> Result<EventOutcome, EventError> {
        let Some(ty) = ev.get("type").and_then(serde_json::Value::as_str) else {
            warn!("event missing `type` field; skipping");
            return Ok(EventOutcome::Handled);
        };
        match ty {
            "conversation.mls-message-add" => self.dispatch_mls_message(client, ev).await,
            "conversation.mls-welcome" => self
                .dispatch_mls_welcome(client, ev)
                .await
                .map(|()| EventOutcome::Handled),
            _ => self
                .dispatch_other_event(client, ty, ev)
                .await
                .map(|()| EventOutcome::Handled)
                .map_err(EventError::after_mls),
        }
    }

    /// Every event type except the two MLS ones — none of them can be
    /// deferred, none touches MLS state.
    async fn dispatch_other_event(
        &self,
        client: &Arc<Client>,
        ty: &str,
        ev: &serde_json::Value,
    ) -> Result<(), KupferdrahtError> {
        match ty {
            "feature-config.update" => {
                self.dispatch_feature_config_update();
                Ok(())
            }
            "user.properties-set" => self.dispatch_user_property_set(ev),
            "user.properties-delete" => self.dispatch_user_property_delete(client, ev).await,
            "conversation.typing" => {
                dispatch_inbound_typing(client, ev);
                Ok(())
            }
            "conversation.delete" => {
                dispatch_conversation_delete(client, ev);
                Ok(())
            }
            other => {
                // Step 118: an event Kupferdraht does not process may still
                // change the room list (a room created or renamed, a member
                // joined or left, a profile changed).
                debug!(
                    event_type = other,
                    "skipping non-MLS event; room list marked stale"
                );
                client.mark_rooms_stale();
                Ok(())
            }
        }
    }

    /// Absorb a `conversation.mls-welcome` event (Step 69, closes
    /// gap-B.24 — the skip-and-heal path lost the initiator's first
    /// message to forward secrecy, observations §67).
    ///
    /// `data` is the one bare-string payload in Wire's event set
    /// (backend-spec §12.2): base64 of `MlsMessage(Welcome)`.
    /// Absorb → persist → align the cached room's epoch with the
    /// joined group (metadata synced before the drain may lag; a
    /// room not yet cached is aligned by the next sync instead).
    ///
    /// Failure classes are `Encoding`/`Generic` by construction
    /// ([`crate::encryption::MlsClient::absorb_welcome`]): the drain
    /// warns + continues, and the room-recovery filter ignores both,
    /// so a garbage Welcome can neither wedge the stream nor trigger
    /// discard+rejoin churn. Already-joined groups (external commit
    /// won the race) and redelivered Welcomes (key-package secret
    /// consumed) land in absorb_welcome's documented skip paths.
    async fn dispatch_mls_welcome(
        &self,
        client: &Arc<Client>,
        ev: &serde_json::Value,
    ) -> Result<(), EventError> {
        // Step 118: a Welcome is a new room — before the absorb, which may
        // fail without changing that.
        client.mark_rooms_stale();
        let epoch = absorb_welcome_event(client, ev).map_err(EventError::Other)?;
        if let Some(room_id) = qualified_id_field(ev, "qualified_conversation") {
            if client.room(&room_id).await.is_some() {
                client
                    .bump_room_epoch(&room_id, epoch)
                    .await
                    .map_err(EventError::after_mls)?;
                client.note_room_devices(&room_id).await;
            }
            // The contract EventHandler::on_room_invited has promised
            // since Step 5c: the group is joined + persisted when
            // this fires; the consumer refreshes its room list.
            self.notify_room_invited(&room_id);
        }
        debug!(epoch, "mls-welcome absorbed");
        Ok(())
    }

    /// Decrypt + dispatch a `conversation.mls-message-add` event. The
    /// message is attributed to the leaf openmls authenticated, not to
    /// the envelope (Step 95a, ADR-0027 — `sync/sender.rs`).
    async fn dispatch_mls_message(
        &self,
        client: &Arc<Client>,
        ev: &serde_json::Value,
    ) -> Result<EventOutcome, EventError> {
        let env = extract_mls_envelope(ev).map_err(EventError::Other)?;
        let decrypted = decrypt_to_application(client, &env.room_id, &env.ciphertext)
            .await
            .map_err(EventError::from_mls_step)?;
        let (plaintext, leaf) = match decrypted {
            Decrypted::Application(plaintext, leaf) => (plaintext, leaf),
            Decrypted::Nothing => return Ok(EventOutcome::Handled),
            Decrypted::AwaitsOwnCommit => {
                return Ok(EventOutcome::Deferred {
                    room_id: env.room_id,
                });
            }
        };
        self.dispatch_decrypted(client, &env, plaintext, leaf)
            .await
            .map(|()| EventOutcome::Handled)
            .map_err(EventError::after_mls)
    }

    /// Decode, attribute and route one decrypted application message.
    async fn dispatch_decrypted(
        &self,
        client: &Arc<Client>,
        env: &MlsEnvelope,
        plaintext: Vec<u8>,
        leaf: Result<LeafIdentity, LeafIdentityError>,
    ) -> Result<(), KupferdrahtError> {
        let generic = GenericMessage::decode(plaintext.as_slice())
            .map_err(|e| KupferdrahtError::Encoding(format!("GenericMessage: {e}")))?;
        if !envelope_id_is_dispatchable(env, &generic.message_id) {
            return Ok(());
        }
        let claimed = EnvelopeSender {
            user_id: env.sender_user_id.as_deref(),
            device_id: env.sender_device_id.as_deref(),
        };
        let Some(sender) = attribute_sender(&claimed, leaf, &generic.message_id) else {
            return Ok(());
        };
        self.dispatch_generic_message(client, &env.room_id, sender, generic)
            .await
    }

    /// Route a decrypted, attributed `GenericMessage`: Kupferdraht's own
    /// payload fields (case, marker, task — outside the content oneof)
    /// first, then the content variant.
    async fn dispatch_generic_message(
        &self,
        client: &Arc<Client>,
        room_id: &str,
        sender: InboundSender,
        mut generic: GenericMessage,
    ) -> Result<(), KupferdrahtError> {
        let InboundSender {
            user_id: sender_user_id,
            device_id: sender_device_id,
        } = sender;
        let (case, task) = take_valid_kupferdraht_payloads(&mut generic);
        // Step 72b (ADR-0018): the case payload (field 262001) is
        // checked FIRST — it never carries a fallback, and a message
        // holds at most one of the two Kupferdraht fields.
        if let Some(case) = case {
            return self
                .dispatch_inbound_case(
                    client,
                    room_id,
                    &sender_user_id,
                    sender_device_id.as_deref(),
                    &generic.message_id,
                    case,
                )
                .await;
        }
        // Step 75 (ADR-0020): the task-list marker (field 262002) —
        // the third Kupferdraht sibling outside the oneof, never with a
        // fallback; any member may flip it.
        if let Some(marker) = generic.task_list_marker.take() {
            return self
                .dispatch_inbound_task_list_marker(
                    client,
                    room_id,
                    &sender_user_id,
                    sender_device_id.as_deref(),
                    &generic.message_id,
                    marker.marked,
                )
                .await;
        }
        // Step 60a (ADR-0014): the task payload travels OUTSIDE the
        // content oneof (field 262000) so it can ride alongside a Text
        // fallback. Check it BEFORE the oneof — a task message with
        // fallback must become a Task item, not a Text item.
        if let Some(task) = task {
            return self
                .dispatch_inbound_task(
                    client,
                    room_id,
                    &sender_user_id,
                    sender_device_id.as_deref(),
                    &generic.message_id,
                    task,
                )
                .await;
        }
        self.dispatch_content_variant(
            client,
            room_id,
            &sender_user_id,
            sender_device_id.as_deref(),
            generic,
        )
        .await
    }

    /// Route an edit or delete of an earlier message to its handler —
    /// both are authorized against `sender_user_id` (Step 95b, ADR-0028).
    /// Split out of [`Self::dispatch_content_variant`] to keep that
    /// match within the line budget.
    async fn dispatch_target_op(
        &self,
        client: &Arc<Client>,
        room_id: &str,
        sender_user_id: &str,
        message_id: &str,
        content: generic_message::Content,
    ) -> Result<(), KupferdrahtError> {
        match content {
            generic_message::Content::Edited(edit) => {
                self.dispatch_inbound_edit(client, room_id, sender_user_id, message_id, edit)
                    .await
            }
            generic_message::Content::Deleted(delete) => {
                self.dispatch_inbound_delete(client, room_id, sender_user_id, message_id, delete)
                    .await
            }
            _ => Ok(()),
        }
    }

    /// Route a decoded `GenericMessage` content variant to its
    /// per-interaction handler (Step 6-II). Split out of
    /// [`Self::dispatch_generic_message`] when the Step 60a task fast-path
    /// pushed that function past the line budget.
    async fn dispatch_content_variant(
        &self,
        client: &Arc<Client>,
        room_id: &str,
        sender_user_id: &str,
        sender_device_id: Option<&str>,
        generic: GenericMessage,
    ) -> Result<(), KupferdrahtError> {
        match generic.content {
            Some(generic_message::Content::Text(t)) => {
                self.dispatch_inbound_text(
                    client,
                    room_id,
                    sender_user_id,
                    sender_device_id,
                    &generic.message_id,
                    t,
                )
                .await
            }
            Some(
                content @ (generic_message::Content::Edited(_)
                | generic_message::Content::Deleted(_)),
            ) => {
                self.dispatch_target_op(
                    client,
                    room_id,
                    sender_user_id,
                    &generic.message_id,
                    content,
                )
                .await
            }
            Some(generic_message::Content::Reaction(reaction)) => {
                self.dispatch_inbound_reaction(
                    client,
                    room_id,
                    sender_user_id,
                    &generic.message_id,
                    reaction,
                )
                .await
            }
            Some(generic_message::Content::Confirmation(conf)) => {
                self.dispatch_inbound_confirmation(
                    client,
                    room_id,
                    sender_user_id,
                    &generic.message_id,
                    conf,
                )
                .await
            }
            Some(generic_message::Content::Asset(asset)) => {
                self.dispatch_inbound_asset(
                    client,
                    room_id,
                    sender_user_id,
                    sender_device_id,
                    &generic.message_id,
                    asset,
                )
                .await
            }
            Some(generic_message::Content::Knock(knock)) => {
                self.dispatch_inbound_knock(
                    client,
                    room_id,
                    sender_user_id,
                    sender_device_id,
                    &generic.message_id,
                    knock,
                )
                .await
            }
            Some(generic_message::Content::Location(location)) => {
                self.dispatch_inbound_location(
                    client,
                    room_id,
                    sender_user_id,
                    sender_device_id,
                    &generic.message_id,
                    location,
                )
                .await
            }
            Some(generic_message::Content::Ephemeral(ephemeral)) => {
                self.dispatch_inbound_ephemeral(
                    client,
                    room_id,
                    sender_user_id,
                    sender_device_id,
                    &generic.message_id,
                    ephemeral,
                )
                .await
            }
            None => {
                warn!(
                    message_id = %generic.message_id,
                    "GenericMessage with no content variant — skip dispatch"
                );
                Ok(())
            }
        }
    }

    /// Tell the handlers of an inbound message — after the page commits
    /// when it arrives in one (Step 109, ADR-0037).
    pub(super) fn notify_message(&self, message: &Message) {
        if outbox::hold(Held::Message(message.clone())).is_some() {
            self.emit_message(message);
        }
    }

    fn emit_message(&self, message: &Message) {
        let snapshot = self
            .handlers
            .lock()
            .expect("event_handlers mutex poisoned")
            .clone();
        for h in &snapshot {
            h.on_message(message.clone());
        }
    }

    /// Tell the handlers of a joined room — after the page commits when
    /// the Welcome arrives in one.
    pub(super) fn notify_room_invited(&self, room_id: &str) {
        if outbox::hold(Held::RoomInvited(room_id.to_string())).is_some() {
            self.emit_room_invited(room_id);
        }
    }

    fn emit_room_invited(&self, room_id: &str) {
        let snapshot = self
            .handlers
            .lock()
            .expect("event_handlers mutex poisoned")
            .clone();
        for h in &snapshot {
            h.on_room_invited(room_id.to_string());
        }
    }

    /// Emit what a committed page held back, in the order it happened.
    pub(super) fn emit_held(&self, client: &Client, held: Vec<Held>) {
        for item in held {
            match item {
                Held::Message(message) => self.emit_message(&message),
                Held::RoomInvited(room_id) => self.emit_room_invited(&room_id),
                Held::Timeline(diff) => client.emit_timeline_diff(&diff),
                Held::Devices(change) => client.emit_device_change(&change),
            }
        }
    }

    pub(super) fn notify_error(&self, description: &str) {
        let snapshot = self
            .handlers
            .lock()
            .expect("event_handlers mutex poisoned")
            .clone();
        for h in &snapshot {
            h.on_sync_error(description.to_string());
        }
    }
}

/// Step 95c: take the case and task payloads out of `generic`, each only
/// if valid — an invalid Kupferdraht payload is treated as absent (a task
/// message then renders its `Text` fallback).
fn take_valid_kupferdraht_payloads(
    generic: &mut GenericMessage,
) -> (
    Option<crate::wire::proto::CasePayload>,
    Option<crate::wire::proto::TaskPayload>,
) {
    let case = generic
        .case
        .take()
        .filter(super::dispatch::case::case_payload_is_valid);
    let task = generic
        .task
        .take()
        .filter(super::dispatch::task::task_payload_is_valid);
    (case, task)
}

fn qualified_id_field(ev: &serde_json::Value, key: &str) -> Option<String> {
    let obj = ev.get(key)?;
    let id = obj.get("id").and_then(serde_json::Value::as_str)?;
    let domain = obj.get("domain").and_then(serde_json::Value::as_str)?;
    // Step 95c: backend JSON — a malformed id is treated as absent.
    crate::identity::UserId::from_parts(id, domain)
        .ok()
        .map(|user_id| user_id.as_str().to_string())
}

/// Apply an inbound `conversation.typing` event: update the per-room
/// typing set and notify observers with the current set. Wire delivers
/// discrete `started`/`stopped` signals (`data.status`); Kupferdraht derives
/// the live "who is typing now" set client-side. Ephemeral — no
/// decrypt, no persist, no I/O. A free function (not a `SyncService`
/// method) because it needs no service state, only the `Client`.
/// Step 21.
fn dispatch_inbound_typing(client: &Arc<Client>, ev: &serde_json::Value) {
    let Some((room_id, sender, started)) = parse_typing_event(ev) else {
        return;
    };
    let typing_users = client.record_incoming_typing(&room_id, &sender, started);
    client.notify_typing(&TypingEvent {
        room_id,
        typing_users,
    });
}

/// Handle an inbound `conversation.delete` event: a conversation was
/// deleted for everyone (by an admin). Prune it from the in-memory room
/// cache right away instead of waiting for the next metadata refresh;
/// the persisted snapshot self-corrects on the next `sync_metadata`.
/// Step 23.
fn dispatch_conversation_delete(client: &Arc<Client>, ev: &serde_json::Value) {
    let Some(room_id) = qualified_id_field(ev, "qualified_conversation") else {
        warn!("conversation.delete: missing qualified_conversation; skip");
        return;
    };
    if client.remove_cached_room(&room_id) {
        debug!(room_id = %room_id, "conversation.delete: pruned room from cache");
    }
}

/// Parse a `conversation.typing` event into `(room_id, sender,
/// started)`, or `None` when the frame is malformed or carries an
/// unknown `data.status`. Typing is ephemeral and best-effort, so a bad
/// frame is logged and skipped — never escalated to `on_sync_error`.
/// Pure — split out so the JSON-shape handling is unit-testable without
/// a `Client`.
fn parse_typing_event(ev: &serde_json::Value) -> Option<(String, String, bool)> {
    let Some(room_id) = qualified_id_field(ev, "qualified_conversation") else {
        warn!("conversation.typing: missing qualified_conversation; skip");
        return None;
    };
    let Some(sender) = qualified_id_field(ev, "qualified_from") else {
        warn!("conversation.typing: missing qualified_from; skip");
        return None;
    };
    let Some(status) = ev
        .get("data")
        .and_then(|d| d.get("status"))
        .and_then(serde_json::Value::as_str)
    else {
        warn!("conversation.typing: missing data.status; skip");
        return None;
    };
    let started = match status {
        "started" => true,
        "stopped" => false,
        other => {
            warn!(status = other, "conversation.typing: unknown status; skip");
            return None;
        }
    };
    Some((room_id, sender, started))
}

/// Flatten a Wire `Notification.payload` into its embedded events.
/// Wire emits a JSON array, a single event object, or a null
/// payload; we tolerate all three shapes so older notifications
/// don't trip the drain loop.
fn extract_notification_events(payload: &serde_json::Value) -> Vec<serde_json::Value> {
    match payload {
        serde_json::Value::Array(arr) => arr.clone(),
        other if !other.is_null() => vec![other.clone()],
        _ => Vec::new(),
    }
}

/// Frame-shape fields lifted out of a `conversation.mls-message-add`
/// JSON event, ready for the decrypt + dispatch passes.
struct MlsEnvelope {
    room_id: String,
    /// What the backend claims — a cross-check only (ADR-0027).
    sender_user_id: Option<String>,
    sender_device_id: Option<String>,
    ciphertext: Vec<u8>,
}

/// Step 80 dispatch boundary: the envelope `message_id` is
/// peer-controlled and reaches the filesystem (asset cache
/// `<id>.enc`). A path-unsafe id is a malformed envelope — log and
/// discard (never a hard error: one hostile message must not abort
/// the drain; never an assert: runtime data).
fn envelope_id_is_dispatchable(env: &MlsEnvelope, message_id: &str) -> bool {
    if crate::storage::is_path_safe_id(message_id) {
        return true;
    }
    warn!(
        room_id = %env.room_id,
        sender = ?env.sender_user_id,
        "discarding inbound message with path-unsafe message_id"
    );
    false
}

/// Parse a `conversation.mls-message-add` JSON event into the fields
/// the decrypt + dispatch passes need. Returns `Err` for missing
/// required fields or malformed base64; the caller folds that into
/// `on_sync_error`.
fn extract_mls_envelope(ev: &serde_json::Value) -> Result<MlsEnvelope, KupferdrahtError> {
    let room_id = qualified_id_field(ev, "qualified_conversation").ok_or_else(|| {
        KupferdrahtError::Encoding("mls-message-add: missing qualified_conversation".into())
    })?;
    let sender_user_id = qualified_id_field(ev, "qualified_from");
    // Only `from_client` is a device claim — a `subconv` id is not a
    // client id and would fake an envelope/leaf mismatch (review #55).
    let sender_device_id = ev
        .get("from_client")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let data_b64 = ev
        .get("data")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| KupferdrahtError::Encoding("mls-message-add: missing data".into()))?;
    let ciphertext = base64::engine::general_purpose::STANDARD
        .decode(data_b64)
        .map_err(|e| KupferdrahtError::Encoding(format!("mls-message-add data b64: {e}")))?;
    if ciphertext.is_empty() {
        return Err(KupferdrahtError::Encoding(
            "mls-message-add: empty data".into(),
        ));
    }
    Ok(MlsEnvelope {
        room_id,
        sender_user_id,
        sender_device_id,
        ciphertext,
    })
}

/// Resolve `room_id` to its MLS group, run the ciphertext through
/// openmls, and filter to Application-message plaintext only, paired
/// with the authenticated signer's parsed identity (ADR-0027).
/// `Ok(None)` for handshake / proposal messages — the caller treats
/// them as "no application content to dispatch".
async fn decrypt_to_application(
    client: &Arc<Client>,
    room_id: &str,
    ciphertext: &[u8],
) -> Result<Decrypted, KupferdrahtError> {
    let group_id = client.group_id_for_room(room_id).await.ok_or_else(|| {
        KupferdrahtError::InvalidInput(format!(
            "mls-message-add: no local group_id for {room_id} \
             (run sync_metadata + room-join first)"
        ))
    })?;
    let mls = client.mls_client_required("dispatch_mls_message")?;
    let decoded = {
        let guard = crate::encryption::lock_mls(mls);
        guard.process_inbound_message(&group_id, ciphertext)?
    };
    surface_decoded(client, room_id, decoded).await
}

/// Absorb the `MlsMessage(Welcome)` a `conversation.mls-welcome` event
/// carries (`data`, base64) and return the joined group's epoch.
fn absorb_welcome_event(client: &Client, ev: &serde_json::Value) -> Result<u64, KupferdrahtError> {
    let data_b64 = ev
        .get("data")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            KupferdrahtError::Encoding("mls-welcome: missing bare-string data".into())
        })?;
    let welcome_bytes = base64::engine::general_purpose::STANDARD
        .decode(data_b64)
        .map_err(|e| KupferdrahtError::Encoding(format!("mls-welcome data b64: {e}")))?;
    let mls = client.mls_client_required("dispatch_mls_welcome")?;
    let guard = crate::encryption::lock_mls(mls);
    let group_id = guard.absorb_welcome(&welcome_bytes)?;
    guard.group_epoch(&group_id)
}

/// What a processed MLS message leaves for the dispatcher, plus its
/// side effects on the room (device-set check, removal flush).
async fn surface_decoded(
    client: &Arc<Client>,
    room_id: &str,
    decoded: DecodedMessage,
) -> Result<Decrypted, KupferdrahtError> {
    match decoded {
        DecodedMessage::Application { plaintext, sender } => {
            Ok(Decrypted::Application(plaintext, sender))
        }
        DecodedMessage::Handshake => {
            // Step 95d: a merged commit may have changed the device set.
            client.note_room_devices(room_id).await;
            Ok(Decrypted::Nothing)
        }
        // Step 97d (ADR-0031): an older epoch is skipped, never healed.
        DecodedMessage::StaleEpoch => {
            debug!(room_id, "mls-message-add from an older epoch; skipped");
            Ok(Decrypted::Nothing)
        }
        DecodedMessage::AwaitsOwnCommit => Ok(Decrypted::AwaitsOwnCommit),
        // Step 97e (ADR-0032): decrypted before, MLS state committed — a
        // replay; its data is already stored.
        DecodedMessage::AlreadyDecrypted => {
            debug!(room_id, "mls-message-add already decrypted; replay skipped");
            Ok(Decrypted::Nothing)
        }
        // Step 28: an external-sender Remove proposal was queued.
        // Step 74: do NOT commit inline — per-proposal commits storm
        // a backlog with `mls-commit-missing-references` rejections
        // (observations §70). Record the room; the end-of-cycle
        // `flush_removal_commits` commits once for every proposal
        // that arrived this cycle. No application plaintext either
        // way.
        DecodedMessage::ProposalQueued => {
            client.queue_removal_flush(room_id);
            Ok(Decrypted::Nothing)
        }
    }
}

#[cfg(test)]
mod mls_envelope_tests {
    use super::extract_mls_envelope;

    /// A `subconv` id is not the sender's client id: it must not become
    /// the envelope's device claim (it would fake a mismatch against the
    /// leaf, and on the X.509 path become the attributed device).
    #[test]
    fn a_subconv_id_is_not_a_device_claim() {
        let ev = serde_json::json!({
            "type": "conversation.mls-message-add",
            "qualified_conversation": {"id": "room-uuid", "domain": "wire.com"},
            "qualified_from": {"id": "bob-uuid", "domain": "wire.com"},
            "subconv": "conference",
            "data": "AAAA",
        });
        let envelope = extract_mls_envelope(&ev).expect("envelope");
        assert_eq!(envelope.sender_device_id, None);
    }
}

#[cfg(test)]
mod typing_event_tests {
    use super::parse_typing_event;

    fn event(status: &str) -> serde_json::Value {
        serde_json::json!({
            "type": "conversation.typing",
            "qualified_conversation": {"id": "room-uuid", "domain": "wire.com"},
            "qualified_from": {"id": "alice-uuid", "domain": "wire.com"},
            "data": {"status": status},
        })
    }

    #[test]
    fn parses_started_into_qualified_room_sender_and_flag() {
        let parsed = parse_typing_event(&event("started")).expect("known status yields Some");
        assert_eq!(
            parsed,
            (
                "room-uuid@wire.com".to_string(),
                "alice-uuid@wire.com".to_string(),
                true,
            )
        );
    }

    #[test]
    fn parses_stopped_flag_as_false() {
        let (_, _, started) = parse_typing_event(&event("stopped")).expect("some");
        assert!(!started);
    }

    #[test]
    fn unknown_status_is_skipped() {
        assert!(parse_typing_event(&event("composing")).is_none());
    }

    #[test]
    fn missing_qualified_conversation_is_skipped_not_errored() {
        let ev = serde_json::json!({
            "type": "conversation.typing",
            "qualified_from": {"id": "a", "domain": "wire.com"},
            "data": {"status": "started"},
        });
        // Best-effort: a malformed typing frame is skipped, not escalated.
        assert!(parse_typing_event(&ev).is_none());
    }

    #[test]
    fn missing_status_is_skipped_not_errored() {
        let ev = serde_json::json!({
            "type": "conversation.typing",
            "qualified_conversation": {"id": "r", "domain": "wire.com"},
            "qualified_from": {"id": "a", "domain": "wire.com"},
            "data": {},
        });
        assert!(parse_typing_event(&ev).is_none());
    }
}

#[cfg(test)]
mod heal_trigger_tests {
    use std::sync::{Arc, Mutex as StdMutex, Weak};

    use super::{EventError, SyncService};
    use crate::error::KupferdrahtError;

    fn mls_add_event() -> serde_json::Value {
        serde_json::json!({
            "type": "conversation.mls-message-add",
            "qualified_conversation": {"id": "room-uuid", "domain": "wire.com"},
            "qualified_from": {"id": "bob-uuid", "domain": "wire.com"},
            "data": "AAAA",
        })
    }

    /// Step 106 (ADR-0034 d.2): since every MLS operation is one
    /// transaction, a storage failure is transient — retry the
    /// notification (ADR-0032), never skip it.
    #[test]
    fn a_storage_failure_of_the_mls_step_is_retried() {
        let failure = EventError::from_mls_step(KupferdrahtError::Storage("disk I/O error".into()));
        assert!(matches!(failure, EventError::Persist(_)));
        let other = EventError::from_mls_step(KupferdrahtError::MlsOutOfOrder);
        assert!(matches!(other, EventError::Other(_)));
    }

    /// Step 106 (ADR-0034 d.2): a storage failure never discards and
    /// rejoins the group.
    #[test]
    fn a_storage_failure_queues_no_heal() {
        let service = SyncService::new(Weak::new(), Arc::new(StdMutex::new(Vec::new())));
        service.note_failed_event(
            "notif-1",
            &mls_add_event(),
            &KupferdrahtError::Storage("disk I/O error".into()),
        );
        assert!(
            service
                .follow_ups
                .lock()
                .expect("follow_ups lock")
                .is_empty(),
            "no heal is queued for a storage failure"
        );
    }
}
