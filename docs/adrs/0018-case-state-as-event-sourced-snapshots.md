# ADR-0018: Represent room case state as event-sourced snapshot messages

**Status:** Accepted
**Date:** 2026-08-17
**Deciders:** Volker Zöpfel

## Context

Wire conversations carry almost no client-controllable metadata — effectively
only the name, and conversation metadata is server-side plaintext. CivicSeal
deployments need rich per-room case state (file reference, status,
assignee, deadlines, authority), which must be end-to-end encrypted and must
work over an unmodified Wire backend (independence constraint, ADR-0008/0009).

MLS provides no message history: a member (or any new device) joining later
can never read messages sent before its join epoch (live-verified in Steps
68/69, observations §67). Any design anchored to one specific past message is
therefore unreadable for exactly the members most likely to need it. Wire's
`MessageEdit` is additionally sender-bound: only the original sender can
update an edited message.

Constraints from prior decisions: ADR-0014 established the pattern for
application semantics beyond Wire's set — event-sourced message content in a
custom `GenericMessage` field outside the `content` oneof, folded into local
state, no server-side state.

## Options

### Option A — mutable "first message" holds room values (updated via MessageEdit)

- Pro: single well-known anchor; no new folding machinery.
- Con: unreadable for every late joiner and every new device (no MLS
  history — fundamental, not fixable by convention); only the original
  sender can edit, so case updates freeze when the creator leaves or
  changes device; concurrent-update conflicts unresolvable.

### Option B — fully generic key-value payload

- Pro: new fields need no client release.
- Con: validity and authorization need semantics; expressing them per
  arbitrary key reinvents a schema as convention without compiler help;
  protobuf's unknown-field tolerance already gives typed schemas most of
  the hoped-for evolvability.

### Option C — several specific top-level message types per case aspect

- Pro: typed.
- Con: burns `GenericMessage` field numbers for no gain; fragments dispatch
  and folding; cross-aspect invariants scatter.

### Option D — server-side conversation metadata (name field, properties)

- Pro: no protocol work.
- Con: plaintext to the operator — disqualifying for case content; size
  limits; per-user (properties) not per-room.

### Option E — one typed `CasePayload` snapshot stream (chosen)

Event-sourced full snapshots in a single custom field, admin-gated fold,
generic extensions bag for display-only extras.

## Decision

Case state is represented as **full-snapshot `CasePayload` messages** in a
single custom `GenericMessage` field (262001, sibling of ADR-0014's task
field 262000), outside the `content` oneof, with **no text fallback**.

1. **Snapshot, not delta.** Every update carries the complete case state
   plus a monotonically increasing `revision`. Local state is a
   last-writer-wins fold (revision, then server delivery order). A late
   joiner needs *any one* snapshot after its join, not the first message.
2. **Re-broadcast closes the late-join gap for the case snapshot.**
   The client that performs an invite MUST send the current snapshot
   immediately after the invite's commit. Clients MAY additionally
   re-broadcast on observed member-join events; duplicate snapshots
   are harmless under fold semantics. The same `invite_members` hook
   is shared: ADR-0020 extends it with the task-list marker and the
   task-board batch (union, helper once); this ADR still owns only
   the case snapshot.
3. **Authorization anchors on Wire conversation roles.** Core-field
   changes fold only when the MLS-authenticated sender is a
   `wire_admin` of the conversation at receive time. Conversation
   roles are server-maintained and fetchable at any time, so the
   authorization root has no history problem. The `extensions` map
   carries no authorization semantics (display-only) and folds from
   any member. **Accepted residual (observations §72):** Wire lets
   any member invite, so a non-admin inviter satisfies decision 2
   mechanically while the joiner's fold discards the snapshot
   (`sender_was_admin == false` and no folded case yet). The joiner
   stays caseless until the next admin core update. Do not add a
   first-snapshot exception — that would disable the anti-forgery
   gate for exactly the members who cannot verify the state.
4. **Typed core, generic edge.** The core schema (validated, admin-gated):
   `revision`, `case_ref` (per-room case id), `external_ref`
   (file reference / leading system), `title`, `case_type`, `state`
   (closed enum: OPEN, ACTIVE, WAITING, RESOLVED, CLOSED, CANCELLED),
   `state_detail`, `assignee` (qualified user id), `due_at_ms`,
   `priority` (LOW, NORMAL, HIGH, URGENT), `opened_at_ms`,
   `closed_at_ms`, `authority_ref` (tenant or public authority) — plus
   `extensions: map<string, string>`. Schema growth uses protobuf field
   addition (old clients ignore unknown fields).

## Consequences

### Positive

- Case values are E2E-encrypted; the server/operator sees traffic, not
  content — unlike every conversation-metadata approach.
- Transport-level Wire compatibility: unmodified backend, and plain Wire
  clients silently discard the unknown payload (envelope rule) instead of
  drowning in fallback noise.
- Deterministic multi-writer behavior (snapshot fold),
  creator-independent updates, and a late-joiner story that lands
  when the inviter is an admin.
- Reuses the proven Step-60 machinery (custom field, dispatch, store fold).

### Negative

- Authorization is a **fold convention**, not server enforcement: a
  modified client can send anything; conforming clients uniformly ignore
  it. Same trust model as ADR-0014 — inherent to E2E systems without
  server logic.
- Plain Wire clients see no case state at all (deliberate trade against
  timeline spam).
- Role granularity is Wire's two levels (admin/member) for now; finer
  roles would later ride the typed core, still rooted at Wire admin.
- A room where no snapshot was sent since a member joined leaves that
  member caseless until the next change or re-broadcast. Same class:
  recovered devices that re-enter via external commit have no
  inviter.
- A non-admin's invite delivers a snapshot the joiner's fold
  discards (decision 3 residual, observations §72). Not a hook
  bug — the gate is the point. Heals on the next admin core update.

### Neutral

- One further custom field number (262001) with the task-payload precedent.
- Store gains a `room_case_state` table + fold, mirroring task folding.

## References

- Builds on: [ADR-0014](0014-tasks-as-event-sourced-message-content.md)
  (tasks as event-sourced message content).
- Constrained by: [ADR-0008](0008-target-wire-api-v14.md) (Wire v14 only),
  [ADR-0009](0009-matrix-rust-sdk-as-api-model.md) (layering; no Wire
  names outside `wire/`), the MLS no-history property (observations §67).
- Related: [ADR-0020](0020-task-list-marker-and-invite-state-rebroadcast.md)
  — extends the shared invite hook (marker + task batch, union);
  does not change this ADR's case snapshot MUST or the §72 residual.

## Revision History

- 2026-08-17: Created
- 2026-08-25: Decision 2–3 and Consequences amended (same
  decisions, current truth — not a supersession). The invite hook
  is shared with ADR-0020 (this ADR still owns only the case
  snapshot). The §72 non-admin-inviter hole is named here as an
  accepted residual of the admin-gated fold, so a later ADR does
  not have to correct this one. Positive "solved late-joiner story"
  narrowed to admin inviters.
- 2026-10-07: Wording — "clean-room constraint" becomes "independence
  constraint"; ADR-0016's entry of the same day says why. Not a
  supersession; decision untouched.
