# Step 129 — Self-room rails: `LastRead` and the personal task list

**Status: commissioned 2026-11-01** (transferred from open.md's former
"Proposed steps", proposed 2026-09-24 in the Step 86 retrospective; open.md
§64 ranked the self-room follow-ups first). Draft sync, the third proposal
of that retrospective, stays a §64 candidate.

## Problem

Step 86 (2026-09-24) opened the self conversation: the first device
establishes it at epoch 0, later devices external-join, `Client::self_room()`
answers it. Three things wanted that channel; two of them are decided and
still unbuilt:

1. **Cross-device read position.** `mark_as_read` (`timeline/confirmation.rs`,
   with its FFI and wasm mirrors) moves the read marker on this device only.
   Wire carries the position between a user's devices as a `LastRead`
   message in the self conversation (`docs/wire/message-types.md`, field 6,
   listed as open). A user who reads on the phone keeps unread rooms on the
   desktop.
2. **The personal task list.** ADR-0014 names the self conversation as the
   vehicle of the personal list; today the list lives in a one-member group
   as a workaround from before Step 86. The Step-89 receiver-side ephemeral
   expiry sends `MessageDelete` into the room and leaves the hide-on-own-
   devices marker for the user's other devices unsent, because it needs the
   same channel (`docs/plan/archive/89-ephemeral-messages.md`;
   `docs/wire/message-types.md` lists it as `MessageHide`, field 12, "no
   step planned").

Neither message is in `kupferdraht-core/proto/generic_message.proto` yet;
the proto is Kupferdraht's scope-trimmed copy and grows by what a step
implements.

## Scope

1. Phase 1: a revision entry of ADR-0014 (the vehicle moves from the
   workaround group to the self conversation; what happens to an existing
   workaround group — migrate its rows once, or leave it as an ordinary
   group), and the fold rule for `LastRead` (a marker only moves forward,
   the newer timestamp wins, a `LastRead` for an unknown room is ignored,
   not queued).
2. `LastRead`: `mark_as_read` on any room other than the self room also
   sends a `LastRead` into the self conversation, best-effort as the
   Step-25b receipts (a rejected upload is logged, the local marker stands).
   An inbound `LastRead` from the user's own other device moves that room's
   read marker (`set_read_marker`) and the timeline reports it the way a
   local `mark_as_read` does. Proto message, encode / decode, the
   message-types row.
3. The personal task list: the ADR-0014 task operations addressed at "my
   list" go into the self conversation; the CLI's and the FFI's own-list
   entry points follow. The Step-89 hide marker for own other devices is
   sent into the self conversation after a receiver-side expiry, and an
   inbound one tombstones the local row.
4. A device that has not joined the self room yet: the Step-86 join
   inheritance of `send_text_auto_join` applies (join, then send); a device
   that cannot join logs it and keeps the local effect.

## Out of scope

- Draft sync (open.md §64).
- Mute / archive state (§64); Wire carries it per conversation, not over
  the self room.
- Any notion of an unread count; the SDK moves the marker and reports it,
  the consumer counts.

## TDD sketch

Where httpmock cannot stand in for openmls (open.md gap-B.11), the Step-86
tests' real second device (`common::mls::PeerDevice`) does:

- proto round-trips for `LastRead` and `MessageHide`, incl. the qualified
  conversation id — red;
- `mark_as_read` on a group sends exactly one `LastRead` into the self
  room, none when the room is the self room, and the local marker moves
  even when the self-room upload is rejected — red;
- a `LastRead` received from the peer device moves the marker forward, an
  older one does not, one for an unknown room is dropped — red;
- a task created for the own list lands in the self conversation and the
  workaround group gets no new rows — red;
- a receiver-side expiry of Step 89 is followed by a `MessageHide` in the
  self room; an inbound one tombstones the row — red.

Smoke (two devices on the test stage, the Step 86 fixtures): read a room on
device A and device B's marker moves; create a personal task on A and B
lists it from the self room.
