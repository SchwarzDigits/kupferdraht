# Step 21 — Typing indicators

> **Status:** implemented on `step/21-typing-indicators` (scope reversal
> approved by Volker 2026-06-11; `CLAUDE.md` and
> `docs/v1-known-simplifications.md` §1.13 flipped in the same change).
> Option A (drop-in), both directions, send-side auto-stop. Merged:
> `POST /conversations/{d}/{c}/typing` (`started`/`stopped`); inbound
> `conversation.typing` → client-derived live set + send-side auto-stop;
> ephemeral, not a `GenericMessage`. `a786b63` (#7) + `307aa21` (#8). Archived from PLAN.md.
>
> ⚠️ **Send works now; receive is dormant until the WebSocket lands.**
> Wire delivers `conversation.typing` as a *transient* event over the
> live `/v14/events` WebSocket only — it is **not** carried by the
> `/v14/notifications` pull queue Kupferdraht uses today (the v14
> `QueuedNotification` schema has no `transient` field — transient
> events are a live-stream artifact, not stored in the pull queue). The
> WebSocket is "target for the rewrite" (`docs/wire/api-notes.md` §7) and
> not built. The receive path (`dispatch_inbound_typing` → `on_typing`)
> is wired + unit-tested but will not fire in production until the push
> transport exists. **For the natrium drop-in this means typing-receive
> regresses vs. natrium's current real-time receive until Kupferdraht gains
> `/v14/events`.**
>
> **Sub-steps:** 21 (outbound + send-side auto-stop), 21b (inbound set + clear), 21c (observer + FFI).
>
> **Historical names:**
> - "Step 21" — proposed.
>
> **Branch / commit prefix:** `step/21-*`, `feat(step-21): …`.

---

## Step 21 — Typing indicators

Wire's typing indicator: a per-conversation, ephemeral
"user X is typing" signal. Send `started` / `stopped`; observe the
current set of typing users per room. **Not** MLS-encrypted, **not**
a `GenericMessage` — a plaintext backend fan-out, orthogonal to the
crypto core.

### Why

Driven by the natrium-ion migration (the KMP facade moving onto Kupferdraht).
natrium exposes typing in **both** directions (`sendTypingStarted` /
`sendTypingStopped` / `observeTyping`). A true drop-in requires Kupferdraht to
provide the same — including the client-side timer logic, since the Wire
backend runs no typing timer of its own.

### matrix-rust-sdk analog

`Room::typing_notice(bool)` (send) and the `RoomEvent`/ephemeral
typing notification (receive). Matrix carries the current typing user
list in an ephemeral event; Wire carries discrete `started`/`stopped`
signals, so Kupferdraht derives the set client-side.

### Backend / client mechanism

- **Backend has no timer.** Wire fans out `started`/`stopped` verbatim;
  the v14 schema carries no TTL field. All timing is client-side.
- **Send side owns the only timer.** After sending `started`, the client
  auto-enqueues a `stopped` after the auto-stop window
  (`TYPING_AUTOSTOP`), with per-room dedupe so repeated `started` calls
  collapse to one in-flight window. The app calls only `started`.
- **Receive side keeps no per-user timer.** A per-room set of typing
  users; a user is removed when their `stopped` arrives (reliable thanks
  to the send-side auto-stop) or by a wholesale clear at sync-session
  start — a safety net against an orphaned indicator (e.g. a sender that
  crashed).
- Wire has a user-level "typing-indicator enabled" property; natrium does
  not surface it, so it is not required for parity — noted only for
  completeness.

### Design options

Two coupled axes. The first of each is the recommended choice
(drop-in, both directions).

- **Direction:** *both* (recommended) · receive-only (~1 day, cheaper)
  · send-only.
- **Where state + timer live:**
  - **A — Drop-in (recommended).** Kupferdraht owns the send-side auto-stop +
    dedupe **and** the receive-side per-room set + clear-on-sync-change.
    The observer delivers the ready-made set
    (`TypingEvent { room_id, typing_users }`). natrium maps 1:1. ~2 days.
  - **B — Primitives.** Kupferdraht forwards raw only —
    `send_typing(started|stopped)` and
    `on_typing(room, user, started|stopped)`. natrium re-implements the
    sender handler (timer) and the incoming set/clear. Leaner Kupferdraht,
    more work in natrium, not a drop-in. ~1–1.5 days in Kupferdraht plus
    natrium-side work.

The rest of this document specifies **Option A**.

### Public API (Option A)

```rust
impl Timeline {
    /// Send `started` and auto-enqueue a `stopped` after
    /// `TYPING_AUTOSTOP`. Repeated calls while one is in flight are
    /// deduped.
    pub async fn send_typing_started(&self) -> Result<(), KupferdrahtError>;

    /// Send `stopped` immediately and cancel the pending auto-stop.
    pub async fn send_typing_stopped(&self) -> Result<(), KupferdrahtError>;
}

/// Delivered to `EventHandler::on_typing` — the current set of users
/// typing in a room (derived client-side from started/stopped).
pub struct TypingEvent {
    pub room_id: String,
    // Flat qualified-id strings, sorted — matching the existing
    // `Message::sender_user_id` convention on the EventHandler surface
    // (NOT `Vec<QualifiedUserId>`, for consistency within that trait).
    pub typing_users: Vec<String>,
}

pub trait EventHandler: Send + Sync {
    // … existing methods …
    /// Default no-op; fired whenever a room's typing set changes.
    fn on_typing(&self, _event: TypingEvent) {}
}
```

### FFI surface (Option A)

Per the Public API ↔ FFI parity rule, the same commit updates
`kupferdraht-core/src/ffi/`:

- `ffi/timeline.rs`: `send_typing_started()` / `send_typing_stopped()`.
- `ffi/sync.rs`: a twin method `on_typing` on the foreign-facing
  `EventHandler` trait, an adapter forward, and a `TypingEvent`
  `uniffi::Record` (`typing_users: Vec<String>` — flat qualified ids,
  matching `Message::sender_user_id`).

### Wire API v14 endpoints

- `POST /v14/conversations/{domain}/{conv}/typing` — body
  `{"status": "started" | "stopped"}`, `200` with no body. Plaintext
  REST, no MLS. Mirrors the `rename_conversation` call shape.
- Inbound event `conversation.typing` (a sibling event, **not** nested
  in `conversation.mls-message-add`): fields `qualified_conversation`
  (room id), `qualified_from` (sender), `data.status`
  (`started`/`stopped`). No protobuf decode.

### RFC 9420 sections

None — typing is Wire-transport-only and never touches the MLS group
or any encrypted payload.

### Wire Message Types

None — not a `GenericMessage` content variant. Pure transport event,
not persisted to the timeline.

### Implementation slices (each TDD: red → approval → green)

1. **Outbound + send-side auto-stop** (the "timer"):
   `wire/http/conversations.rs` `send_typing(...)`; `client/runtime.rs`
   `http_send_typing(...)`; a new sender handler
   (`tokio::time::sleep(TYPING_AUTOSTOP)` + `Mutex<HashMap<RoomId, ()>>`
   dedupe; explicit `stopped` cancels and clears the queue); FFI methods.
   Keep time injectable (tokio time pause) for deterministic tests.
2. **Inbound set + clear** (no per-user timer):
   `sync/service.rs` `"conversation.typing"` match arm +
   `dispatch_inbound_typing()` (extract room/sender/status); a per-room
   `HashMap<RoomId, HashSet<UserId>>` (`started` inserts, `stopped`
   removes, **clear-all at sync-session start**); emit the current set on
   every change.
3. **Observer + FFI:** `EventHandler::on_typing` (core default no-op) +
   `TypingEvent`; `Client::notify_typing()`; the `ffi/sync.rs` twin +
   adapter + record.

### Dependencies

- Step 5 (sync receive / event dispatch) — extends `dispatch_event`.
- Step 20c (FFI surface / observer twin-trait pattern) — extends
  `EventHandler` and its adapter.
- Independent of the MLS/crypto path entirely.

### Effort

~2 days for Option A (both directions, Kupferdraht-owned send-side timer +
receive set), including the TDD cycle, FFI parity, doc freshness, and a
Kotlin/Swift binding smoke. ~1 day for receive-only; the send-side
auto-stop adds ~0.5 day over a raw-events-only build.

### Documentation touched (same commit)

`docs/library.md` (new `pub` API), `docs/bindings.md` (FFI surface),
`docs/api-mapping-matrix-rust-sdk.md` (new public symbol → matrix
analog), and — as the scope reversal — `CLAUDE.md` and
`docs/v1-known-simplifications.md` §1.13.

### Verification

- **Send:** mock HTTP asserts path + body
  (`{"status":"started"}`, correct `<uuid>@<domain>` split).
- **Send auto-stop:** `started` → automatic `stopped` after
  `TYPING_AUTOSTOP`; dedupe swallows a second in-flight `started`;
  explicit `stopped` drains the queue.
- **Receive:** event JSON → dispatch → observer set correct
  (add/remove); a sync-session-start clear empties every room.
- **FFI:** adapter forwards; `TypingEvent` record maps core → FFI.
- Full gate: `cargo fmt --check`, build 0 warnings,
  `cargo clippy -D warnings`, `cargo test`, `cargo deny check`.
- Binding smoke: Kotlin + Swift see `send_typing_*` and
  `on_typing(TypingEvent)`.
