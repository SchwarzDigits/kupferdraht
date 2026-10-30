# Step 34 — Find message by id

> **Status:** MERGED to `main` — `7697f04`, PR #28 (squash-merge, no
> separate merge commit), 2026-06-17. Scope: `Client::find_message_by_id`,
> a local by-id timeline lookup closing the single-message read gap
> surfaced by the natrium-ion reply/quote (B9) analysis. No dated
> live-smoke record (local-only store read; the feature never touches the
> network, so its own TDD section lists store-level tests only).
>
> **Sub-steps:** none.
>
> **Historical names:**
> - "Step 34" — current.
>
> **Branch / commit prefix:** `step/34-*`, `feat(step-34): …`.

---

## Step 34 — Find message by id

Add a local, by-id lookup of a single timeline item:
`Client::find_message_by_id(message_id) -> Option<TimelineItem>`. Until now
the only timeline reads were list-shaped (`Room::messages`, `Timeline::items`)
and `Client::find_message_room` (which only returns the owning room id, not the
item). There was no way to resolve one message — by its stable id or by an
edit-event id — to its content + sender without scanning a whole room.

### Why

The natrium-ion migration flagged that **replies render as normal messages**:
Kupferdraht's `TimelineItemKind::Text` carries only `reply_to: Option<String>` (the
parent id), whereas Kalium handed the wrapper a fully-resolved
`QuotedMessageDetails` (sender name + a typed content preview). To rebuild the
quote preview, natrium needs to resolve the parent by id. natrium already loads
the full room timeline, but a direct by-id getter is the clean primitive: it
works regardless of the caller's loaded window and avoids the wrapper building
an id→item map. This is the read-side mirror of the existing
`find_message_room` (same id-resolution semantics), and a generally useful
FFI primitive.

### matrix-rust-sdk analog

`Timeline::item_by_id` / the event cache's `get_event` — resolve a single
event by id from the local store. Drift: matrix keys events globally; Kupferdraht's
`timeline_events` table is keyed `(message_id, room_id)`, and the same
`target_id` may also match an edit-event id via `current_edit_event_id`
(Wire's edit chain), exactly as `find_message_room` already handles.

### Semantics

- **Local-only.** Reads `timeline_events`; never hits the network. Returns
  `None` when the id is not in the local store (or no `SqliteStore` is
  attached). A parent that was never synced/pruned is simply absent — Wire's
  `Quote` carries only a SHA-256 digest, not a preview, so there is nothing to
  fetch.
- **Id resolution** mirrors `find_message_room`: matches a stable `message_id`
  **or** an edit-event id in `current_edit_event_id`. On the (astronomically
  unlikely) duplicate id across rooms, returns the first row.
- **Lightweight / no overlays.** Returns the item built straight from the
  stored row (`TimelineItem::from_record`): `kind` (body / asset / location / …),
  `sender_user_id`, `sent_at_ms`, `edited_at_ms`, `redacted`. Overlay
  collections (`reactions`, `read_by`, `delivered_to`) are **empty** — this is a
  point lookup for content + sender (the quote-preview use case), not a fully
  hydrated timeline read. Use `Room::messages` for an overlay-hydrated item.
- A **redacted** parent still resolves (its row survives with a cleared body and
  `redacted = true`), so a quoting message can render "quoting a deleted
  message" rather than orphaning the reply.

### Public API

```rust
impl Client {
    /// Resolve a single timeline item by id from the local store.
    /// `message_id` may be a stable id or an edit-event id (Wire's
    /// edit chain), matching `find_message_room`. Returns `None`
    /// when the id is not stored locally (or no store is attached).
    ///
    /// Lightweight: the returned item carries no overlay data
    /// (reactions / read receipts are empty). Use `Room::messages`
    /// for a fully hydrated item.
    pub async fn find_message_by_id(
        &self,
        message_id: &str,
    ) -> Result<Option<TimelineItem>, KupferdrahtError>;
}
```

Exposed over UniFFI on the FFI `Client` (→ `findMessageById` in the bindings),
returning `Option<TimelineItem>` (the existing FFI record).

Store trait gains the read primitive it delegates to:

```rust
async fn load_timeline_event(
    &self,
    target_id: &str,
) -> Result<Option<TimelineEventRecord>, KupferdrahtError>;
```

### TDD

Tested at the store layer (`sqlite/timeline_events.rs`), matching the
`find_message_room` precedent — the SQL is the logic; the `Client`/FFI
wrappers are thin and the same shape as the already-shipped, store-tested
`find_message_room`:
- returns the row for a known stable `message_id`
- resolves an edit-event id via `current_edit_event_id`
- returns `None` for an unknown id

The `Client::find_message_by_id` wrapper guards on a missing store (`None`)
and maps via the `from_record` path already covered by `timeline/items.rs`
tests.

### Implementation notes

- Reuses `read_raw_timeline_row` + `assemble_timeline_event`; the only new SQL
  is `… WHERE message_id = ?1 OR current_edit_event_id = ?1 LIMIT 1`.
- Lives alongside `find_message_room` in `client/message_lookup.rs` and the
  existing FFI `Client` impl block in `ffi/admin.rs`.
