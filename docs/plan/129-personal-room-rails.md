# Step 129 — Personal-room rails: read state across devices, the hide marker

**Status: commissioned 2026-11-01** (transferred from open.md's former
"Proposed steps", proposed 2026-09-24 in the Step 86 retrospective; open.md
§64 ranked the personal-room follow-ups first). Draft sync, the third
proposal of that retrospective, stays a §64 candidate. **Extended 2026-11-05**
by the browser embedder's requirement R7 (read position across devices,
measured there against `4f30f02`) and split into sub-steps (Volker):

- **129a — the read position across devices**: Wire's `LastRead` through
  the personal room, behind the browser surface of Step 138 unchanged. It
  reports through Step 146's `changed_rooms` / `changedRooms`. A received
  `LastRead` clears the device-local unread flag like a local read does.
- **129b — the hide marker**: Step 89's hide-on-own-devices marker
  (`MessageHide`) through the personal room, with ADR-0014's revision
  note on the personal task list. After 129a, no fixed slot.
- **129c — the unread flag across Kupferdraht devices**: parked
  2026-11-25, the design kept below for the day a consumer needs it.

Taken with Volker on 2026-11-05: the unread flag rides along as a
Kupferdraht extension of `GenericMessage` in the personal room (field
262003, a new ADR beside ADR-0014 / 0018 / 0020); read position and flag
are ordered by the server time of the personal-room message that carries
them, not by device clocks. **Revised 2026-11-25** (Volker): the flag stays
device-local for now — no consumer needs it across devices, R7 asked for
the read position — and its sync is 129c, parked. 129a transcribes Wire's
`LastRead` and nothing of Kupferdraht's own, so it needs no ADR.

**Refreshed 2026-11-25** (Volker), before the start, against `main`
b02f7ea:

- The names follow Step 163c (ADR-0009 revision 1.2): the room is the
  **personal room** — `Client::personal_room()`, `RoomType::Personal`,
  "personal room" in every public surface and doc. `LastRead`,
  `MessageHide` and Wire's `self_room` type stay Wire's words, in `wire/`
  and in the proto only.
- ADR-0044 of the 2026-11-05 text went to Step 156's own certificate
  authorities; 129a needs no ADR, 129c takes the next free number when it
  starts. The next free migration is V39 today; 129a expects none.
- Steps 146, 147 and 152 are merged (2026-11-09/10): nothing blocks 129a.
- Phase 1 opens with readings: Wire's `LastRead` and `MessageHide` are in
  neither `docs/wire/api-notes.md` nor `docs/wire/kalium-readings.md` yet;
  `message-types.md` lists both as open.
- Scope 4 lost its fallback of a narrow read-state hook: the embedder
  reads on request (Volker, 2026-11-09, Steps 146 and 152).
- 129b shrank: the personal task list already rides the personal room over
  the generic task path (Step 86's two-device smoke, observations §82), so
  129b is the hide marker plus ADR-0014's revision note.
- The flag sync left 129a for 129c (parked), see above.

## Problem

Step 86 (2026-09-24) opened the personal room — Wire's self conversation:
the first device establishes it at epoch 0, later devices external-join,
`Client::personal_room()` answers it. Three things want that channel; two
are not built:

1. **Cross-device read position.** `mark_as_read` (`timeline/confirmation.rs`,
   with its FFI and wasm mirrors) moves the read marker on this device only.
   Since Step 138 the browser surface reports read state — `rooms()`'
   `unreadCount` / `lastReadAtMs` / `markedUnread`, `onRoomsChanged` — and
   all of it is device-local: a user who reads in one browser finds the
   room unread in the other. Wire carries the position between a user's
   devices as a `LastRead` message in its self conversation, Kupferdraht's
   personal room (`docs/wire/message-types.md`, field 6, listed as open).
   `onRoomsChanged` fires only for a local change today (`report_read_state`
   in `wasm/manager.rs`); a change that sync applies reaches the embedder
   through Step 146's `changed_rooms` / `changedRooms` and a `rooms()` read.
2. **The hand-set unread flag** (`Room::set_unread_flag`, browser
   `markAsUnread`, Step 138, `room_read_markers.marked_unread`, V31) is
   device-local too, and stays so (129c, parked). Wire's `GenericMessage`
   has no message for it; matrix-rust-sdk syncs the same flag as room
   account data (`m.marked_unread`), which Wire does not have. What 129a
   settles is the one interaction: a read on another device clears it, as
   a local read does (Step 138), so no device shows a room as needing
   attention after the user read it elsewhere.
3. **The hide marker.** ADR-0014 names the self conversation as the
   vehicle of the personal task list; since Step 86 the personal room
   carries personal tasks over the generic task path (observations §82: a
   task sent by device A in the personal room, read by device B), while
   ADR-0014's text still describes the state before Step 86. What the
   channel still lacks is the Step-89 marker: the receiver-side ephemeral
   expiry sends `MessageDelete` into the room and leaves the hide-on-own-
   devices marker for the user's other devices unsent, because it needs
   this channel (`docs/plan/archive/89-ephemeral-messages.md`;
   `docs/wire/message-types.md` lists it as `MessageHide`, field 12, "no
   step planned").

None of these messages is in `kupferdraht-core/proto/generic_message.proto`
yet; the proto is Kupferdraht's scope-trimmed copy and grows by what a step
implements.

## 129a — the read position across devices

### Scope

1. **Phase 1: the readings and the rule.**
   - The readings: Wire's `LastRead` (field 6 — the conversation, its
     qualified id, the read timestamp; the fields confirmed against Wire's
     schema) and how Wire's own apps send it into the self conversation
     and apply one they receive; `MessageHide` (field 12) for 129b. Both
     recorded in `docs/wire/kalium-readings.md` (the source index) and in
     `docs/wire/api-notes.md` / `message-types.md`, before the rule below
     is fixed.
   - The position: how Wire's `last_read_timestamp` maps onto
     Kupferdraht's marker, the `arrived_at_ms` position of Step 151, in
     both directions.
   - The fold rule: a `LastRead` moves the marker forward only (the newer
     read position wins), one for a room this device does not know is
     ignored, not queued; a `LastRead` that moves the marker clears the
     device-local unread flag, as a local read does (Step 138); one that
     does not move it leaves the flag alone.
   - No ADR: 129a transcribes Wire's message and keeps Step 138's rule.
2. **Sending.** `mark_as_read` on any room other than the personal room
   also sends a `LastRead` into the personal room, for
   `ReceiptType::Read` and `ReadPrivate` alike — it reaches only the
   user's own devices, as matrix's `m.read.private` does. On an item of
   Step 145's room-event kinds it carries that item's time like any
   other. Best-effort as the Step-25b receipts: a rejected upload is
   logged, the local state stands.
3. **Receiving.** An inbound `LastRead` from one of the user's own
   devices — Kupferdraht's or Wire's own apps on the same account —
   moves that room's marker under the fold rule and clears the flag.
   Storage: `room_read_markers` as it is; a migration only if phase 1's
   position mapping needs a column (the next free number when the step
   starts).
4. **Reporting.** A change sync applies notes its room in Step 146's set,
   so `changed_rooms` / `changedRooms` names it, and `rooms()` shows the
   new `unreadCount` / `lastReadAtMs` / `markedUnread`. `onRoomsChanged`
   stays the report of a local change. No further signal: the embedder
   reads on request (Volker, 2026-11-09), so R7 is served by
   `changedRooms` plus `rooms()`.
5. **A device not in the personal room.** The Step-86 join inheritance of
   `send_text_auto_join` applies (join, then send); a device that cannot
   join logs it and keeps the local effect. Phase 1 confirms that a
   browser device joins the personal room (Step 86 was smoked natively
   only).
6. **No new public call.** The FFI, wasm and tabs surfaces keep their
   shape; their docs say the position now follows the account across
   devices and the flag stays device-local, cleared by a read on any
   device. The CLI's `mark-read` exercises it.
7. **Docs:** `docs/wire/message-types.md` (`LastRead` implemented),
   `docs/wire/kalium-readings.md` and `docs/wire/api-notes.md` (the
   readings), `docs/library.md` and `docs/bindings.md` (the position
   across devices, Wire's own apps included; the flag device-local),
   `docs/api-mapping-matrix-rust-sdk.md` (`read_marker` now across
   devices; `set_unread_flag` / `is_marked_unread` stay Aligned,
   device-local), `docs/ARCHITECTURE.md` (the personal-room traffic),
   `CHANGELOG.md`, `README.md`.

### Out of scope

- The flag across devices (129c, parked); draft sync (open.md §64);
  mute / archive state (§64 — Wire carries it per conversation, not over
  the personal room).
- The personal-room roster (open.md's former gap-B.32): establishment adds
  every key package Wire hands out for the user's other registered
  devices, dead devices of unreadable homes included (`claim … skip_own`),
  so the roster carries leaves nobody operates until Wire's
  device-deletion Remove proposals clear them (observations §69 / §70).
  Harmless for confidentiality; a stale-snapshot establish race is handled
  by discard + retry. `LastRead` rides the roster as it is.

### TDD sketch

Where httpmock cannot stand in for openmls (open.md gap-B.11), the Step-86
tests' real second device (`common::mls::PeerDevice`) does:

- a proto round-trip for `LastRead` (incl. the qualified conversation id)
  — red;
- `mark_as_read` with `Read` and with `ReadPrivate` on a group sends
  exactly one `LastRead` into the personal room, none when the room is
  the personal room, and the local marker moves even when the
  personal-room upload is rejected — red;
- a `LastRead` from the peer device moves the marker forward, an older
  one does not, one for an unknown room is dropped; one that moves the
  marker clears a flag set on this device, one that does not leaves it
  — red;
- a sync-applied change is reported: `changed_rooms` names the room
  (native), `rooms()` shows the new state (a browser test, CI) — red;
- `migration_immutability`, and the migration over the previous schema if
  phase 1 adds one.

Smoke: two devices of one smoke account, on wire-ref (SERVER_4, a valid
client flow) or on the wire-server-rs stack (galley-rs creates the self
conversation server-side), with the Step 86 fixtures: read a room on A —
B's `unreadCount` is 0 after its next sync; mark it unread on B, read it
on A — B's flag is gone. wire-ref caps an account at seven devices and
rate-limits logins (observations §82, side findings): the second device is
a second smoke home, registered once.

## 129b — the hide marker

### Scope

1. Phase 1: a revision entry of ADR-0014 — the personal list's vehicle is
   the personal room as it works since Step 86 (observations §82); no
   migration of rows, a task stays in the room it was created in. The
   CLI's and the docs' examples of a personal task name the personal room.
2. The Step-89 hide marker (`MessageHide`, field 12) for the user's own
   other devices is sent into the personal room after a receiver-side
   expiry, and an inbound one tombstones the local row.
3. The join inheritance of 129a's scope 5 applies.

### TDD sketch

- a proto round-trip for `MessageHide` — red;
- a receiver-side expiry of Step 89 is followed by a `MessageHide` in the
  personal room; an inbound one tombstones the row — red;
- the personal task list needs no new test: Step 86's two-device test
  already sends a task into the personal room and reads it on the peer.

Smoke: let an ephemeral message expire on A and it is hidden on B; a
personal task created on A is listed by B from the personal room (as in
Step 86).

## 129c — the unread flag across Kupferdraht devices (parked)

Parked 2026-11-25 (Volker): no consumer needs the hand-set flag across
devices today, and Wire has no message for it, so Kupferdraht would be
alone on this rail. The design of 2026-11-05, kept for the day one does:

- **The extension:** field 262003, `UnreadFlagPayload { qualified
  conversation id, marked }`, sent into the personal room only — outside
  the oneof like the other three extensions; Wire's own apps skip the
  field and discard the message (ADR-0014's interop facts), and they do
  not show the self conversation. An ADR beside ADR-0014 / 0018 / 0020,
  with matrix-rust-sdk's analog (`Room::set_unread_flag` over
  `m.marked_unread`) and why Wire needs a message instead.
- **The order:** flag and read position are ordered by the server time of
  the personal-room message carrying them, not by device clocks — a later
  `LastRead` clears the flag, a later flag message sets or clears it. A
  local change is stamped with the `time` of Wire's answer to the send
  (own messages do not come back through sync); until that answer, the
  local clock stands in.
- **Storage:** `room_read_markers` gains the two server times the rule
  compares (the flag's last change, the last applied read), one migration.
- **Sending and receiving:** `set_unread_flag` sends the flag message,
  best-effort; an inbound one sets or clears the flag under the order
  above. 129a's scope 5 (join inheritance) and scope 4 (reporting through
  `changed_rooms`) apply unchanged.
- **Tests and smoke:** a proto round-trip for `UnreadFlagPayload`;
  `set_unread_flag` sends one flag message; a flag message from the peer
  device sets and clears under the order, and a `LastRead` clears a flag
  set before it and not one set after it. Smoke: mark a room unread on A —
  B shows the flag; read it on B — A's flag is gone.
