# Step 75 — Task-list room marker

> **Status:** merged to `main` as `cd9f923` (2026-09-09; red tests
> approved + implemented + live-smoked 2026-08-26 — 10 integration
> tests incl. the union pin and two preservation pins, proto codec
> pin, fallback truth table; CI green). Scope: `TaskListMarker`
> (`GenericMessage` field 262002, bare payload) with any-member
> send/fold (`Room::is_task_list()`/`set_task_list`, own
> `TimelineItemKind::TaskListMarker`), invite hook re-sends the
> union of case snapshot and/or marker plus every folded task once,
> `with_fallback: Option<bool>` on all three task send paths (Core +
> FFI, BREAKING), CLI `task mark` + `--fallback` tri-state. Live
> smoke (schwarz-dev-wire): late joiner's first sync showed marker +
> full board incl. a done task; union room delivered case + marker +
> task; caught the stale-inviter gap (CLI now drains before
> inviting) and Wire's 7-device cap (smoke homes now persistent).
> ADR-0020 amended twice in flight (union rule 2026-08-25; decision
> 4 all-three-paths 2026-08-26). Closes open.md §67.

Commissioned 2026-08-24; ADR-0020 Accepted (2026-08-25
clarifications: invite hook is the **union** of both anchors, task
helper once).
Surfaced by kupferdraht-tui Step 14 (cross-room task views in the
navigation). Scope per ADR-0020: `TaskListMarker` (262002,
any-member, event-order LWW) + `Room::is_task_list()`/`set_task_list` +
invite-hook extension (case snapshot if present, marker if marked,
every folded task **once** if either) + `send_task` fallback default
from the marker.

## Problem

ADR-0014 knows "dedicated task-list conversations" only as a SEND
mode: `send_task(draft, with_fallback: false)` omits the Text
fallback, so official clients show nothing. Nothing marks the ROOM
itself — a client cannot recognize a conversation as a task list,
so a consumer's navigation cannot group them (the TUI ask), and
every sender must independently remember to suppress the fallback.

## Design — ADR-0020 (decided 2026-08-24)

All questions below are resolved by
`docs/adrs/0020-task-list-marker-and-invite-state-rebroadcast.md` /
`rules/0020-…`: any-member marker (a), ALL tasks re-broadcast (b),
`send_task` fallback default from the marker (c), §72 case-side
hole stays an ADR-0018 residual (d). Invite hook is the **union** of
case snapshot and/or marker plus the task helper once (a room that
is both a case and a marked task list must not send 2N tasks).
ADR-0014 decision 6 owns the snapshot mechanism; ADR-0018 decision 2
owns the case-snapshot MUST. The sketch is kept for the record.

## Original sketch (superseded by ADR-0020)

A small marker event in the ADR-0014 family — e.g. a
`TaskListMarker` payload in the same field-number neighborhood
(262000/262001 precedent), full-state LWW like tasks and cases:
set/unset by any member (or admin-gated like ADR-0018 — decide),
folded on read into `Room::is_task_list()`. Open questions for the
ADR:

- Authorization: any member (task precedent) vs. conversation-admin
  (case precedent, ADR-0018)? **Interacts hard with the decided
  point below:** an admin-gated marker inherits ADR-0018's
  undocumented hole — the fold discards a NON-admin inviter's
  re-broadcast when the joiner has no state yet (observations §72),
  and Wire lets any member invite. Any-member gating makes the
  re-broadcast robust from every inviter.
- ~~Late joiners~~ **DECIDED (Volker, 2026-08-24): late-join
  re-broadcast is a MUST** — the ADR adopts the ADR-0018 invite
  re-broadcast (after commit-merge, best-effort warn). Follow-on to
  decide: marker-only, or marker + current task state (a late
  joiner in a marked room otherwise sees an EMPTY task list — the
  §63 "join snapshot for tasks" cut from Step 72; re-sending each
  open task's folded state is idempotent for existing members under
  ADR-0014 LWW but costs one message per task per invite)?
- Should `send_task` default `with_fallback` from the marker, so
  senders stop deciding per message?
- What do official clients see of the marker message itself?
  (Bare payload outside the oneof — nothing, per ADR-0014.)

## Consumer

kupferdraht-tui would list marked rooms under its AUFGABEN navigation
section (today it shows only the cross-room views — Step 60c's
`my_tasks`/`delegated_tasks`).
