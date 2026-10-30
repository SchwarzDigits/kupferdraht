# Step 6-II — Timeline read + Wire's full text-messaging set

> **Status:** merged — Step 6-II closes end-to-end (live 2026-04-30 → 2026-05-01). 6-II-a Timeline-Read-API live 2026-04-30 (fixed FK-CASCADE history-wipe, CHANGELOG.md §32, V9); 6-II-b Quote/Reply live 2026-04-30 (V10; `quoted_message_sha256`, observations.md §33); 6-II-c Edit live 2026-04-30 (V11/V12 chain pointer, observations.md §34); 6-II-d Delete live 2026-04-30 (V13; tombstone); 6-II-e Reactions live 2026-04-30 (V14; full-set toggle); 6-II-f Read receipts live 2026-04-30 (V15, outbound + local; inbound deferred); 6-II-g Mentions live 2026-05-01 (V16). Archived from PLAN.md.
>
> **Sub-steps:** 6-II-a (read API), 6-II-b (Quote/Reply), 6-II-c (Edit), 6-II-d (Delete), 6-II-e (Reactions), 6-II-f (Read receipts), 6-II-g (Mentions).
>
> **Historical names:**
> - "Step 6-II" — current. (Filename slug uses `06-2` because Roman numerals are awkward in URLs and `ii` is a homograph for `11` at small font sizes.)
>
> **Branch / commit prefix:** `step/6-2-*` or `step/6-ii-*`, `feat(step-6-2): …`, `chore(step-6-2): …`.

---

## Step 6-II — Timeline read + Wire's full text-messaging set

Step 4 only sends `Text` and Step 5 only dispatches incoming text
through `EventHandler`. Step 6-II completes the text-messaging
feature set: a real readable `Timeline`, plus every Wire
`GenericMessage` content variant that participates in textual
communication (quote/reply, edit, delete, reaction, read receipt,
mention, analog to the matrix-rust-sdk patterns.

CLAUDE.md "Functional Scope" lists these as in-scope. Step 6-II
is where they land.

### matrix-rust-sdk analog

`matrix_sdk_ui::timeline::Timeline` is the structural reference
throughout 6-II. Kupferdraht's `Timeline` already exists as a
send-only handle from Step 4; 6-II grows it into the
matrix-rust-sdk-shaped read+write surface:

- `Timeline::items() -> Vec<TimelineItem>`
- `Timeline::subscribe() -> impl Stream<Item = TimelineDiff>`
- `Timeline::send_reply(parent_id, body)`
- `Timeline::edit(item_id, new_body)`
- `Timeline::redact(item_id)` (matrix term for delete)
- `Timeline::toggle_reaction(item_id, key)`
- `Timeline::send_single_receipt(receipt_type, item_id)`

Kupferdraht maps these onto Wire's `GenericMessage` variants instead
of Matrix events. The public-API names follow matrix-rust-sdk
(ADR-0009); the on-the-wire encoding follows Wire's protobuf.

Source references:

- `crates/matrix-sdk-ui/src/timeline/mod.rs` — `Timeline` surface.
- `crates/matrix-sdk-ui/src/timeline/event_item.rs` —
  `TimelineItem` shape and edit/redaction/reaction state.
- `crates/matrix-sdk-ui/src/timeline/reactions.rs` — toggle-reaction
  pattern.
- `crates/matrix-sdk-ui/src/timeline/event_handler.rs` — how
  edits/redactions/reactions are applied to existing items
  rather than appended.

### Pre-conditions (established earlier — not re-implemented here)

Step 6-II inherits these from earlier steps:

- `timeline_events` table with `message_id`, `room_id`,
  `sender_*`, `body_ct`, `kind`, `sent_at_ms`, `delivery_status`
  — built in **Step 4** (outgoing) and **Step 5** (incoming).
- `Timeline` send-only handle (`Timeline::send_text`) — built
  in **Step 4**.
- Polling-loop receive (`SyncService::run_polling`, dispatch
  through `EventHandler`) — built in **Step 5d**.

### Sub-steps

| Sub-step | Scope | GenericMessage variant | matrix-rust-sdk analog |
|---|---|---|---|
| 6-II-a | Read-API on `Timeline`: `items()`, `subscribe()`, `Room::messages()`. CLI `timeline --id <room>`. Foundation for everything that follows. | (none — read-only) | `Timeline::items`, `Timeline::subscribe` |
| 6-II-b | Quote / Reply — send a `Text` whose `quote.quoted_message_id` references an existing item. Render as inline quote in CLI. | `Text { quote: Quote { quoted_message_id, .. } }` | `Timeline::send_reply` |
| 6-II-c | Edit — replace the body of a previously sent message; receive path applies `MessageEdit` to the original item in `timeline_events`. | `MessageEdit { replacing_message_id, text }` | `Timeline::edit` |
| 6-II-d | Delete — delete a previously sent message; receive path marks the item as `redacted` in `timeline_events`. | `MessageDelete { message_id }` (Wire also has `Hidden`; Kupferdraht implements `MessageDelete` for "delete-for-everyone") | `Timeline::redact` |
| 6-II-e | Reactions — add / remove a single emoji reaction; per-message reaction-state aggregated in storage. | `Reaction { message_id, emoji }` | `Timeline::toggle_reaction` |
| 6-II-f | Read receipts — `Confirmation { type: READ, .. }` sent on `Timeline::mark_as_read`; receive path attaches receipts to existing items. | `Confirmation { type: READ, first_message_id, more_message_ids }` | `Timeline::send_single_receipt` |
| 6-II-g | Mentions — resolve `@handle` to qualified user IDs and emit `Text.mentions`; receive path renders mention spans in CLI output. | `Text { mentions: [Mention { ... }] }` | included in `Timeline::send` content |

6-II-a ships **first** because every later sub-step needs the
read-side data structure to apply edits / deletes / reactions /
receipts to. 6-II-b through 6-II-g may then land in any
convenient order; 6-II-c (edit) and 6-II-d (delete) are usually
paired because they share the same receive-side mutation pattern.

### Public API additions

```rust
impl Room {
    pub async fn messages(
        &self,
        opts: MessagesOptions,
    ) -> Result<Vec<TimelineItem>, KupferdrahtError>;
}

pub struct MessagesOptions {
    pub limit: u32,
    pub before: Option<String>,         // pagination cursor
}

impl Timeline {
    pub fn items(&self) -> Vec<TimelineItem>;

    pub async fn subscribe(
        self: Arc<Self>,
        listener: Arc<dyn TimelineListener>,
    ) -> TaskHandle;

    pub async fn send_reply(
        &self,
        parent_id: String,
        body: String,
    ) -> Result<MessageHandle, KupferdrahtError>;

    pub async fn edit(
        &self,
        item_id: String,
        new_body: String,
    ) -> Result<MessageHandle, KupferdrahtError>;

    pub async fn redact(
        &self,
        item_id: String,
    ) -> Result<(), KupferdrahtError>;

    pub async fn toggle_reaction(
        &self,
        item_id: String,
        key: String,
    ) -> Result<(), KupferdrahtError>;

    pub async fn mark_as_read(
        &self,
        up_to_item_id: String,
    ) -> Result<(), KupferdrahtError>;
}

pub struct TimelineItem {
    pub item_id: String,                // GenericMessage.message_id
    pub sender_user_id: String,
    pub sent_at_ms: i64,
    pub kind: TimelineItemKind,
    pub edited_at_ms: Option<i64>,
    pub redacted: bool,
    pub reactions: Vec<ReactionGroup>,  // aggregated
    pub read_by: Vec<String>,           // user IDs that confirmed READ
}

pub enum TimelineItemKind {
    Text {
        body: String,
        reply_to: Option<String>,
        mentions: Vec<Mention>,
    },
    Redacted,
}

pub struct ReactionGroup {
    pub key: String,                    // emoji
    pub senders: Vec<String>,           // user IDs
}

pub struct Mention {
    pub user_id: String,
    pub start: u32,                     // utf-16 offset, like Wire
    pub length: u32,
}

pub trait TimelineListener: Send + Sync {
    fn on_update(&self, diff: TimelineDiff);
}

pub enum TimelineDiff {
    Append(TimelineItem),
    UpdateAt { index: u32, item: TimelineItem },
    RemoveAt(u32),
    Reset(Vec<TimelineItem>),
}
```

### Wire API v14 endpoints

All sub-steps continue to use:

- `POST /v14/mls/messages` for outgoing application messages
  (every variant is just a different `GenericMessage.content`).

No new HTTP endpoints are introduced.

### Wire Message Types (per sub-step)

- 6-II-a: none (read-only over local `timeline_events`).
- 6-II-b: `GenericMessage { content: Text { quote: Quote { .. } } }`.
- 6-II-c: `GenericMessage { content: MessageEdit { .. } }`.
  Wire's `MessageEdit` carries the *new* `Text` plus
  `replacing_message_id` pointing at the original. The original
  message_id stays valid; the edit's own `message_id` is fresh.
- 6-II-d: `GenericMessage { content: MessageDelete { message_id } }`.
  "Delete for everyone". Wire also has `MessageHide` (delete-for-me
  only); Kupferdraht does not need it for the text-messaging set.
- 6-II-e: `GenericMessage { content: Reaction { message_id, emoji } }`.
  Empty `emoji` string means "remove my reaction".
- 6-II-f: `GenericMessage { content: Confirmation { type: READ,
  first_message_id, more_message_ids } }`. Wire batches receipts —
  one `Confirmation` can cover many messages.
- 6-II-g: `Mention { user_id, start, length }` entries inside
  `Text.mentions`.

### Store additions

Step 6-II extends `timeline_events` rather than introducing a
parallel table for each variant:

- `timeline_events.replaced_by_id` (nullable) — set when an
  inbound `MessageEdit` is applied.
- `timeline_events.redacted_at_ms` (nullable) — set when an
  inbound `MessageDelete` is applied.
- `timeline_events.reply_to_id` (nullable) — set on outgoing/
  incoming `Text` whose `quote.quoted_message_id` is non-null.

New tables:

- `reactions(message_id, sender_user_id, emoji, sent_at_ms)` —
  deduped per (message_id, sender_user_id, emoji). An empty-emoji
  reaction deletes any existing row for that sender on that
  message.
- `read_receipts(message_id, reader_user_id, confirmed_at_ms)` —
  one row per (message_id, reader). Inbound `Confirmation` with
  multiple message_ids fans out into multiple rows.

`Timeline::items()` joins these into the `TimelineItem` shape
above; `subscribe()` emits `TimelineDiff` whenever any of them
changes.

### Implementation notes

- **Edit and delete are processed by message_id, not by
  storage rowid.** Wire's `MessageEdit.replacing_message_id` and
  `MessageDelete.message_id` are the GenericMessage UUID. If
  the original isn't in `timeline_events` (we joined the room
  later), we still record the edit/delete as a "pending"
  mutation that applies if the parent ever arrives via
  pagination.
- **Reactions are pure application messages** — no MLS
  Add/Remove. Same encrypt path as `send_text`.
- **Read receipts opt-in.** `mark_as_read` sends a
  `Confirmation`; the polling-loop receive path applies inbound
  ones to the `read_receipts` table. Kupferdraht does not auto-send
  receipts on `Timeline::items()` — the application chooses
  when to flush.
- **Mentions use UTF-16 offsets** because Wire's protobuf field
  semantics match what iOS / web emit (UTF-16 code units, not
  byte offsets, not grapheme clusters).
- The receive path for *every* mutating variant must be
  idempotent — Wire can redeliver during reconnects (LEARNINGS
  §2.5). Use `INSERT OR IGNORE` / `ON CONFLICT DO NOTHING`.

### TDD starter tests

6-II-a:
- `Timeline::items()` returns `timeline_events` rows for the
  room in `sent_at_ms` order.
- `Timeline::subscribe` emits `Append` for a freshly-sent
  outgoing message.
- `Room::messages(limit=10)` paginates oldest-first with a
  `before` cursor.

6-II-b:
- Outgoing `send_reply(parent_id, body)` produces a
  `GenericMessage` with `content::Text { quote: Quote {
  quoted_message_id: parent_id, .. } }` round-trippable via
  `prost`.
- Inbound text with `quote` populates
  `TimelineItem::reply_to`.

6-II-c:
- Outgoing `Timeline::edit(item_id, new_body)` produces
  `GenericMessage { content: MessageEdit { replacing_message_id:
  item_id, text: Text { content: new_body, .. } } }`.
- Inbound `MessageEdit` updates the original
  `timeline_events` row's `replaced_by_id` and exposes the
  new body via `TimelineItem::kind::Text`.
- Inbound `MessageEdit` for an unknown parent stores a pending
  edit; a later parent insert applies it.

6-II-d:
- Outgoing `Timeline::redact(item_id)` produces
  `GenericMessage { content: MessageDelete { message_id:
  item_id } }`.
- Inbound `MessageDelete` flips the row to
  `TimelineItemKind::Redacted` and clears the body.

6-II-e:
- `toggle_reaction("👍")` first-time emits
  `Reaction { emoji: "👍" }`, second-time emits
  `Reaction { emoji: "" }` (Wire's "remove" encoding).
- Inbound `Reaction` populates / removes a row in `reactions`
  and updates `TimelineItem::reactions`.

6-II-f:
- `mark_as_read(item_id)` emits
  `Confirmation { type: READ, first_message_id: item_id,
  more_message_ids: [...] }` covering every unconfirmed message
  up to `item_id`.
- Inbound `Confirmation` populates `read_receipts` rows for
  every message_id it covers.

6-II-g:
- `send_text("Hi @alice!")` resolves `@alice` via
  `POST /v14/list-users` and emits a `Text` with one
  `Mention { user_id, start, length }` covering the handle.
- Inbound mention spans are exposed via
  `TimelineItem::kind::Text::mentions`.

### CLI commands

```
kupferdraht-cli timeline       --id <room-id> [--limit N] [--before <id>]
kupferdraht-cli reply          --id <room-id> --to <message-id> -m "body"
kupferdraht-cli edit           --id <room-id> --message <id> -m "new body"
kupferdraht-cli delete-message --id <room-id> --message <id>
kupferdraht-cli react          --id <room-id> --message <id> --emoji "👍"
kupferdraht-cli unreact        --id <room-id> --message <id> --emoji "👍"
kupferdraht-cli mark-read      --id <room-id> --message <id>
```

`send` keeps its Step 4 form. `--message <id>` accepts the
GenericMessage UUID (column `timeline_events.message_id`).

### Smoke test

```bash
# Send + read back
MSG=$(kupferdraht-cli send --id "$ROOM" -m "step-6-II baseline" \
        | awk '/message_id/ {print $2}')
kupferdraht-cli timeline --id "$ROOM" --limit 5

# Quote / reply
kupferdraht-cli reply --id "$ROOM" --to "$MSG" -m "quoting baseline"

# Edit + delete
EDIT_TARGET=$(kupferdraht-cli send --id "$ROOM" -m "to be edited" \
                | awk '/message_id/ {print $2}')
kupferdraht-cli edit          --id "$ROOM" --message "$EDIT_TARGET" -m "edited"
DEL_TARGET=$(kupferdraht-cli send --id "$ROOM" -m "to be deleted" \
                | awk '/message_id/ {print $2}')
kupferdraht-cli delete-message --id "$ROOM" --message "$DEL_TARGET"

# Reactions + read receipts
kupferdraht-cli react     --id "$ROOM" --message "$MSG" --emoji "👍"
kupferdraht-cli unreact   --id "$ROOM" --message "$MSG" --emoji "👍"
kupferdraht-cli mark-read --id "$ROOM" --message "$MSG"

kupferdraht-cli timeline --id "$ROOM" --limit 10
# on the counterpart (iOS / desktop): each operation appears
# correctly — quote shows the parent, edit replaces the body,
# delete tombstones the message, reactions toggle, READ
# receipt updates the seen-by indicator.
```
