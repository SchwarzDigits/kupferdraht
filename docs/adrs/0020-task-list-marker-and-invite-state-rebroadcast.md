# ADR-0020: Mark task-list rooms and re-broadcast full state on invite

**Status:** Accepted
**Date:** 2026-08-24
**Deciders:** Volker Zöpfel

## Context

ADR-0014 made tasks pure event-sourced payloads (field 262000,
membership-is-ACL) and knows "dedicated task-list conversations" only
as a *send mode*: `send_task(draft, with_fallback: false)` omits the
Text fallback so official clients render nothing. Nothing marks the
room itself. Two consumer-visible gaps follow (surfaced by kupferdraht-tui
Step 14, open.md §67): a client cannot recognize a conversation as a
task list, so navigation cannot group them; and every sender must
independently remember to suppress the fallback per message.

MLS has no history, so a member added after any room-level state was
established never sees it. ADR-0018's invite hook re-sends the case
snapshot after the Add-commit merges. ADR-0014 decision 6 is that
same pattern for tasks: a snapshot is a batch of ordinary updates,
fired on that hook. Case rooms therefore have both a *marker*
("this room is a case" — `Room::case()` is `Some`) and a
late-join story. Task lists have the snapshot mechanism but no
room-level marker, so nothing triggers the batch and a late joiner
sees an empty board. This ADR adds that marker and the union rule
on the shared hook; it does not replace 0014#6 or 0018#2.

ADR-0018 decision 3's admin-gated fold has a named residual
(observations §72): `invite_members` re-broadcasts the case
snapshot unconditionally, but the receiving fold discards a
**non-admin** sender's snapshot when no folded case exists yet —
by design, as the anti-forgery gate. Wire lets any member invite,
so a non-admin's invite satisfies the 0018 MUST mechanically while
the new member still ends up without case state. Any admin-gated
room-level state inherits this hole; any-member-gated state does
not. This ADR does not reopen it.

Constraints from prior decisions: all Wire content is `GenericMessage`
Protobuf (CLAUDE.md); bare payloads outside the `oneof` are invisible
to official clients (ADR-0014); the public API is
matrix-rust-sdk-shaped (ADR-0009); tasks fold last-writer-wins in
event order (ADR-0014); case cores fold LWW by revision with
receive-time admin capture (ADR-0018).

## Options Considered

### Option A: No marker — keep task lists a send convention

Status quo. Task-only rooms stay recognizable only to humans.

**Pros:**
- Zero new protocol surface; nothing to bootstrap.

**Cons:**
- Consumers cannot group task lists (the concrete TUI ask goes
  unanswered).
- Every sender keeps deciding `with_fallback` per message; one wrong
  call spams official-client views.
- Late joiners see nothing and nothing can fix it, since there is no
  anchor to hang a re-broadcast on.

### Option B: Navigation-only marker

A minimal room-level marker payload, re-broadcast on invite, used
purely for grouping. No task-state re-broadcast.

**Pros:**
- One message per room, one per invite; trivially cheap.
- Solves the grouping ask.

**Cons:**
- A late joiner recognizes the room as a task list — and sees an
  **empty** one. The marker advertises state the joiner cannot get,
  which is arguably worse than not knowing.
- Leaves task lists second-class next to case rooms, whose invite
  hook already re-sends the full case snapshot.

### Option C: Marker + full late-join re-broadcast, symmetric with case rooms *(chosen)*

Introduce the marker as the anchor task lists were missing, then
treat both room types identically in the invite hook: the inviter
re-sends the union of whatever anchors the room currently has
(case snapshot and/or marker) plus every task's current folded
state, once.

**Pros:**
- Late joiners get the full board in both room types — the same
  mental model everywhere ("the inviter hands the newcomer the
  room's state").
- The marker is any-member (task-family precedent, see Decision), so
  the task-list re-broadcast lands regardless of who invites —
  task lists are end-to-end robust where the case branch still has
  the §72 admin-gate hole.
- Folded task state is idempotent under a re-send of the current
  board (ADR-0014 event-order LWW) — `Room::tasks()` does not
  duplicate.

**Cons:**
- Invite cost grows by one message per task (O(N) sends per invite
  into a populated room), and every existing member receives those
  messages as real timeline rows and diffs, not silence.
- A room-level flag any member can flip is forgeable UI state.

### Option D: Bundled board snapshot (one message, N tasks)

A new `TaskBoardPayload` field carrying every folded task in a
single `GenericMessage`, sent once per invite.

**Pros:**
- O(1) invite traffic regardless of board size; one timeline row
  for existing members.

**Cons:**
- Burns a fourth custom field and a second fold path for the same
  state ADR-0014 already reconstructs from ordinary updates.
- Diverges from ADR-0014 decision 6's stated snapshot mechanism
  ("a snapshot is just a batch of ordinary updates").
- Rejected: reuse the existing send/fold path; if O(N) hurts in
  production, a cap or a bundled payload is a new decision.

## Decision

Option C, with the five sub-decisions from the design discussion
(Volker, 2026-08-24):

1. **The marker is any-member.** `TaskListMarker` (field **262002**,
   bare payload outside the `oneof` — official clients see nothing)
   carries the full marker state (`marked: bool`) and folds
   last-writer-wins in event order, exactly like tasks (ADR-0014).
   Any member sets or clears it; no role capture, no revision field.
   Rationale: membership-is-ACL is the task family's model, and an
   admin gate would re-open the §72 hole for exactly the MUST this
   ADR exists to serve.
2. **Late-join re-broadcast is a MUST, as the union of both
   anchors.** In `Client::invite_members`, after the Add-commit
   merges (ADR-0018 order), the inviter re-sends whatever the room
   currently has, with the task helper run **once**:
   - `Room::case()` is `Some` → the case snapshot (existing 72c
     MUST);
   - `Room::is_task_list()` → the marker;
   - either anchor (or both) → every task's current folded state,
     once.
   A room that is both a case and a marked task list therefore
   sends case snapshot + marker + N tasks, not 2N tasks.
   Unmarked 1:1s / group chats that merely contain a delegated
   task are not an anchor — they keep the ADR-0014 empty-on-join
   behaviour. Best-effort with a warning on failure, like 72c.
3. **All tasks are re-broadcast, not only open ones.** Done and
   cancelled tasks are part of the board a newcomer must see
   (audit-trail reading of a case); the extra cost is bounded by
   the room's task count.
4. **All three task send paths derive their fallback default from
   the marker.** `send_task`, `update_task`, and `move_task` share
   one send tail; in a marked room the fallback defaults to *off*,
   unmarked rooms keep the current default, and an explicit caller
   choice always wins. The FFI mirror carries the same
   `Option<bool>` contract — a `bool` mirror would keep the footgun
   for Kotlin/Swift. The per-message footgun ("forgot
   `with_fallback: false` in a task room") disappears for every
   task mutation, not only creation.
5. **The §72 case-side hole stays an ADR-0018 residual**, not
   reopened here: a first-snapshot exception in the case fold would
   disable the anti-forgery gate for exactly the members who cannot
   verify the state. The case branch of the invite hook therefore
   only fully lands when the inviter is an admin; tasks and marker
   land from any inviter. Heals on the next admin core update.

## Consequences

### Positive

- Consumers can group task lists (`Room::is_task_list()`), the
  original ask.
- Late joiners receive the complete room state in both room types;
  invites stop producing members who see an empty or unmarked room.
- Senders stop deciding the fallback per message in marked rooms.
- Folded task state is idempotent for existing members — no
  duplicate tasks, no reordering (LexoRank rides in the state).
  Timeline rows and listener diffs still fire; see Negative.

### Negative

- **Any member can flip the marker** — forgeable, room-visible UI
  state. Accepted: the marker structures navigation, it grants no
  authority (unlike the case core, which stays admin-gated).
- **Invite cost is O(task count) in traffic and in the timeline**
  — one MLS application message per task per invite. A 50-task
  room costs 50 sends; every existing member persists 50 new
  `timeline_events` rows and emits 50 timeline diffs (inbound
  tasks dispatch as full timeline items, not silent folds).
  `Room::tasks()` does not grow. No cap is specified; if it
  hurts, a cap or Option D's bundled payload is a new decision.
- **The §72 asymmetry remains on the case branch**: a non-admin's
  invite into a case room delivers marker/tasks but the case
  snapshot is discarded by the joiner's fold — the room will not
  render as a case for them until an admin next touches the core.
- **Recovered devices stay blind**: a fresh kupferdraht.db that re-enters
  via external commit has no inviter, so neither marker nor tasks
  nor case arrive until the room's next organic activity. Same
  class of residual as ADR-0018; a state-sync-request mechanism
  would be its own ADR.
- Existing task-only rooms need a one-time manual marker set before
  any of this applies to them.

### Neutral

- Field 262002 extends the 262000-family reservation
  (tasks 262000, cases 262001).
- Official clients see nothing of the marker or the re-sent task
  states (bare payloads, no fallback) — consistent with ADR-0014's
  dedicated-task-list mode.
- The `send_task` default change is observable API behavior; the
  step records it consumer-facing in the CHANGELOG (BREAKING if the
  signature changes rather than the default).

## Implementation Notes

- Proto: `TaskListMarkerPayload { marked: bool }` as field 262002 in
  `generic_message.proto`; encode/decode next to the task codec.
- Fold: room-level marker fold on read (event order, last write
  wins); no `sender_was_admin` capture — the V26 role column is not
  consulted for the marker.
- API: `Room::is_task_list()`, `Room::set_task_list(bool)`; FFI
  mirrors in the same commit (CLAUDE.md parity rule);
  api-mapping entries (both Kupferdraht-specific).
- Invite hook: extend the 72c block in `Client::invite_members` —
  compute the union (case snapshot if present, marker if marked,
  task helper once if either). Task re-sends go
  `with_fallback: false` unconditionally (they are state
  transfer, not user speech).
- `send_task` / `update_task` / `move_task`: `with_fallback` becomes
  `Option<bool>` in Core and FFI (BREAKING, recorded at land); the
  shared send tail reads the marker fold only when the caller passed
  `None`.
- Marker rows surface as `TimelineItemKind::TaskListMarker` /
  `MessageBody::TaskListMarker` (the Case precedent): rows stay in
  the timeline as the audit trail of who flipped the any-member
  marker, the fold comes from `Room::is_task_list()` — never
  silently filter them from `messages()` or the diffs; consumers
  render a system line or hide repeats themselves.
- Tests follow the Step-74 fixture line (`PeerGroup` +
  `add_member_commit`): late joiner folds marker + tasks from the
  invite re-broadcast; existing member's `Room::tasks()` is
  unchanged (fold no-op) even though new timeline rows arrive;
  a room that is both case and marked sends case + marker + N
  tasks, not 2N; non-admin inviter in a case room delivers tasks
  but not the case (pins the accepted §72 residual).

## References

- Builds on: ADR-0014 (task payloads, event-order LWW,
  membership-is-ACL, fallback modes, join snapshot as a batch of
  ordinary updates on invite — the marker joins this family;
  recovered devices remain deferred per 0014#6),
  ADR-0018 (invite re-broadcast hook and ordering; the case
  snapshot MUST; the §72 residual of the admin-gated fold).
- Constrained by: ADR-0009 (matrix-rust-sdk-shaped public API),
  CLAUDE.md Wire Message Types (GenericMessage Protobuf only).
- Related: ADR-0004 (external senders / administrative proposals —
  the other "room-level authority" precedent), observations §72
  (the non-admin inviter hole), open.md §67 / §63,
  `docs/plan/75-task-list-room-marker.md` (Step 75).

## Revision History

- 2026-08-24: Created
- 2026-08-24: Accepted (Volker: "a to d as recommended")
- 2026-08-25: Clarifications before Step 75 (same decision, not a
  supersession): invite hook is the **union** of both anchors with
  the task helper run once; Option D (bundled board) recorded as
  rejected; existing-member "no-op" narrowed to folded state —
  timeline rows and diffs still fire; "four" → five sub-decisions.
  ADR-0014 decision 6 and ADR-0018 decision 2–3 were amended in
  the same pass so each ADR states its own current truth; this ADR
  no longer patches them.
- 2026-08-26: Decision 4 clarified before implementation (same
  decision, Volker): the derivation covers all three task send
  paths — `send_task` / `update_task` / `move_task` share one send
  tail, and only a shared contract delivers the Positive
  consequence "senders stop deciding the fallback per message";
  Core and FFI both take `Option<bool>` (a `bool` mirror would
  keep the footgun for bindings; BREAKING recorded at land).
  Implementation Notes: marker rows surface as
  `TimelineItemKind::TaskListMarker` (Case precedent, audit
  trail of the deliberately forgeable any-member flag) — never
  silently filtered from `messages()`.
