# Step 2 — Rooms list + metadata sync

> **Status:** merged — Step 2 (Rooms list + `sync_metadata`) `2e13eda` (live 2026-04-24); opportunistic 2g (Mutable account / room metadata) live 2026-04-30 (`profile-set` / `room-rename`). Archived from PLAN.md.
>
> **Sub-steps:** (none — single-cycle step). + opportunistic 2g (Mutable account / room metadata).
>
> **Historical names:**
> - "Step 2" — current. (No renames.)
>
> **Branch / commit prefix:** `step/2-*`, `feat(step-2): …`, `chore(step-2): …`.

---

## Step 2 — Rooms list + metadata sync

Fetch the user's rooms from Wire and materialize them in the
local Store. One-shot metadata sync; live event handling is
Step 5.

### matrix-rust-sdk analog

`Client::rooms()` returns the `Room` list; `Client::sync_once()`
is the one-shot metadata sync. Kupferdraht mirrors both.

Source references:

- `crates/matrix-sdk/src/room/mod.rs` — `Room` public API
  shape.
- `crates/matrix-sdk-base/src/rooms/mod.rs` — room state
  cache patterns.

### Public API (Rust + UDL)

```rust
// kupferdraht-core/src/rooms/mod.rs
impl Client {
    pub async fn rooms(&self) -> Vec<Arc<Room>>;
    pub async fn room(&self, room_id: String) -> Option<Arc<Room>>;
    pub async fn sync_metadata(&self) -> Result<(), KupferdrahtError>;
}

pub struct Room { /* ... */ }

impl Room {
    pub fn id(&self) -> String;
    pub fn name(&self) -> String;
    pub fn room_type(&self) -> RoomType;
    pub fn epoch(&self) -> u64;
    pub async fn members(&self) -> Result<Vec<RoomMember>, KupferdrahtError>;
    pub fn counterpart(&self) -> Option<RoomMember>;
}

pub enum RoomType { DirectMessage, Group, Channel, SelfRoom }

pub struct RoomMember {
    pub user_id: String,
    pub is_self: bool,
    pub display_name: Option<String>,
    pub handle: Option<String>,
}
```

UDL:

```idl
interface Client {
    // ... from Step 1

    [Async]
    sequence<Room> rooms();

    [Async]
    Room? room(string room_id);

    [Async, Throws=KupferdrahtError]
    void sync_metadata();
};

interface Room {
    string id();
    string name();
    RoomType room_type();
    u64 epoch();
    RoomMember? counterpart();

    [Async, Throws=KupferdrahtError]
    sequence<RoomMember> members();
};

enum RoomType { "DirectMessage", "Group", "Channel", "SelfRoom" };

dictionary RoomMember {
    string user_id;
    boolean is_self;
    string? display_name;
    string? handle;
};
```

### Wire API v14 endpoints

- `POST /v14/conversations/list-ids` — paginated qualified
  conversation IDs (§5).
- `POST /v14/conversations/list` — bulk metadata for a list
  of IDs (§5).
- `POST /v14/list-users` (qualified-id shape) — resolve
  display names / handles for 1:1 counterparts (§8).

### RFC 9420 sections

None directly in Step 2 — it's metadata-only. RFC types
appear in `epoch` field (MLS GroupContext §8).

### Store additions

- `rooms` table — room_id, group_id (MLS), ciphersuite,
  epoch, name_ct, room_type, protocol, counterpart_user_id,
  last_synced_at.
- `room_members` table — room_id, user_id, is_self,
  display_name_ct, handle.

### Implementation notes

- Map Wire's `type` integer (0=group, 1=self, 2=direct,
  3=channel) to `RoomType` at the `wire::dto` → `Room`
  boundary. Consumers never see the integer.
- For `RoomType::DirectMessage`, `Room::counterpart()` is
  pre-populated during `sync_metadata` via
  `POST /v14/list-users`. No extra call needed to display a
  1:1's partner name.
- Filter out `protocol: "proteus"` conversations at the
  `wire::dto` → `Room` boundary. Mixed/MLS pass through.

### TDD starter tests

- `sync_metadata` writes one row per conversation to `rooms`.
- Proteus-only conversations are filtered out.
- `Room::counterpart` returns `Some` for 1:1 rooms, `None`
  for groups and channels.
- `Room::room_type()` mapping for each Wire type integer.
- Repeated `sync_metadata` calls are idempotent (no duplicate
  rows).

### CLI command

```
kupferdraht-cli rooms
kupferdraht-cli sync
```

### Smoke test

Continues from Step 1's smoke.

```bash
kupferdraht-cli sync
kupferdraht-cli rooms
# rooms output must list every established conversation with
# correct type + member count.
```

---

## Step 2g — Mutable account / room metadata (opportunistic)

Originally tracked in PLAN.md's separate "Opportunistic — Steps 1d and 2g" block alongside Step 1d. Hoisted here so the Step-2 file stays the single home for rooms-list + room/account metadata. Opportunistic framing preserved in PLAN.md §Suggested implementation order.

Today Kupferdraht can *read* its own profile and room metadata
(Step 1a's `self`, Step 2's room list) but cannot *write*
them. 2g closes that gap.

#### matrix-rust-sdk analog

`Account::set_display_name(...)`, `Account::set_handle(...)`,
`Room::set_name(...)`, `Room::set_topic(...)`.

#### Public API

```rust
impl Account {
    pub async fn set_display_name(&self, name: String) -> Result<(), KupferdrahtError>;
    pub async fn set_handle(&self, handle: String) -> Result<(), KupferdrahtError>;
}

impl Room {
    pub async fn set_name(&self, name: String) -> Result<(), KupferdrahtError>;
    pub async fn set_topic(&self, topic: String) -> Result<(), KupferdrahtError>;
}
```

`Client::account()` returns the `Account` handle (matrix-rust-sdk
shape).

#### Wire API v14 endpoints

- `PUT /v14/self`        — patches `name` / `handle` on the
  caller's own user record.
- `PUT /v14/conversations/{id}/name` — sets room name.
  Wire emits a `conversation.rename` event to all members;
  receive path updates the local `rooms` row.
- Wire has no first-class `topic` field; if introduced later
  Kupferdraht tunnels it via the conversation `message_timer` /
  `receipt_mode` siblings or punts. **Provisional:** if Wire
  v14 lacks a topic endpoint, ship `set_name` only.

#### CLI commands

```
kupferdraht-cli profile-set --display-name "..." | --handle "..."
kupferdraht-cli room-rename --id <room-id> -n "New name"
```

#### Smoke test

```bash
kupferdraht-cli profile-set --display-name "Alice (Kupferdraht)"
kupferdraht-cli room-rename --id "$ROOM" -n "Renamed via Kupferdraht"
kupferdraht-cli rooms                 # new name appears
kupferdraht-cli self                  # display name updated
```

### CLI commands

**Step 2 — rooms:**

| Command | Options |
|---|---|
| `rooms` | — |
| `sync` | — (one-shot metadata) |
