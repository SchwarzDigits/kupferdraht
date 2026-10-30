# Step 60 — Tasks / todos (event-sourced, ADR-0014)

> **Status:** COMPLETE — all three sub-steps implemented and
> live-smoked against Wire production 2026-07-17 (incl. the
> 1001 → 262000 renumber re-smoke and the mode-a badge check).
> Branch `step/60c-task-views`: 60a `36b495d`, 60b `4227e40`,
> 60c `3511054` + `1793cec` (ISO-8601 CLI timestamps follow-up);
> PR pending at archive time. Scope: TaskPayload send/receive in
> both encoding modes, full-state LWW folding with upsert,
> materialized `tasks` table (V23) with cross-room views
> (`my_tasks` / `delegated_tasks`), fractional-index manual ordering,
> CLI + FFI parity. Architecture fixed in ADR-0014 (+ same-named
> rule); full design discussion in `docs/design/tasks-todos.md`.
> Charter adopted 2026-07-16. Self-conversation verified broken →
> own step (`open.md` §63).
>
> **Sub-steps:** 60a (payload + send/receive + CLI), 60b (update folding),
> 60c (list views + ordering). One branch and one commit per sub-step.
>
> **Branch / commit prefix:** `step/60a-*` etc., `feat(tasks): …`.

## Why

Todos/tasks inside conversations, E2EE end to end: personal list, delegated
tasks in 1:1 chats, shared task-list conversations, and cross-conversation
views ("all my open tasks", "all I delegated"). The server learns nothing —
tasks are event-sourced `GenericMessage` content folded locally, not
server-side objects (the Google Chat model is unreachable under E2EE;
see ADR-0014 Options).

Binding rules: `rules/0014-tasks-as-event-sourced-message-content.md`.
Out of scope for this step: join snapshot (deferred past V1, ADR-0014
decision 6), bridges (`kupferdraht-bridge-*`, separate binaries), SCRUM/process
tooling (anti-goal).

## 60a — TaskPayload + send/receive + CLI

- **Proto** (`generic_message.proto` + `docs/wire/message-types.md`, same
  commit): `TaskPayload` message; `optional TaskPayload task = 262000;`
  (renumbered from 1001 pre-merge 2026-07-17 — headroom insurance)
  **outside** the `content` oneof. Fields per ADR-0014: `task_id`
  (required, UUID), `title` (required), `status` (required enum
  `open | in_progress | done | cancelled`), optional `description`,
  `assignee` (`QualifiedUserId`), `due_at_ms`, `priority`
  (`low | medium | high | urgent`), `sort_key`.
- **Send** (`timeline/task.rs`): `Room::send_task(...)` through
  `Client::encrypt_and_upload`. Fallback mode is an explicit parameter in
  V1 (mode c: `Text` fallback set, for normal chats / mode a: oneof unset,
  for task-list rooms); a room-level default may follow in 60c.
- **Receive** (`sync/dispatch/`): dispatch checks field 262000 **before**
  the oneof; `TimelineItemKind::Task`; payload persisted as JSON in
  `timeline_events.body` (the `Location` small-payload precedent).
  Unknown `status` decodes as `open`.
- **CLI:** `kupferdraht task add`, `kupferdraht task list` (per room).
- **TDD list:** proto round-trip (all fields, minimal fields); message
  with 262000 + `Text` dispatches as Task, not Text; bare 262000 (no oneof)
  dispatches as Task; unknown-variant envelope still log-and-discard;
  unknown status → `open`; fallback text carries assignee mention
  (UTF-16 offsets); CLI output.
- FFI mirror + `api-mapping-matrix-rust-sdk.md` entries ("Kupferdraht
  extension, no matrix equivalent") in the same commit.

## 60b — Update folding (full-state LWW + upsert) — implemented, smoked

> Live smoke 2026-07-17 against Wire production (same room/clients as
> the 60a smoke): folded `task list` on both clients; `task done` sent
> by the assignee folded to `☑ done` on the creator's client; `assign`
> and `edit` (priority + status) folded on the counterpart; a reopen
> (`done → open`) round-tripped; the list re-sorted live when a task
> became urgent.

- Fold task events in arrival order (the local proxy for the event
  stream's server order); last writer wins per `task_id`; an update
  for an unknown `task_id` **upserts** — create and update are the
  same message. Fold-at-read: `fold_task_rows` over the room's
  `timeline_events`, exposed as `Room::tasks()` (pulled forward from
  60c because `update_task`'s read-modify-write needs it anyway).
- `Timeline::update_task(task_id, TaskPatch, with_fallback)` —
  set-only patch onto the folded state, sent as full state. Clearing
  an optional field is deliberately not expressible in V1 (nested
  `Option` doesn't cross FFI); the wire format supports it.
- Status transitions incl. reopen (`done → open`) are ordinary
  updates; any transition is legal (no state machine); `cancelled`
  retires a task. The mode-c fallback checkbox reflects the status
  (`☑` done, `☒` cancelled).
- **Delete stance (resolved in review):** the original note claimed a
  `MessageDelete` "does not retract folded state" — wrong, because
  our delete wipes the row body (privacy, 6-II-d). Actual semantics,
  test-pinned: a deleted task message drops out of the fold and the
  prior state resurfaces; a task whose only message was deleted
  disappears.
- **CLI:** `kupferdraht task done`, `task assign`, `task edit`; `task list`
  now shows the folded view.
- **TDD list (all green):** LWW wins entirely (no field bleed); upsert
  from a bare update; full 4×4 transition matrix; redacted-latest
  fallback + redacted-only disappearance; malformed/non-task rows
  skipped; multi-task independence; redelivery idempotence; default
  ordering due → priority → first-seen.

## 60c — List views + ordering — implemented, smoked

> Live smoke 2026-07-17 against Wire production (same room/clients as
> 60a/60b): V23 migration + cache backfill reproduced the folded list
> on both clients; `task add` surfaced instantly in the creator's
> `--delegated` and (once delivered) the assignee's `--mine`;
> `task done` dropped the task from `--mine` and showed `☑ done` in
> the creator's `--delegated`; the bootstrap move (no neighbors)
> seeded the room's first `sort_key`, `--before` placed a second key,
> and the manual order folded identically on the counterpart; the
> assignee's counter-move reordered locally. Caught during this
> smoke: the first move run hit a stale CLI binary (pre-bootstrap-fix
> error text) — rebuild before smoking.

- **Store (fold-on-write, chosen for scale):** materialized `tasks`
  table (V23; PK `(room_id, task_id)`, indexes `(assignee, status)`
  and `creator_user_id` — `docs/ARCHITECTURE.md` §7). Maintenance:
  send + inbound dispatch upsert (first-writer-wins on
  creator/created_at; a redelivered duplicate does not regress the
  cache); a redacted task message triggers a per-room re-fold; a
  one-time backfill on resume populates the table after the migration
  (`COUNT` short-circuit when warm). `Store` trait methods carry
  default no-op bodies so the IndexedDB backend compiles unchanged
  (WASM rework lands its own impl).
- **API:** `Room::tasks()` now reads the materialized table;
  `Client::my_tasks()` (assigned to me, open/in-progress) and
  `Client::delegated_tasks()` (created by me, assigned to someone
  else, any status) are index-backed cross-room reads.
- **Ordering:** lexicographic fractional `sort_key` (LexoRank
  principle, lowercase `a`–`z`; a move touches exactly one task;
  generated keys never end in `a`). Keyed tasks first, then due date,
  then priority, then first-seen. `Timeline::move_task(task_id,
  after, before, with_fallback)` — the new key travels as an ordinary
  full-state update; a referenced neighbor must already carry a key;
  both neighbors absent = move to the front (the bootstrap — caught
  in review: without it a room's first move had no valid call shape).
- **CLI:** `kupferdraht-cli task list --mine | --delegated`,
  `kupferdraht-cli task move --after/--before`.
- FFI mirrors: `RoomTask` record, `Timeline.move_task`,
  `Client.my_tasks` / `Client.delegated_tasks` — coordinate binding
  regeneration with the WASM rework in flight.

## Smoke (every sub-step has backend surface)

- Round-trip against Wire. **60a: done 2026-07-17 against Wire
  production** (two CLI clients + official client).
- **Ship-blockers from ADR-0014 — verified 2026-07-17:** the official
  Wire client rendered the mode-c fallback text as a plain message
  (mention intact) and ignored the mode-a bare payload without error,
  placeholder, or crash; a text sent from the official client
  afterwards arrived on both Kupferdraht clients (group healthy).
- Sequencing rule learned in this smoke: the receiver's CLI device
  must be an MLS member **before** the task is sent — log in the
  second client before inviting, or run `room-join` after; an event
  that arrives pre-join is skipped and does not replay.
- **Renumber re-smoke (2026-07-17, after 1001 → 262000):** A→B
  round-trip in both modes under the new field number; official
  client rendered mode-c (mention intact; its quote-reply arrived as
  an ordinary text message — the carrier-interaction stance observed
  live); mode a fully UX-invisible in the official client: no unread
  badge, no conversation bump, no push (the badge seam from external
  review does not exist).

## Out of scope / follow-ups

- **Join snapshot** — own step later (ADR-0014 decision 6).
- **Self-conversation** for the personal list: **verified broken in
  the 60c smoke (2026-07-17)** — `task add` into the self room fails
  with `MlsGroupMissing` (server shows epoch 40, no local group
  state; the row persists locally as a failed send, the local-echo
  path, but never leaves the device). Establishing/joining the self
  conversation is its own step.
- **Bridges** (Apple Reminders / GitLab / Jira / Google Tasks) and IDE
  integration — separate projects consuming the public API.
- **Room-level fallback-mode default** ("this room is a task list →
  mode a implied") — floated for 60c, not built; `with_fallback`
  stays an explicit parameter in V1.
- **Due dates are instants, not calendar days** — `due_at_ms` carries
  a UTC instant; "due July 20" as an all-day *date* (what Google
  Tasks / Apple Reminders model) is not expressible. The CLI's
  bare-date input maps to 23:59:59 local as a convention. Revisit for
  the bridges step (all-day mapping).
- **Cache `updated_at` source differs by path** (incremental upsert
  stamps drain/send time, a rebuild re-derives from row `sent_at`) —
  millisecond-level, per-device-divergent metadata anyway; noted, not
  scheduled.
