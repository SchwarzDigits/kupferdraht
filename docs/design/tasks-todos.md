# Design Discussion: Tasks / Todos over MLS

**Status: 2026-07-16 · design discussion captured, pre-ADR · no code yet**

Briefing document for the implementation workstream. Everything below was
discussed and settled between Volker and Claude on 2026-07-16 unless marked
**OPEN**. Nothing here is charter yet — adopting it requires the scope +
ADR work listed in §10.

## 1. Goal and use cases

Manage todos/tasks inside Kupferdraht conversations, E2EE end to end:

- **Personal todo list** — tasks to self, synced across own devices
  (vehicle: Wire's self-conversation, see §7).
- **Delegated task in a 1:1 chat** — "you need to do X by Friday",
  assignee set, status visible to the creator.
- **Shared task list as a dedicated conversation** — e.g. a shopping
  list with spouse; MLS group membership *is* the ACL.
- **Cross-conversation aggregation** — "all my open tasks" and "all
  tasks I delegated, with status" without browsing conversations (§7).
- Later: IDE integration (§8) and bridges to external task tools (§9).

Anti-goal (agreed): SCRUM/process tooling (boards, sprints, reporting,
automation) — that wants a server-side system of record and fights E2EE
structurally. Personal + small-group todos are the sweet spot.

## 2. Core architectural findings

- **MLS is not the obstacle.** MLS transports opaque application bytes;
  PFS/PCS, group encryption, and membership management apply to tasks
  automatically. All design questions live one layer up.
- **The two viable architectures.** Google Chat models tasks as
  server-side objects in the Google Tasks service; chat messages merely
  reference them (verified 2026-07-16: space tasks appear in the Tasks
  API with `assignmentInfo.ContextType = SPACE`, are readable but not
  creatable via the public API). That reference architecture requires a
  server that reads plaintext — unreachable under E2EE without breaking
  the core promise. The E2EE-world standard (Matrix does the same) is
  **event-sourced state in messages**: immutable log, folded projection.
- **Thick-client insight.** A Kupferdraht client already receives every event
  of every conversation it is in and holds a full local replica. The
  aggregation Google does server-side is a local store query here. No
  server involvement, no server knowledge.
- **Messages stay immutable.** A `TaskUpdate` never mutates anything on
  the wire — it appends an event; every client folds the log into the
  current task state locally. Exactly the existing
  `MessageEdit`/`MessageDelete`/`Reaction` pattern
  (`timeline/items.rs`: `edited_at_ms`, `redacted`, `reactions` are fold
  results, not message content). Audit trail comes for free: the log is
  the history, every event cryptographically attributed to its sender.

## 3. Encoding in the content layer

All Wire content is one `GenericMessage` protobuf
(`kupferdraht-core/proto/generic_message.proto`). Options discussed:

- **A — own oneof variant** (`Task task = <high field number>`): clean
  and typed; invisible in official Wire clients.
- **B — JSON inside `Text.content`**: rejected (schema-free, fragile).
- **C — hybrid with fallback**: a regular `Text` variant carrying a
  human-readable fallback ("☐ Prepare deploy — due 20.07, @anna …")
  **plus** a structured `TaskPayload` field *outside* the oneof (a
  high non-oneof field number). Protobuf decoders ignore unknown fields → official
  clients render the fallback text (mentions even notify the assignee),
  Kupferdraht clients parse the structured field. Matrix "extensible events"
  pattern.

**Decided (2026-07-16): hybrid of both** — Option C for single tasks
inside normal chats; Option A (no fallback) inside dedicated task-list
conversations, because C's per-update fallback messages read as noisy
activity-log spam in official clients, and official clients enter a
dedicated list room as an empty room anyway.

**Encoding unification proposal (for the ADR):** both modes use ONE
proto mechanism — `optional TaskPayload task = 262000;` declared
outside the `content` oneof (adopted with field 1001, renumbered to
262000 on 2026-07-17 pre-merge as headroom insurance — the extra tag
byte is noise next to MLS overhead, and Wire's schema growth of ~20
fields per decade will never reach 262000). Mode C sets the `Text`
oneof variant (the fallback) *plus* the task field; mode A sets the
task field with no oneof content at all (official clients treat that
exactly like an unknown variant: ignore). Receive-side dispatch
checks the task field before the oneof. One field, one parse path,
fallback presence = whether `Text` is set. 262000 is far above Wire's
occupied range (< ~30), outside protobuf's reserved 19000–19999, and
below the 4-byte-tag boundary at 262144; collision strategy
documented per §10 in `docs/wire/mls-deviations.md` W-10.

Compatibility facts established:

- **Unknown variants do not crash official clients** — protobuf forward
  compatibility (unknown fields skipped, oneof decodes unset), and
  Wire's own ecosystem relies on exactly this for every new variant it
  ships (`Composite`, `DataTransfer`, `InCallEmoji`, `Multipart`).
  Kupferdraht's own rule ("deserialize the envelope, log, discard") mirrors
  that behavior. **Must still be verified live** (project rule: live
  smoke is the source of truth) — send a task variant to a conversation
  with an official web client on schwarz-dev-wire and observe.
- **Capability discovery of peer clients is impossible.** Verified in
  the v14 Swagger: `GET /users/{domain}/{uid}/clients` returns
  `PubClient { id, class }` only; `/clients/{client}/capabilities` is
  self-only. Gating on membership capabilities would be brittle anyway
  (membership is dynamic) → design for graceful degradation instead.

## 4. Task data format

One protobuf message, used for create *and* update:

```
TaskPayload {
  task_id      required  — UUID, stable identity (deliberately NOT the
                           message_id: edit chains change message ids,
                           see the Reaction/edit-chain warning in the proto)
  title        required
  description  optional  — links are plain text inside it
  assignee     optional  — QualifiedUserId (already in the proto)
  due_at_ms    optional
  status       required  — open | in_progress | done | cancelled
  priority     optional  — low | medium | high | urgent
  sort_key     optional  — fractional index (LexoRank-style), lexicographic
}
```

Decisions:

- **Full-state updates, not deltas.** Every update carries the complete
  task state. Conflict resolution is trivial LWW in server order of the
  event stream (same rationale as Wire's full-set `Reaction` semantics).
  Concurrent-field-edit loss is acceptable for todos.
- **Upsert semantics.** An update for an unknown `task_id` *creates*
  local state. This makes create == update (one variant, one dispatch
  route) and partially heals the late-joiner problem: any single event
  fully reconstructs the task.
- **Status enum evolution:** unknown status values decode as `open`
  (task stays visible, next full-state update from a newer client heals
  it). Reopen (`done → open`) is just another update, no special case.
- **Ordering:** naive integer positions break under event sourcing
  (renumbering cascade = many messages per drag-and-drop; concurrent
  moves shred the list under per-task LWW). Fractional indexing makes a
  move touch exactly one task = one update message. Rebalancing is lazy
  and rare. `sort_key` absent → default order: due date, then priority,
  then created-at. Design fixed now; implementation deferred to the
  list step.
- **Authorization is client convention.** No server enforcement exists
  under E2EE; MLS guarantees authenticated senders and the log gives an
  audit trail. Policy "any member may update" (like reactions). Record
  in the ADR.

## 5. Known limitations and mitigations

- **MLS join ≠ history.** A new member (or new own device) sees an
  empty list — MLS delivers no history. Mitigation: **snapshot
  pattern** — after a join, an existing member (automated) re-posts the
  full state of every open task; with full-state semantics a snapshot
  *is* just a batch of ordinary updates. **Decided (2026-07-16):
  deferred past V1**; **amended (2026-08-25, ADR-0014#6):** the
  invite-path batch ships (anchors and union in ADR-0020); unmarked
  rooms and recovered devices (external commit) stay deferred.
  Meantime full-state upsert still heals incrementally.
- **No server-side reminders / search / push-on-due.** The server sees
  ciphertext only. Reminders are client-local, scheduled per device.
- **Mixed clients:** official Wire clients can (with Option C) *see*
  the fallback text but never *interact* — both creating and updating
  tasks requires Kupferdraht-family clients on every participating device.
  The real constraint is app availability, not protocol.

## 6. Folding and storage

- New `TimelineItemKind::Task`; folding of updates in the timeline
  pipeline exactly like edits/reactions today (`timeline/` send file +
  `sync/dispatch/` handler; all sends through `Client::encrypt_and_upload`).
- Small-payload storage precedent exists: `Location` packs JSON into
  `timeline_events.body`.
- **Materialized `tasks` table** in the store (ARCHITECTURE §7 schema
  addition), cross-conversation, indexed on assignee/status/due/prio.
  Views: `client.my_tasks()` (`assignee = me AND status != done`),
  `client.delegated_tasks()` (`creator = me AND assignee != me`),
  `Room::tasks()` (per-conversation list, sorted by `sort_key`).
  Self-updating because status changes arrive as events.

**Carrier interactions — how the three existing folding systems act
on a task-carrier message (clarification 2026-07-17, requested in
external review; all three semantics are chosen and test-pinned, see
ADR-0014 Revision History):**

- **`MessageDelete` on a carrier: revert, deliberately.** Kupferdraht's
  delete wipes the row body (privacy, 6-II-d), so the redacted event
  drops out of the fold and the most recent *surviving* full state
  wins — deleting the latest update reverts the task to its prior
  state; deleting a task's only event removes the task entirely.
  Test-pinned (`fold_falls_back_when_latest_row_is_redacted`,
  `fold_drops_task_whose_only_row_is_redacted`); the materialized
  cache rebuilds per room on carrier redaction (Step 60c).
- **`MessageEdit` on a carrier: no-op on task state.** The store's
  `apply_message_edit` carries a `kind = 'text'` guard (test-pinned:
  `apply_message_edit_skips_non_text_rows`) — an edit targeting a
  `kind='task'` row changes nothing. A text edit is therefore never a
  silent task update, and task state cannot drift under edits;
  Kupferdraht-side task changes always travel as new full-state messages
  (`update_task`). Residual display asymmetry: an official client of
  the *same user* (multi-device) can edit the fallback text; other
  official clients then show the edited text while Kupferdraht clients
  ignore the edit — task state stays consistent across all Kupferdraht
  replicas.
- **`Reaction` on a carrier: independent.** Reactions key on
  `message_id` in their own table; the task fold consumes only
  `kind='task'` rows. An official client reacting to the fallback
  shows up as an ordinary reaction on the timeline item and never
  touches task state.
- Live-observed (2026-07-17): an official client's quote-reply to a
  mode-c fallback arrived as an ordinary text message; a mode-a bare
  payload produced no unread badge, no conversation bump, and no push
  in the official client — mode a is UX-invisible, not just
  render-invisible.

**`created_at` tiebreak caveat (noted 2026-07-17):** `TaskPayload`
carries no `created_at` field, so the last sort stage is *first-seen*
(local arrival of the event that materialized the task) — which is
also what the docs call it. A late joiner who reconstructs a task via
upsert from an update inherits that update's arrival time, not the
original creation time. Tiebreak-only, never correctness. If the
proto is touched for the join-snapshot step, an
`optional int64 created_at_ms` echoed through full-state updates
would heal this for late joiners — deliberately not pulled into
Step 60.

## 7. Personal list: self-conversation

Wire's self-conversation (all own devices, "note to self") is the
natural home for the personal todo list — E2EE sync across own devices.

**Finding (2026-07-16, code check):** support is *partial*.
`RoomType::SelfRoom` exists as a classification (`rooms/room.rs`; Wire
`ConvType 1` mapped in `wire/dto.rs`, persisted, listed) — but there is
no dedicated machinery for *establishing* the MLS self-conversation
group or sending into it. Whether the generic join/send path just works
for it is unverified → live-smoke item (see §12).

## 8. IDE integration

- **IntelliJ: genuinely easy.** Plugins are JVM/Kotlin; Kupferdraht's
  Kotlin/JVM bindings exist. Tool window consuming the same API as the CLI.
- **Zed: constrained by Zed, not by Kupferdraht.** Its extension model (WASM
  sandbox: languages, themes, slash commands, context servers) offers no
  free-form UI panels today. Route: CLI or a local MCP server over the store.
- **CLI is the universal adapter** and exists by rule anyway (ADR-0007:
  CLI is the API's first consumer) — `kupferdraht task list --mine` etc.
- Embedded Kupferdraht in an IDE = **own MLS device** (own identity, own key
  packages, visible in the device list) — the clean multi-device way.
  Sharing one local store between app and plugin trades that for
  locking/lifecycle problems; not recommended.

## 9. Bridges to external task tools

Pattern: **bridge-as-client** (Matrix precedent, mautrix). The bridge is
a normal conversation member that folds task events and mirrors them via
the external tool's REST API; external changes come back as ordinary
`TaskUpdate` messages. Lives in a **separate binary** (`kupferdraht-bridge-*`),
never in `kupferdraht-core` (layer rule); consumes only the public API — zero
new core symbols expected (a good API-first litmus test).

- **A bridge is a deliberate, per-list E2EE opt-out** — everything
  bridged is plaintext in the external tool. Must be an explicit
  decision per list, never a default.
- Membership model: shared lists → bridge as a *visible bot user*
  (transparency through membership); personal lists → bridge as an
  additional own device on user-controlled hardware.
- Mechanics: id-mapping table (task_id ↔ external key), echo
  suppression (origin tracking), LWW consistent with §4. Full-state
  updates make mirroring trivial.
- Per tool: **Apple Reminders** — best fit, local EventKit in the
  Swift app layer, no daemon, no cloud middleman. **GitLab** — best
  cloud case (webhooks, clean API). **Jira** — fine (REST + webhooks).
  **Google Tasks** — polling only (no push); public API cannot create
  space-assigned tasks, personal lists fully writable.
- Bridge daemon holds Wire credentials + external OAuth tokens → run on
  user-controlled hardware (e.g. next to schwarz-dev-wire). Offline
  bridge loses nothing: the v14 event stream retains events.

## 10. Process requirements before any code

- **Charter change:** extend CLAUDE.md "Functional Scope", README
  "What it does", and PLAN.md — same commit (scope rule).
- **ADR + same-named `rules/` file** covering: encoding decision
  (A/C hybrid), full-state LWW conflict model, interop stance,
  authorization-as-convention, fractional-index ordering, field-number
  strategy for `TaskPayload` (collision risk with future Wire fields —
  pick high number, document in `docs/wire/mls-deviations.md` or
  observations).
- New `pub` symbols → `docs/api-mapping-matrix-rust-sdk.md` entries
  ("Kupferdraht extension, no matrix equivalent").
- FFI parity Kotlin/Swift/WASM in the same commits — **coordinate with
  the WASM rework in flight** (colleague, since 2026-07-05).
- Every step follows the 12-phase procedure (tests first, hard stops).

## 11. Proposed step breakdown (needs PLAN.md adoption — user's OK)

1. **Core:** `TaskPayload` in the proto, `TimelineItemKind::Task`,
   send path + dispatch handler, CLI `kupferdraht task add/list`.
2. **Mutation:** update folding (complete / re-assign / re-schedule),
   upsert semantics, CLI `task done/assign`.
3. **List view:** materialized `tasks` table, `Room::tasks()`,
   `my_tasks()`, `delegated_tasks()`, `sort_key` implementation.
4. **Join snapshot** — invite-path batch of ordinary updates
   (ADR-0014#6 as amended 2026-08-25; anchors/union in ADR-0020).
   Device recovery via external commit remains deferred.
5. Later, separate project: `kupferdraht-bridge` (first target: GitLab or
   Apple Reminders).

## 12. Open decisions checklist

- [x] Encoding: **hybrid decided** — Option C in chats, Option A in
      dedicated list conversations; unified single-field mechanism
      proposed in §3 (2026-07-16).
- [x] Self-conversation: **partial** — classification exists, establish/
      send-into unverified; folded into the live-smoke item (§7).
- [x] Snapshot: **invite-path batch in V1** (amended 2026-08-25,
      ADR-0014#6 / ADR-0020); recovered devices still deferred.
      Original 2026-07-16 decision was "deferred past V1" (§5).
- [x] `TaskPayload` field number: **262000** (renumbered from the
      originally adopted 1001 on 2026-07-17, pre-merge — headroom
      insurance, §3); collision strategy documented in
      `docs/wire/mls-deviations.md` W-10.
- [x] Live smoke: **verified 2026-07-17** against Wire production
      (Step 60a) — official client renders the mode-c fallback and
      shows nothing for mode-a; group healthy. Self-conversation
      establish/send remains a 60c smoke item.
- [ ] Public API naming pass (matrix-rust-sdk style; no matrix
      equivalent exists for tasks).

## 13. References

- `kupferdraht-core/proto/generic_message.proto` — envelope, existing
  variants, edit-chain warning under `Reaction`.
- `docs/wire/message-types.md` — variant catalogue, unknown-variant rule.
- `kupferdraht-core/src/timeline/items.rs` — fold-result fields, `Location`
  JSON-in-body precedent.
- `reference/wire.swagger.json` — `PubClient`, capabilities endpoints
  (checked 2026-07-16).
- Google Chat / Tasks API findings (2026-07-16):
  https://support.google.com/chat/answer/9920628 ·
  https://developers.google.com/tasks/reference/rest/v1/tasks ·
  https://github.com/googleworkspace/cli/issues/694
