# Task-list marker + invite state re-broadcast

Condensed from `docs/adrs/0020-task-list-marker-and-invite-state-rebroadcast.md`.

- **`TaskListMarker` is field 262002** (bare payload, no fallback —
  official clients see nothing): full marker state, event-order LWW
  like tasks, **any member** sets/clears it. No admin gate, no role
  capture, no revision field.
- **Invite re-broadcast is a MUST as the union of both anchors**:
  after the Add-commit merges, the inviter re-sends — `Room::case()`
  is `Some` → case snapshot; `Room::is_task_list()` → marker;
  **either (or both)** → every folded task **once**. A room that is
  both a case and a marked task list therefore sends case +
  marker + N tasks, not 2N. Unmarked rooms with a delegated task
  are not an anchor. Best-effort, warn on failure. All tasks, not
  only open ones; task re-sends go `with_fallback: false` always.
- **Re-sends are real MLS + timeline traffic** for existing members
  (`timeline_events` rows and diffs fire). Folded `Room::tasks()`
  does not duplicate. Don't invent a bundled `TaskBoardPayload`
  without a new ADR.
- **All three task send paths default their fallback from the
  marker** (`send_task` / `update_task` / `move_task`; marked →
  off); an explicit caller choice wins. Core AND the FFI mirror
  take `Option<bool>` — don't leave a `bool` mirror that keeps the
  footgun for bindings.
- **Marker rows surface as `TimelineItemKind::TaskListMarker`**
  (audit trail of the forgeable any-member flag, Case precedent) —
  never silently filter them from `messages()` or the diffs;
  consumers render a system line or hide repeats themselves.
- **Don't** "fix" the §72 case-fold hole with a first-snapshot
  exception (ADR-0018 residual: a non-admin's invite delivers
  marker/tasks but not the case), and don't cap or filter the task
  re-broadcast without a new decision. Device recovery via external
  commit stays out of scope per ADR-0014 decision 6.
