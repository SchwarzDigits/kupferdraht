# Tasks as event-sourced message content

Condensed from `docs/adrs/0014-tasks-as-event-sourced-message-content.md`.

- **Serialize tasks as `TaskPayload` in `GenericMessage` field 262000, outside the `content` oneof** (renumbered from 1001 pre-merge 2026-07-17 — see the ADR's Revision History). In normal chats also set the `Text` variant as human-readable fallback; in dedicated task-list conversations leave the oneof unset. Receive-side dispatch checks the task field before the oneof.
- **Official clients see tasks (mode c) but never interact** — their replies to the fallback text are ordinary text messages in the timeline, never task state.
- **Carrier interactions are fixed:** a `MessageDelete` on a task message reverts the fold to the prior surviving state (sole-event delete removes the task); a `MessageEdit` on a task carrier is a no-op on task state (`kind='text'` guard); reactions are independent of the fold. Don't route task mutations through edit/delete — task changes travel as new full-state messages.
- **Every task message carries the full state of one task** — create and update are the same message; fold in event-stream server order, last writer wins per task; an unknown `task_id` upserts (creates local state).
- **Key tasks by their own `task_id` (UUID)** — never by `message_id` (edit chains mutate it).
- **Order lists by lexicographic fractional `sort_key`** — never integer positions. Absent key → due date, then priority, then created-at.
- **Unknown `status` values decode as `open`.**
- **Join snapshot is a batch of ordinary updates on invite** (same messages as `send_task`, no second payload). ADR-0020 names the anchors and the union rule; this ADR owns the mechanism. Unmarked rooms stay empty-on-join.
- **Don't build server-side task state, reminders, or search** — derived state is the local materialized `tasks` table only (`Room::tasks()`, `my_tasks()`, `delegated_tasks()`).
- **Don't gate task features on peer-client capabilities** (v14 exposes none) — degrade gracefully instead.
- Any member may update any task (client convention, like reactions); the folded log is the audit trail.
- Out of scope for V1: join snapshot for recovered devices (external commit); bridges (separate `kupferdraht-bridge-*` binaries, never in `kupferdraht-core`).
