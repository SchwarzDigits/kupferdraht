# Step 23 — Team-delete (delete a conversation for everyone)

> **Status:** merged on `step/23-team-delete`. Closes the
> "delete conversation" gap (POC spec §3, "… or deleting
> conversations") from the natrium migration. `DELETE /teams/{tid}/conversations/{cid}`;
> inbound `conversation.delete` prunes the room cache. `045d894` (#10). Archived from PLAN.md.
>
> **Sub-steps:** none.
>
> **Historical names:**
> - "Step 23" — current.
>
> **Branch / commit prefix:** `step/23-*`, `feat(step-23): …`.

---

## Step 23 — Team-delete

Delete a conversation for **all** members (a team-admin action),
distinct from leaving it. Closes the last conversation-management gap
for the natrium drop-in (`ConversationOperations.delete()`).

### Why

The POC statement of work (§3, "… or deleting conversations")
requires deleting conversations, not just leaving them. Kupferdraht had only
`leave` / `leave_room`.

### matrix-rust-sdk analog

None — Matrix has no "delete room for everyone" (you leave / forget).
Wire extension.

### Wire API v14 endpoints

- `DELETE /v14/teams/{tid}/conversations/{cid}` — both ids are
  *unqualified* UUIDs; `200` (deleted) / `403` (insufficient role) /
  `404` (unknown); no body. A `404` is treated as **idempotent success**
  (already gone). Wire emits a `conversation.delete` event to the
  remaining members.

### Owning team id

The endpoint is team-scoped. `delete_room` fetches the conversation
(`list_conversations`) and reads its `team` field — **but Wire's
conversation-list response frequently omits `team`** (confirmed against
live Wire). So it **falls back to the signed-in user's own team**
(`GET /self` → `SelfUser.team`), which is also what other Wire clients
use: deleting a team conversation requires team-admin rights, i.e. it is
your own team. An account in no team yields `InvalidInput`. No
`Room`/schema change, no migration.

### Public API

```rust
impl Client {
    pub async fn delete_room(&self, room_id: &str) -> Result<(), KupferdrahtError>;
}
impl Room {
    /// Delegates to `Client::delete_room`.
    pub async fn delete(&self) -> Result<(), KupferdrahtError>;
}
```

FFI: `ffi/room.rs` `Room::delete()` + `Client::delete_room(room_id)`.

### CLI

`kupferdraht-cli room-delete --id <room>@<domain>` — mirrors `room-leave`;
resumes the client and calls `Client::delete_room`. The live-validation
path for the two open assumptions (is `team` populated in the
conversations-list response? does the `conversation.delete` event fire?).

### Flow

`delete_room`: parse qualified id → fetch conversation → read `team`
(`None` → `InvalidInput`) → `DELETE /v{N}/teams/{team}/conversations/
{local-id}` → `sync_metadata` (drops the room from the cache).

### Error mapping

`403` → `BackendRejected` with the Wire label; `404` (already gone) →
idempotent success. A non-team conversation → typed `InvalidInput`
before any network call.

### Inbound `conversation.delete` handling

When another admin deletes a conversation, Wire emits a durable
`conversation.delete` event (delivered via the `/notifications` pull
queue — it is not transient, so no WebSocket is required). `dispatch_event`
now prunes the room from the in-memory cache (`Client::remove_cached_room`)
instead of silently skipping it, so the deleted room drops out promptly.
The persisted snapshot self-corrects on the next `sync_metadata`. This is
local-state hygiene only — a live consumer push (an `on_room_deleted`
observer) is deferred to the observe-conversations work.

### RFC 9420 sections / Wire Message Types

None — pure conversation-admin REST, no MLS, not a `GenericMessage`.

### Dependencies

- Step 6e (leave / logout) — same Room-mutator pattern.
- Step 2 (rooms list / `OwnConversation` parse) — extended with the
  `team` field.

### Verification

- `wire/http/conversations.rs` httpmock: team-scoped path + `200` ok,
  `403` / `404` → `BackendRejected`.
- Full gate + Kotlin/Swift binding smoke (`delete` / `delete_room`
  present).
- Live (optional): delete a team conversation → `200` +
  `conversation.delete` event.
