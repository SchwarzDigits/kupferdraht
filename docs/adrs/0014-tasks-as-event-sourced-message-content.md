# ADR-0014: Tasks as event-sourced GenericMessage content

**Status:** Accepted
**Date:** 2026-07-16
**Deciders:** Volker Zöpfel

## Context

Kupferdraht is to support todos/tasks inside conversations: a personal list
(vehicle: the self-conversation), single delegated tasks in 1:1 chats,
shared task-list conversations (MLS membership *is* the ACL), and
cross-conversation views ("all my open tasks", "all tasks I delegated,
with status"). SCRUM/process tooling (boards, sprints, reporting,
automation) is an explicit anti-goal — it wants a server-side system of
record and fights E2EE structurally.

The E2EE constraint shapes everything: the backend sees only MLS
ciphertext. Google Chat's task model — tasks are server-side objects in
the Google Tasks service, chat messages merely reference them (verified
against the Tasks API docs, 2026-07-16) — presupposes a server that
reads plaintext. Under MLS, that architecture would make exactly the
structured, business-relevant content the one thing *not* end-to-end
encrypted.

Interop facts established during design (see
`docs/design/tasks-todos.md`): unknown `GenericMessage` content does not
break official Wire clients — protobuf skips unknown fields, the oneof
decodes unset, clients log-and-discard; this is the same forward-compat
behavior Wire itself relies on when shipping new variants (`Composite`,
`DataTransfer`, `Multipart`), and the same rule Kupferdraht already follows.
Peer-client capability discovery does not exist in Wire v14 (`PubClient`
carries only `id` + `class`; the capabilities endpoint is self-only),
and gating on dynamic membership would be brittle anyway — graceful
degradation is the only robust interop stance.

Constraints from prior decisions: the public API stays
matrix-rust-sdk-shaped (ADR-0009; tasks have no matrix equivalent — a
Kupferdraht extension), the CLI is the API's first consumer (ADR-0007), the
event stream is v14's server-ordered `ConsumableNotifications` model
(ADR-0008), and all Wire content is `GenericMessage` protobuf (charter).

## Options considered

**1. Server-side task service (Google Chat model).** Tasks as plaintext
REST objects; messages reference them. *Rejected:* requires a
plaintext-reading server — breaks the E2EE core promise for precisely
the most sensitive structured content.

**2. Reference to an encrypted blob (asset pattern, ADR-0011).** Task
state as an off-MLS encrypted blob; messages carry the reference.
*Rejected:* blobs are immutable, so every status change is a new blob +
a new reference message — the event pattern again, plus an HTTP
round-trip and key handling per update; the server still cannot query,
remind, or backfill; the only gain (payload size) is irrelevant for
tasks.

**3. Event-sourced task state in messages (chosen).** Tasks are
immutable events in the conversation; every client folds the log into
current state locally. The pattern exists three times in Kupferdraht already
(`MessageEdit`/`MessageDelete`/`Reaction` folding into `TimelineItem`);
Matrix models room state the same way under the same constraint.

Encoding sub-options for option 3:

- **(a) bare custom variant** — clean, but invisible in official
  clients everywhere.
- **(b) JSON inside `Text.content`** — *rejected:* schema-free,
  fragile, renders as raw JSON.
- **(c) Text fallback + structured side-field** — official clients
  render a human-readable line (mentions still notify the assignee),
  Kupferdraht clients parse the structured field (Matrix "extensible
  events" pattern); cost: updates read as a chatty activity log in
  official clients.

## Decision

Tasks are event-sourced `GenericMessage` content, with a hybrid (c)/(a)
encoding through **one mechanism**:

1. **One proto field, outside the oneof:** `optional TaskPayload task =
   1001;`. In normal chats the message *also* sets the `Text` variant
   as human-readable fallback (mode c); in dedicated task-list
   conversations the oneof stays unset (mode a) — official clients
   treat that exactly like an unknown variant. Receive-side dispatch
   checks field 1001 before the oneof. 1001 sits far above Wire's
   occupied range and outside protobuf's reserved 19000–19999.
2. **`TaskPayload`:** `task_id` (required UUID — deliberately *not* the
   `message_id`, which edit chains mutate), `title` (required),
   `status` (required: `open | in_progress | done | cancelled`; unknown
   values decode as `open`), plus optional `description`, `assignee`
   (`QualifiedUserId`), `due_at_ms`, `priority`
   (`low | medium | high | urgent`), `sort_key`.
3. **Full-state updates, LWW, upsert.** Every task message carries the
   complete state of one task; create and update are the same message.
   Receivers fold in server order of the event stream (last writer wins
   per task — same rationale as Wire's full-set `Reaction` semantics).
   An update for an unknown `task_id` creates local state, so any
   single event fully reconstructs a task.
4. **Ordering by fractional index.** `sort_key` is a lexicographically
   ordered fractional index (LexoRank principle): a manual move touches
   exactly one task = one message. Integer positions rejected
   (renumbering cascades; concurrent moves shred the list under
   per-task LWW). Absent `sort_key` → due date, then priority, then
   created-at.
5. **Authorization is client convention.** Any member may update any
   task (like reactions). No server enforcement exists under E2EE; MLS
   authenticates every sender and the event log is a complete audit
   trail.
6. **Join snapshot is a batch of ordinary updates, on the invite
   path.** With full-state semantics the snapshot *is* a re-send of
   each folded task as the same messages `send_task` already
   produces — no second payload type. That batch fires in
   `Client::invite_members` after the Add-commit merges (ADR-0018
   hook order). ADR-0020 names the anchors that trigger it (case
   room and/or marked task list) and requires the helper to run
   once; this ADR owns the mechanism. Unmarked 1:1s / group chats
   that merely contain a delegated task stay empty-on-join.
   Recovered devices that re-enter via external commit stay
   deferred: no inviter, no batch, until a later organic update.

Derived state is local only: a materialized cross-conversation `tasks`
table feeds `Room::tasks()`, `Client::my_tasks()`,
`Client::delegated_tasks()`.

## Consequences

**Positive**

- Tasks inherit MLS E2EE wholesale (PFS/PCS, membership-as-ACL); the
  server learns nothing.
- Cross-conversation aggregation is a local store query — the thick
  client already holds a full replica; works offline.
- Complete, sender-authenticated change history for free.
- Full-state events make bridges (separate `kupferdraht-bridge-*` binaries,
  bridge-as-client pattern) and late-join healing trivial.
- Graceful degradation in mixed rooms: fallback + assignee mention in
  chats; list rooms appear empty to official clients rather than
  spammy.

**Negative / accepted costs**

- Official clients can *see* (mode c) but never *interact* — creating
  and updating requires Kupferdraht-family clients on every participating
  device. The practical constraint is app availability, not protocol.
- No server-side reminders, search, or due-date push — reminders are
  client-local, per device.
- Invitees into an unanchored room, and recovered devices that
  re-enter via external commit, still start with an empty list
  (mitigated incrementally by upsert). Invitees into an anchored
  room receive the board (ADR-0020).
- Full-state LWW loses one side when two members concurrently edit
  different fields of the same task — accepted at todo scale.
- Field 1001 carries a residual collision risk if Wire's schema ever
  grows there; collision strategy documented in
  `docs/wire/mls-deviations.md`.
- "Official clients ignore it": **live-verified 2026-07-17** against
  Wire production (Step 60a smoke) — the official client renders the
  mode-c fallback and shows nothing for mode-a; the group stays
  healthy afterwards (details: `docs/wire/mls-deviations.md` W-10).

**Neutral**

- Charter/scope extension (CLAUDE.md Functional Scope, README,
  PLAN.md) in the adopting commit.
- New `pub` symbols need api-mapping entries ("Kupferdraht extension, no
  matrix equivalent") and FFI parity across Kotlin/Swift/WASM
  (coordinate with the WASM rework in flight).
- Self-conversation is classified (`RoomType::SelfRoom`) but unverified
  for establish/send — folded into the live-smoke items.

## Implementation Notes

Step sequencing: ①–③ shipped (payload, fold, materialized views).
④ invite-path snapshot is ADR-0020 / Step 75 (batch of ordinary
updates on the shared invite hook). Device-recovery snapshot remains
deferred. Bridges later, outside `kupferdraht-core`. Full discussion:
`docs/design/tasks-todos.md`.

## References

- Builds on [ADR-0008](0008-target-wire-api-v14.md) — the LWW fold
  order presupposes the v14 event stream's server ordering.
- Builds on [ADR-0007](0007-cli-in-rust-not-jvm.md) — the CLI is the
  first consumer of the task API.
- Constrained by [ADR-0009](0009-matrix-rust-sdk-as-api-model.md) —
  public task symbols follow matrix-rust-sdk shape; Kalium is no model.
- Related: [ADR-0011](0011-asset-content-protocol.md) — same
  `GenericMessage` content layer; its blob-reference pattern was
  evaluated and rejected as Option 2.
- Related: [ADR-0004](0004-integrate-external-senders-extension.md) —
  the backend's external-sender membership proposals bound the
  "membership = ACL" property of task-list conversations.
- Related: [ADR-0013](0013-sqlcipher-engine.md) — the materialized
  `tasks` table lives in the SQLCipher-encrypted store.
- Related: [ADR-0018](0018-case-state-as-event-sourced-snapshots.md)
  — the shared invite re-broadcast hook this decision's snapshot
  rides.
- Related: [ADR-0020](0020-task-list-marker-and-invite-state-rebroadcast.md)
  — room marker, anchors, union rule, and `send_task` fallback
  default; does not replace this decision's snapshot mechanism.
- Design brief: `docs/design/tasks-todos.md` (full discussion,
  2026-07-16).

## Revision History

- 2026-07-16: Created
- 2026-07-17: Accepted — Step 60a implemented; live smoke on Wire
  production verified the interop ship-blockers (official client
  renders mode-c fallback, ignores mode-a bare payload, group healthy)
- 2026-07-17: `TaskPayload` field renumbered 1001 → 262000 pre-merge,
  before any deployment (external-review suggestion: headroom
  insurance for the one assumption depending on Wire's future schema;
  costs one tag byte). Decision text above intentionally unchanged
  (immutability); current number in `rules/`, the proto, and
  `docs/wire/mls-deviations.md` W-10.
- 2026-07-17: Carrier-interaction semantics (MessageDelete reverts
  the fold, MessageEdit is a no-op on task state, Reaction is
  independent) clarified after external review — chosen and
  test-pinned, recorded in `docs/design/tasks-todos.md` §6 and
  condensed into the `rules/` sibling.
- 2026-08-25: Decision 6 amended (same mechanism, current truth —
  not a supersession). The original "deferred past V1" left this
  ADR lying once the invite-path batch shipped; a later ADR must
  not have to correct it. Snapshot = batch of ordinary updates, on
  invite; unmarked rooms and recovered devices remain deferred.
  Anchors / union / marker live in ADR-0020.
