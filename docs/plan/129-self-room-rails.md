# Step 129 — Self-room rails: read state across devices, the personal task list

**Status: commissioned 2026-11-01** (transferred from open.md's former
"Proposed steps", proposed 2026-09-24 in the Step 86 retrospective; open.md
§64 ranked the self-room follow-ups first). Draft sync, the third proposal
of that retrospective, stays a §64 candidate. **Extended 2026-11-05** by
the browser embedder's requirement R7 (read position across devices,
measured there against `4f30f02`) and split into two sub-steps (Volker):

- **129a — read state across devices**: `LastRead` and the hand-set
  unread flag through the self conversation, behind the browser surface
  of Step 138 unchanged. After Step 147; it uses Step 146's room-change
  hook.
- **129b — the personal task list** in the self conversation and the
  Step-89 hide marker. After 129a, no fixed slot.

Taken with Volker on 2026-11-05: the unread flag rides along as a
Kupferdraht extension of `GenericMessage` in the self conversation
(field 262003, a new ADR beside ADR-0014 / 0018 / 0020); read position
and flag are ordered by the server time of the self-room message that
carries them, not by device clocks.

## Problem

Step 86 (2026-09-24) opened the self conversation: the first device
establishes it at epoch 0, later devices external-join, `Client::self_room()`
answers it. Three things want that channel; none is built:

1. **Cross-device read position.** `mark_as_read` (`timeline/confirmation.rs`,
   with its FFI and wasm mirrors) moves the read marker on this device only.
   Since Step 138 the browser surface reports read state — `rooms()`'
   `unreadCount` / `lastReadAtMs` / `markedUnread`, `onRoomsChanged` — and
   all of it is device-local: a user who reads in one browser finds the
   room unread in the other. Wire carries the position between a user's
   devices as a `LastRead` message in the self conversation
   (`docs/wire/message-types.md`, field 6, listed as open). `onRoomsChanged`
   fires only for a local change today (`report_read_state` in
   `wasm/manager.rs`), so a change that sync applies would reach nobody.
2. **The hand-set unread flag** (`Room::set_unread_flag`, browser
   `markAsUnread`, Step 138, `room_read_markers.marked_unread`, V31) is
   device-local too. Wire's `GenericMessage` has no message for it;
   matrix-rust-sdk syncs the same flag as room account data
   (`m.marked_unread`), which Wire does not have.
3. **The personal task list.** ADR-0014 names the self conversation as the
   vehicle of the personal list; today the list lives in a one-member group
   as a workaround from before Step 86. The Step-89 receiver-side ephemeral
   expiry sends `MessageDelete` into the room and leaves the hide-on-own-
   devices marker for the user's other devices unsent, because it needs the
   same channel (`docs/plan/archive/89-ephemeral-messages.md`;
   `docs/wire/message-types.md` lists it as `MessageHide`, field 12, "no
   step planned").

None of these messages is in `kupferdraht-core/proto/generic_message.proto`
yet; the proto is Kupferdraht's scope-trimmed copy and grows by what a step
implements.

## 129a — read state across devices

### Scope

1. **Phase 1: ADR-0044** (the unread flag through the self conversation):
   - the extension: field 262003, `UnreadFlagPayload { qualified
     conversation id, marked }`, sent into the self conversation only —
     outside the oneof like the other three extensions; Wire's own apps
     skip the field and discard the message (ADR-0014's interop facts),
     and they do not show the self conversation;
   - the fold rule for read state: a `LastRead` moves the marker forward
     only (the newer read position wins), one for a room this device
     does not know is ignored, not queued; flag and read are ordered by
     the server time of the self-room message carrying them — a later
     `LastRead` clears the flag, a later flag message sets or clears it;
     how a local change is stamped until its own message's server time
     is known;
   - matrix-rust-sdk's analog (`Room::set_unread_flag` over
     `m.marked_unread`) and why Wire needs a message instead.
2. **Sending.** `mark_as_read` on any room other than the self room
   also sends a `LastRead` into the self conversation, for
   `ReceiptType::Read` and `ReadPrivate` alike — it reaches only the
   user's own devices, as matrix's `m.read.private` does. On an item of
   Step 145's room-event kinds it carries that item's time like any
   other. `set_unread_flag` sends the flag message. Both best-effort as
   the Step-25b receipts: a rejected upload is logged, the local state
   stands.
3. **Receiving.** An inbound `LastRead` from one of the user's own
   devices — Kupferdraht's or Wire's own apps on the same account —
   moves that room's marker under the fold rule and clears an older
   flag; an inbound flag message sets or clears the flag under the same
   rule. Storage: `room_read_markers` gains the server times the rule
   compares (the flag's last change and the last applied read), one
   migration (the next free number when the step starts).
4. **Reporting.** A change sync applies reports like a local one: in the
   browser `onRoomsChanged` fires and `rooms()` shows the new
   `unreadCount` / `lastReadAtMs` / `markedUnread` — the embedder changes
   nothing — through Step 146's core-level room-change hook, which the
   sync fires here as the own room changes do there; natively the same
   through the path phase 1 picks (an existing listener or a room-list
   update).
5. **A device not in the self room.** The Step-86 join inheritance of
   `send_text_auto_join` applies (join, then send); a device that cannot
   join logs it and keeps the local effect. Phase 1 confirms that a
   browser device joins the self conversation (Step 86 was smoked
   natively only).
6. **No new public call.** The FFI, wasm and tabs surfaces keep their
   shape; their docs say the state now follows the account across
   devices. The CLI's `mark-read` / `mark-unread` exercise it.
7. **Docs:** `docs/wire/message-types.md` (`LastRead` implemented, the
   262003 row), ADR-0044 + `rules/0044-…`, `docs/library.md` and
   `docs/bindings.md` (across devices, Wire's own apps for `LastRead`,
   not for the flag), `docs/api-mapping-matrix-rust-sdk.md`
   (`set_unread_flag` / `is_marked_unread` stay Aligned, now also in
   effect), `docs/ARCHITECTURE.md` (§7 for the columns, the self-room
   traffic), `CHANGELOG.md`, `README.md`.

### Out of scope

- Draft sync (open.md §64); mute / archive state (§64 — Wire carries it
  per conversation, not over the self room).
- Showing or setting the flag in Wire's own apps (they have no message
  for it).
- The self-room roster (open.md's former gap-B.32): establishment adds
  every key package Wire hands out for the user's other registered
  devices, dead devices of unreadable homes included (`claim … skip_own`),
  so the roster carries leaves nobody operates until Wire's
  device-deletion Remove proposals clear them (observations §69 / §70).
  Harmless for confidentiality; a stale-snapshot establish race is handled
  by discard + retry. `LastRead` and the flag ride the roster as it is.

### TDD sketch

Where httpmock cannot stand in for openmls (open.md gap-B.11), the Step-86
tests' real second device (`common::mls::PeerDevice`) does:

- proto round-trips for `LastRead` (incl. the qualified conversation id)
  and `UnreadFlagPayload` — red;
- `mark_as_read` with `Read` and with `ReadPrivate` on a group sends
  exactly one `LastRead` into the self room, none when the room is the
  self room, and the local marker moves even when the self-room upload
  is rejected; `set_unread_flag` sends one flag message — red;
- a `LastRead` from the peer device moves the marker forward, an older
  one does not, one for an unknown room is dropped; it clears a flag set
  before it and not one set after it; a flag message sets and clears
  under the same order — red;
- a sync-applied change is reported: the core report fires (native),
  `onRoomsChanged` fires and `rooms()` shows the new state (a browser
  test, CI) — red;
- the migration over the previous schema; `migration_immutability`.

Smoke (two devices of one account on the wire-server-rs stack, the Step 86
fixtures): read a room on A — B's `unreadCount` is 0 after its next sync;
mark it unread on A — B shows the flag; read it on B — A's flag is gone.

## 129b — the personal task list and the hide marker

### Scope

1. Phase 1: a revision entry of ADR-0014 — the vehicle moves from the
   workaround group to the self conversation; what happens to an existing
   workaround group (migrate its rows once, or leave it as an ordinary
   group).
2. The personal task list: the ADR-0014 task operations addressed at "my
   list" go into the self conversation; the CLI's and the FFI's own-list
   entry points follow.
3. The Step-89 hide marker (`MessageHide`, field 12) for own other devices
   is sent into the self conversation after a receiver-side expiry, and an
   inbound one tombstones the local row.
4. The join inheritance of 129a's scope 5 applies.

### TDD sketch

- a proto round-trip for `MessageHide` — red;
- a task created for the own list lands in the self conversation and the
  workaround group gets no new rows — red;
- a receiver-side expiry of Step 89 is followed by a `MessageHide` in the
  self room; an inbound one tombstones the row — red.

Smoke: create a personal task on A and B lists it from the self room; let
an ephemeral message expire on A and it is hidden on B.
