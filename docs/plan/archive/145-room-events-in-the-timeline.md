# Step 145 — Room events as timeline items

**Status: MERGED 2026-11-09** — `c8539a6` on
`step/145-room-events-in-the-timeline`, merged to `main` as PR #61 (`763cb71`); gate
green on 1.98 (fmt, 0 build warnings, clippy, 1567 tests, deny; wasm32 check, clippy
`--lib` and CI's wasm clippy line); the browser suite green locally (18 suites, 80
tests, headless Firefox) and in CI, without a logged-in client, so the projection
is held by the wasm32 check, a unit test of the names and a pin on the sources.
Smoked 2026-11-09 on the wire-server-rs stack with the CLI built from this tree:
`kupferdraht-smoke.sh --throwaway` 24/24, then a room's life with five accounts:
seven items at the creator in order, the own events among them; a member added later
without the creation; unread 0; a reaction refused; `mark-read` naming its reason; a
join by link starting that member's history; an account deleted; a direct room. The
Kotlin and Swift bindings were generated and read, not compiled: no JDK here, and CI
builds neither. Phase 1 taken and red tests approved by Volker on 2026-11-09.
Commissioned 2026-11-05 (from the browser embedder's
requirements for a work-queue client, R6, measured there against
`4f30f02` and re-checked against `main` `9e1a11c`). Taken with Volker on
commissioning:

- one item per Wire event, with the acting user and the list of
  affected users (Wire's `member-join` names several users at once;
  matrix-rust-sdk has one membership item per user);
- matrix-rust-sdk's shape and names: `TimelineItemKind::MembershipChange`
  carrying a `RoomMembershipChange { change, actor, users }`, `change` a
  `MembershipChange` = `Joined` / `Added` / `Left` / `Kicked`
  (matrix-sdk-ui's enum; `Kicked` also matches Kupferdraht's own `kick`;
  `Added` is Wire's add without invitation, where matrix's nearest is
  `Invited`), `RoomNameChange { name }`, `RoomCreated { name }`; in the
  browser the kinds `"membership_change"` (`change`: `"joined"` /
  `"added"` / `"left"` / `"kicked"`), `"room_name_change"`,
  `"room_created"`;
- the items are not unread; reactions, replies, edits and deletes on
  them are refused; `mark_as_read` on one moves the local read marker
  and sends no confirmation;
- the history starts with the device: Wire has no API to fetch these
  events later, so a new device shows none from before its
  registration, and `conversation.create` reaches only the members of
  the room at its creation;
- Kupferdraht delivers data, not sentences: whether the embedder says
  "Alice added you", "you were added" or "Bob was added" is its choice
  (`actor` stays in the data, `users` names the own user where it is
  one of them).

After Step 144 (no dependency, order of the embedder's list).

## Phase 1, measured 2026-11-09

**Live, on the wire-server-rs stack** with throwaway accounts, as each member's
client sees it on `GET /notifications`:

| Action | Event, and who gets it | `from` | `data` |
|---|---|---|---|
| A creates a room | `conversation.create`: A alone | A | the whole conversation; `name` |
| A adds B, C, D | `member-join`: all four | A | `add_type: internal_add`, `user_ids`, `users[].qualified_id`; no `qualified_user_ids` |
| E joins by link | `member-join`: all, E too | E | as above, `users` = E, and `add_type` also `internal_add` |
| A opens a DM with B | `member-join`: both; no `create` | A | `users` = A and B |
| A renames | `conversation.rename`: all | A | `name` |
| A renames to the same name | another `conversation.rename`, its own `time` | A | `name` |
| A kicks C | `member-leave`: all, C too | A | `qualified_user_ids`, `reason: removed` |
| B leaves | `member-leave`: all, B too | B | `reason: left` |
| D's account is deleted | `member-leave`: all | **D** | `reason: user-deleted` |

The Swagger requires `qualified_from`, `qualified_conversation`, `time`, `via` and
`data` on an `Event`, and `reason` is `left`, `removed` or `user-deleted`.

**The acting device gets its own events,** on `/notifications` and as frames on its
own `/await` socket: all five kinds of action above rang A's socket at once.

**Inbound rows carry this device's clock.** `sent_at_ms` of an inbound message is
`received_at_ms`, `now_ms()` at dispatch (`sync/dispatch/text.rs`), and an outgoing
row's its send time. The server's `time` is on no row today.

**The browser has no timeline diffs** (`docs/bindings.md`): `onSync`'s `roomsUpdated`
is its signal to pull `messages()` again, and every one of these events marks the room
list stale, so a pass carrying one reports `roomsUpdated > 0` where the account has a
room.

**Deviations from the commissioning, all from this:**

1. **No path for own actions.** Scope item 3 falls away: the sync delivers them.
2. **`sent_at_ms` is this device's clock, not the event's `time`.** With the server's
   time, a catch-up drain would put every item before the messages it arrived between,
   and `mark_as_read` on an item with a server clock ahead would set the marker past
   the next message, so that message would never count as unread. The `time` goes into
   the id only. The price: on a device that was offline, an item shows when it was
   synced, as a message does.
3. **`user-deleted` is `Left`, not `Kicked`.** The event names the deleted user as
   `from`, so `Kicked` would read as the user removing themself.
4. **`actor` is not optional.** Phase-1 question 2: no event without `from` exists on
   the stack or in the Swagger; one without is malformed and skipped.
5. **`RoomCreated`'s name is optional:** a conversation's `name` may be null.
6. **The refusal also holds on receive.** An item's id can be computed by anyone who
   saw the event, and ADR-0028 lets a row's sender edit or delete it, so the actor
   could rewrite "Bob removed Carol". An inbound edit, delete or reaction on an item is
   dropped with a `warn!`. A `Confirmation` naming one stays harmless: `read_by` on an
   item nobody shows.
7. **The ids are UUID-shaped,** as every other item id: version 8, the first 16 bytes
   of SHA-256 over the JSON array of a version tag, the type, the room, the actor, the
   `time` and the row's body.
8. **BREAKING (Volker, 2026-11-09):** a new kind breaks an exhaustive `match`, `when`
   or `switch`, so the entry is `BREAKING` under Changed and the release 0.5.0.
   `TimelineItemKind` stays without `non_exhaustive`: the generated Kotlin and Swift
   enums stay exhaustive anyway.

## Problem

1. **The events arrive and are dropped.** Apart from the MLS events,
   typing, `conversation.delete`, feature configs and user properties,
   `dispatch_other_event` (`sync/service.rs`) marks the room list stale
   in its `other =>` arm and drops the event — `conversation.create`,
   `conversation.rename`, `conversation.member-join` and
   `conversation.member-leave` among them. The embedder writes creator
   and date into the room name as a workaround; joins, leaves and
   renames are not shown at all.
2. **Two paths would misread such an item today.** The unread queries
   (`unread_count_for_room`, `unread_counts` in
   `storage/sqlite/read_markers.rs`) count every row of another sender
   after the marker, so "Bob joined" would count as unread. And
   `Timeline::mark_as_read` (`timeline/confirmation.rs`) sends a
   `Confirmation` with the item's id to the room's members — an id that
   only this device knows.

## Scope

1. **Dispatch.** `dispatch_other_event` handles the four types: each
   becomes one timeline item, and the room list is still marked stale
   (Step 118). From the event: `qualified_conversation`, `qualified_from`
   (the actor), `time` (the server's, into the id only; deviation 2) and `data` —
   `member-join`'s users, `member-leave`'s users and `reason` (`left`,
   `removed`, `user-deleted`), `rename`'s and `create`'s `name`. The
   exact v14 shapes are phase 1's. `change`: a join by its only user is
   `Joined` (by link or code), else `Added`; a leave with `reason` `left`
   is `Left`, `removed` and `user-deleted` are `Kicked`. A malformed
   event is a `warn!` and is skipped, never a failed sync.
2. **The item.**
   - Its id is derived from the event's content — type, conversation,
     actor, server time and data, hashed — not from the notification:
     the same event arriving twice (redelivered, or once as the answer
     to an own call and once through sync) yields the same id, and the
     `(message_id, room_id)` key keeps one row.
   - `sender_user_id` the actor, `sender_device_id` empty (a server
     event has no device), `kind` with a JSON `body` like `Location`
     (Step 26); no migration. An event for a room not stored yet is
     written all the same (`timeline_events` has no foreign key since
     V9) and shows once the room does.
3. **Own actions.** Measured: the acting device receives its own events
   through sync, so they take the same path, at the next drain. Nothing to
   add to `create_room`, `invite_members`, `kick_members`, `leave_room` or
   `rename_room`.
4. **What the items are not.** The unread queries filter the three
   kinds. `mark_as_read` on one moves the local marker to its position
   and sends no `Confirmation`, as `ReceiptType::ReadPrivate` does
   (Step 138). Reactions, replies, edits and deletes that target one are
   `InvalidInput`; every other path that treats an item as a message
   was checked on the step: ephemeral timers read `expires_at_ms`, which an
   item has not; a delivery confirmation and `on_message` come from
   `surface_inbound_message`, which an item does not pass; the room list
   reads no last item.
5. **Public surface, in the same commit.** `TimelineItemKind::
   {MembershipChange, RoomNameChange, RoomCreated}`, the first carrying
   `RoomMembershipChange` with the enum `MembershipChange::{Joined,
   Added, Left, Kicked}` (matrix-sdk-ui's names); the FFI mirror
   (`ffi/timeline.rs`); the wasm projection in `messages()` (`kind`,
   `change`, `actor`, `users`, `name`); the item types in
   `kupferdraht-tabs/index.d.ts`. Live diffs carry the new items through
   the same Append path as messages, natively; the browser has no diffs and
   pulls `messages()` on `onSync` (phase 1).
6. **CLI.** `messages` prints a system line per item (`* alice@d added
   bob@d`, `* alice@d renamed the room to "…"`).
7. **Docs:** `docs/wire/api-notes.md` and `docs/backend-spec.md` (the
   four events as observed), `docs/library.md`, `docs/bindings.md` (the
   kinds, the unread and read rules, "history starts with the device",
   the actor as the backend's statement), `docs/cli.md`,
   `docs/api-mapping-matrix-rust-sdk.md` (`MembershipChange` /
   `RoomMembershipChange` Aligned with `TimelineItemContent::
   MembershipChange(RoomMembershipChange)` — one item per Wire event
   with a user list where matrix has one per user, `Added` beside
   matrix's variants; `RoomNameChange` and
   `RoomCreated` as Drift to `OtherState` with `RoomName` /
   `RoomCreate`), `docs/THREAT_MODEL.md` (one line under the backend's
   trust: the actor is `qualified_from`, not authenticated by MLS — a
   hostile backend can forge "Alice removed Bob" as it can any room
   metadata), `README.md`, `CHANGELOG.md` `[Unreleased]`: Added, and `BREAKING`
   under Changed for the new kinds.

## Out of scope

- The other room events (`member-update` roles, `receipt-mode-update`,
  `message-timer-update`, `access-update`, `code-update`): the same
  pattern, a later step if an embedder needs them.
- Backfill of earlier events (Wire has no such API); items derived from
  MLS commits (the membership the commits change stays the MLS path's,
  these items are display only).
- Sentences: Kupferdraht renders no text.

## Phase-1 questions

1. **Live shapes on the wire-server-rs stack:** the v14 JSON of the four
   events as A and B see them — `users` versus `qualified_user_ids`, the
   `reason` values, `create`'s `data` — and whether the acting device
   receives its own event through `/notifications` and the socket.
   Recorded in `docs/wire/api-notes.md`.
2. **An absent actor:** the events where Wire names no `from` (a
   deleted user's leave, a removal by the backend); `actor` is
   optional for them.

## TDD sketch

- httpmock / injected events: each of the four types → one item with
  actor, users or name; the four `change` values; an item standing where it
  arrived whatever the server's clock says; a
  malformed event → no item, the sync goes on; the same event twice →
  one row; an event for a room not stored yet → stored and listed once
  the room is.
- Rules: an item of the new kinds after the marker is not unread (both
  queries); `mark_as_read` on it moves the marker and sends no
  `Confirmation` (the mock sees no message); a reaction, reply, edit or
  delete on it → `InvalidInput`.
- Own action (per phase 1): a rename through `Room::set_name` yields one
  item, also when the event comes back through sync.
- FFI, wasm and tabs: the kinds in the parity tests and in the
  projection of `messages()`; wasm32 `check` and `clippy`.
- Smoke: `kupferdraht-smoke.sh --throwaway` 18/18, then on the stack: A
  creates a room, adds B, renames it, B leaves; `messages` at A and at B
  shows the items with actor, users and time.
