# Step 105 — One store write per event

**Status: MERGED 2026-09-29** — commissioned 2026-09-29 (Markus — residual
of review finding B9, `open.md` §71; found in Step 97e). `dbb1550`;
`43fb097`. Merged to `main` as PR #75 (`b090a38`); CI green. Smoke
2026-09-29 against schwarz-dev-wire (release CLI). Scope shipped: text +
mentions, asset + attachment, edit + mentions, task + fold are one
atomic `Store` call each, inbound and outbound. Retrospective:
`docs/LEARNINGS.md` §5.45. Archived with this header.

## Problem (verified in code 2026-09-29)

Since Step 97e an inbound notification is processed at least once: a
failure replays it. Four events write `kupferdraht.db` twice, and the replay
sees the first write as a duplicate and skips the second for good:

| Event | First write | Second write | Left behind |
|---|---|---|---|
| Text (plain, ephemeral) | `timeline_events` row | mentions | text without mentions, no `on_message` |
| Asset | `timeline_events` row | `attachments` row | an asset that cannot be downloaded |
| Edit | `apply_message_edit` | mentions (replace) | new text, old mentions |
| Task | `timeline_events` row | `tasks` fold row | the task's previous state |

The send side writes the same pairs (text + mentions, asset +
attachment, edit + mentions, task + fold). And an outbound edit of an
edit resolves the mention key *after* the chain pointer advanced — the
lookup then finds nothing and the edit's mentions are never written.

## Decisions (2026-09-29, Markus)

- **D1 — one store call per event.** New `Store` methods, each one
  transaction in the `SQLite` store:
  `insert_timeline_event_with_mentions`,
  `insert_timeline_event_with_attachment`,
  `insert_timeline_event_with_task` (→ `Ok(false)`: the row already
  exists, nothing written) and `apply_message_edit_with_mentions`
  (→ whether the edit applied; mentions keyed by the row's stable id,
  resolved before the edit). A failed second write rolls the first back.
- **D2 — backward compatible.** Default implementations write in
  sequence as before, so custom stores still compile; the `SQLite`
  store overrides them atomically; the docs tell custom stores to do
  the same. The duplicate check moves into the `SQLite` store (the
  constraint's error code instead of matching the message text).
- **D3 — the send side uses them too;** the outbound edit resolves the
  stable id before the edit (fixes the edit-of-an-edit mentions).

## TDD

`storage/sqlite/event_writes.rs`, fault injection with a `SQLite`
trigger that aborts the second write (the Step 97c technique):

- each of the four methods fails as a whole — no `timeline_events` row
  (edit: the old body) survives the failed second write; after the
  trigger is dropped the retry writes both;
- a row that already exists is reported (`false`) and left untouched —
  its mentions are not replaced.
- `tests/step_95b_sender_bound_edit_delete.rs`: an edit of an own edit
  surfaces as an update of the row (red: the second edit's id resolved
  after the chain pointer moved — no row, no update, no mentions).

**As built (2026-09-29).**
- `storage/records.rs`: the four methods with sequential default bodies
  (duplicate check by the error text, as before, for custom stores);
  `MessageEditRecord`.
- `storage/sqlite/event_writes.rs`: `in_write_tx` (one `IMMEDIATE`
  transaction) around connection-level helpers split out of
  `timeline_events` (`try_insert_timeline_event_on` — `false` on
  `SQLITE_CONSTRAINT_PRIMARYKEY` — and `apply_message_edit_on`),
  `mentions::replace_mentions_on`, `attachments::insert_attachment_on`,
  `tasks::upsert_task_on`.
- Inbound: text / ephemeral text (`persist_inbound_text`), asset,
  edit, task (`surface_inbound_message` with the fold row); the simple
  content events go through `insert_timeline_event_with_mentions` with
  no mentions — the dispatch no longer matches the error text.
- Outbound: `persist_and_emit_outgoing_text`, both asset paths,
  `persist_outgoing_task`, and the edit (`persist_local_edit`: stable id
  first). An outbound row that already exists is an error
  (`outgoing_row_exists`).
- Tests: 5 red → green (4 fault-injection + the edit of an edit); 1
  pin (an existing row).
- Smoke 2026-09-29 (schwarz-dev-wire, release CLI): device A = the
  persistent smoke home of user 1, device B = a fresh device of user 2
  (joined the 1:1 by external commit, purged afterwards). A sent texts
  with an `@handle` mention, edited them (again with the mention) and
  added a task; A's and B's timelines both show every edited text with
  its mention, B's `task list` the task. The first read-out looked
  mention-less — the check had cut the lines at 200 characters, before
  the suffix.
