# Step 5 — SyncService: receive

> **Status:** merged — Step 5 (SyncService receive, poll loop) `e8ef09d` (5a–5c) + 5d continuous poll, live 2026-04-30; long-poll surface, WebSocket dropped (CHANGELOG.md §30). Archived from PLAN.md.
>
> **Sub-steps:** 5a, 5b, 5c, 5d (sub-step decomposition evolved during merge — 5a–5c shipped pull, 5d added the continuous-poll loop).
>
> **Historical names:**
> - "Step 5" — current.
>
> **Branch / commit prefix:** `step/5-*`, `feat(step-5): …`, `chore(step-5): …`.

---

## Step 5 — SyncService: receive

Long-lived WebSocket to Wire's v14 event stream, per-event
ack via `ConsumableNotifications`, decrypt MLS application
messages, dispatch to the consumer's `EventHandler`.

Decomposed into **three sub-steps** so the WebSocket
lifecycle, the ack/resume protocol, and the MLS-decrypt
dispatch land independently. Each produces a runnable
state: 5a shows raw frames, 5b shows reliably-acked +
reconnecting frames, 5c shows decrypted text arriving in
`EventHandler::on_message`.

### matrix-rust-sdk analog

`Client::sync()` / `SyncService::start()`. Event handlers
registered via `Client::add_event_handler`. Kupferdraht's shape
mirrors this exactly.

Source references:

- `crates/matrix-sdk/src/sync.rs` — sync loop patterns.
- `crates/matrix-sdk-ui/src/sync_service.rs` — service
  lifecycle, start/stop semantics, reconnect handling. **This
  is the primary adaptation target for Step 5** (Apache-2.0,
  direct code adaptation permitted per CLAUDE.md).

### Public API

```rust
impl Client {
    pub async fn sync(&self) -> Arc<SyncService>;
    pub fn add_event_handler(&self, handler: Arc<dyn EventHandler>);
}

pub struct SyncService { /* ... */ }

impl SyncService {
    pub async fn run(&self) -> Result<(), KupferdrahtError>;     // blocking
    pub async fn stop(&self) -> Result<(), KupferdrahtError>;
}

pub trait EventHandler: Send + Sync {
    fn on_message(&self, message: Message);
    fn on_room_invited(&self, room_id: String);
    fn on_sync_error(&self, description: String);
}

pub struct Message {
    pub room_id: String,
    pub sender_user_id: String,
    pub sender_device_id: Option<String>,
    pub body: MessageBody,
    pub received_at_ms: i64,
    pub message_id: String,
}

pub enum MessageBody {
    Text(String),
    // Unknown variants logged + discarded; not surfaced as a
    // distinct variant so consumers don't need to handle them.
}
```

UDL:

```idl
interface Client {
    // ...
    [Async]
    SyncService sync();
    void add_event_handler(EventHandler handler);
};

interface SyncService {
    [Async, Throws=KupferdrahtError]
    void run();

    [Async, Throws=KupferdrahtError]
    void stop();
};

[Trait]
interface EventHandler {
    void on_message(Message message);
    void on_room_invited(string room_id);
    void on_sync_error(string description);
};

dictionary Message {
    string room_id;
    string sender_user_id;
    string? sender_device_id;
    MessageBody body;
    i64 received_at_ms;
    string message_id;
};

[Enum]
interface MessageBody {
    Text(string body);
};
```

### Sub-steps

| Sub-step | Scope | Wire API v14 | RFC 9420 | Store additions |
|---|---|---|---|---|
| 5a | WebSocket connect with `?access_token=<token>` query param (Bearer headers are stripped by Wire's nginx — LEARNINGS §3), frame parse, dispatch to a dumb internal sink. No ack, no decrypt, no reconnect. | `GET /v14/events` (WebSocket, 101 Switching Protocols) | §6 (MlsMessage framing) | `sync_state` stub (connection metadata) |
| 5b | Per-event `EventAcknowledgeRequest` sent back over the socket after each frame is consumed, sync-marker persistence across restarts, reactive 401 → `POST /v14/access` → reconnect, `/v14/notifications` backfill on cursor expiry. | `EventAcknowledgeRequest` (inline on socket), `GET /v14/notifications?since=<id>&client=<device_id>&size=100` | — | `sync_state` gains `sync_marker_uuid`, `last_event_id`, `last_connected_at` |
| 5c | MLS decrypt + GenericMessage dispatch. `conversation.mls-message-add` → `MlsClient::decrypt_message` → `GenericMessage` → `EventHandler::on_message` (Text only). `conversation.mls-welcome` → `process_welcome_message` → `on_room_invited`. Unknown oneof variants logged + discarded. | — | §6.3 (PrivateMessage), §12.1.1 (Welcome), §12.4 (Commit processing) | `timeline_events` gains inbound rows with `delivery_status='received'` |

### Wire API v14 endpoints

Consolidated list:

- `GET /v14/events?client=<device_id>&sync_marker=<uuid>&access_token=<token>`
  — WebSocket, 101 Switching Protocols, ConsumableNotification
  frames (§7).
- Per-event `EventAcknowledgeRequest` sent back over the
  same socket (§7 — exact frame shape confirmed in
  docs/observations.md when first live-tested).
- `GET /v14/notifications?since=<id>&client=<device_id>&size=100`
  — REST backfill for missed events on reconnect (§7).

### RFC 9420 sections

- §6 — Framing (MlsMessage envelope).
- §6.3 — PrivateMessage (incoming application messages).
- §12.4 — Commit processing (server-forwarded commits from
  other members).
- §12.1.1 — Welcome handling (new room memberships).

### Wire Message Types

**`GenericMessage` Protobuf is mandatory here** (CLAUDE.md).
Inbound flow:

1. Event type is `"conversation.mls-message-add"`.
2. `data` field is base64-encoded TLS-serialized
   `MlsMessage(PrivateMessage)`.
3. `MlsClient::decrypt_message` hands back the plaintext.
4. Plaintext is deserialized as `GenericMessage`.
5. If `content == Some(Text(...))` → emit
   `EventHandler::on_message` with `MessageBody::Text`.
6. Any other oneof variant → log + discard, no handler fire.
7. Ack the event on the socket regardless (consumed = we
   processed it, even if we don't surface it).

Welcome flow:

1. Event type is `"conversation.mls-welcome"`.
2. `data` is base64-encoded `MlsMessage(Welcome)`.
3. `MlsClient::process_welcome_message` absorbs the group
   state into the local MLS provider.
4. Update `rooms` table.
5. Emit `EventHandler::on_room_invited`.

All other event types (team, user, feature-config, …) are
acknowledged and skipped.

### Store additions

- `sync_state` table — `sync_marker_uuid`, `last_event_id`,
  `last_connected_at`.
- Inbound decrypted messages go into `timeline_events` with
  `delivery_status='received'`.

### Implementation notes

- **Reactive 401 retry.** Long-lived connection will
  outlive the 10-min proactive token refresh. On auth
  failure at stream level: re-run `POST /v14/access`,
  reconnect with fresh token. Unified with the HTTP-layer
  401 retry helper (cross-cutting infrastructure).
- **Per-event ack placement.** Ack AFTER the handler
  returns, not before. A consumer crash loses at most one
  event.
- **Sync marker persistence.** `sync_marker_uuid` stored in
  `sync_state` so reconnect can resume without full
  `/notifications` backfill.
- **Backfill policy.** On reconnect, if sync marker is
  rejected by Wire (cursor expired), fall back to
  `GET /v14/notifications?since=<last_event_id>&size=1000`
  until caught up, then resume WebSocket.

### TDD starter tests

5a:
- WebSocket upgrade to `/v14/events?...&access_token=<t>`
  returns 101 and delivers binary frames.
- Frame parser yields structured `ConsumableNotification`
  values; malformed frames surface via `on_sync_error`.
- `SyncService::run` returns `Ok(())` on clean server-side
  close.

5b:
- Per-event ack is dispatched AFTER the consumer is
  notified, BEFORE the next frame is read.
- `sync_marker_uuid` persists in `sync_state`; a second
  `run()` resumes from it without re-reading consumed
  events.
- 401 mid-stream triggers `POST /v14/access`, new token,
  reconnect — without surfacing the 401 to the consumer.
- Cursor-expired 4xx falls back to
  `GET /v14/notifications?since=<last_event_id>` until
  caught up, then resumes WebSocket.

5c:
- `conversation.mls-message-add` with a text GenericMessage
  fires `EventHandler::on_message` with `MessageBody::Text`.
- `GenericMessage` with unknown oneof variant is
  acknowledged but not dispatched to `on_message`.
- `conversation.mls-welcome` event persists the room via
  `process_welcome_message` and fires `on_room_invited`.
- Handler failures do NOT stop the loop — error surfaces via
  `on_sync_error` and stream continues.

### CLI command

```
kupferdraht-cli sync-stream --format pretty|json [--room <id>]
```

Renamed from the first-pass `listen` to match the SyncService
naming.

### Smoke test

```bash
# Terminal 1:
kupferdraht-cli sync-stream --format pretty &
LISTEN=$!

# Terminal 2 (same account, or counterpart iOS):
kupferdraht-cli send --id <room-id> -m "round-trip test"

# Terminal 1 must print the received message within ~2 seconds.
kill "$LISTEN"
```

End-to-end validation: Kupferdraht-CLI sends → Wire delivers →
Kupferdraht-CLI receives and decrypts.

### CLI commands

**Step 5 — sync stream:**

| Command | Options |
|---|---|
| `sync-stream` | `--format pretty\|json`, `--room <id>` optional filter |
