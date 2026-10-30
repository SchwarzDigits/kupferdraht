# Step 109 — Commit a drained page, not each notification

**Status: MERGED 2026-09-30** — in review from 2026-09-29 (Markus —
ADR-0037). `c0ec93d` (ADR-0037 + rule), `f02d070`. Merged to `main` as PR
#83 (`220a03f`), after main and the browser-storage chain were merged into
it: `SqliteStore` keeps both the page gate and the purge's `closed` flag,
and the recovery-state write (Step 112b) takes the page's write permit
like every other write; CI green. Smoke 2026-09-29 (schwarz-dev-wire,
release CLI). Retrospective: `docs/LEARNINGS.md` §5.48. Archived with this
header.

## Problem

Since Step 97e (ADR-0032) the drain commits per notification: each event's
data in `kupferdraht.db` (one transaction per event since Step 105), the
notification's MLS state in `mls.db`, the cursor. Locally that is a few
fsyncs; on the remote storage of ADR-0035 every commit is a round trip. The
Step 108 spike: a page of 100 notifications at 50 ms round-trip time took
16.8 s per notification versus 0.32 s with one MLS transaction and one data
transaction for the page.

## Decisions (2026-09-29, Markus — ADR-0037)

- **D1 — per page, both databases, ADR-0032's order.** One MLS window and one
  `kupferdraht.db` page transaction per fetched page; data commit → MLS commit →
  notifications to the embedder → cursor. The cursor never goes into the data
  transaction (it would pass a lost epoch change after a failure between the
  two commits).
- **D2 — a savepoint per notification** in both databases: a failure or a
  deferral rolls back to it, everything before commits, the cursor stops before
  it. (Replaces the first idea of reprocessing a failed page one notification
  at a time — same effect, no double work.)
- **D3 — other writers wait for the page.** `kupferdraht.db` has one connection
  (and the remote VFS one per database): a write from outside the drain would
  otherwise join the page and be rolled back with it. A page gate in
  `SqliteStore`; the drain's task passes it through a task-local marker.
- **D4 — notifications after the commit**, so the embedder never hears of
  rolled-back data, and a replay never tells it twice.
- **D5 — a `Store` capability** with defaults that group nothing.

The approach was compared with batching only the MLS state (data per event):
simpler, but still about 5–6 s per page remotely. Markus chose full batching.

## TDD

- `storage/sqlite/page.rs` (unit): one commit per page (commit hook); a
  savepoint undoes one notification; a rolled-back page leaves nothing; an
  outside write waits for the page and survives its rollback; reads never
  wait.
- `encryption/atomicity_tests.rs`: an MLS window with two messages, rollback
  to the second — the first stays processed, the second decrypts again; one
  commit.
- `tests/step_109_page_commits.rs`: one page transaction, the cursor after the
  commit; a failed notification keeps the ones before it; a failed page commit
  rolls back and nothing is announced; announcements after the commit; a send
  waits for the whole page; a write during a failing page is kept; a replay
  after the MLS commit is quiet.
- The Step 97e fixture moved to `tests/common/drain_fixture.rs`; `FaultyStore`
  forwards the page methods and records the call order.

**As built (2026-09-29).**
- `storage/page.rs`: task-local page scope. `storage/sqlite/page.rs`:
  `BEGIN IMMEDIATE` / `SAVEPOINT notification_<i>` / `ROLLBACK TO` / `COMMIT`,
  the page gate (`tokio::sync::RwLock`, held exclusively by the page, shared by
  every write method). `storage/sqlite/write_tx.rs`: the 13 per-write
  transactions become savepoints inside a page.
- `Store`: `begin_page`, `savepoint`, `rollback_to_savepoint`, `commit_page`
  (async) and `rollback_page` (sync, so a dropped window can roll back).
- `encryption/provider.rs`: MLS savepoints on the window's transaction.
- `sync/window.rs::PageWindow` (replaces `NotificationWindow`),
  `sync/page.rs` (dispatch a page; a page of one for the push paths),
  `sync/outbox.rs` (held notifications: messages, room invitations, timeline
  diffs, device changes).
- Found while going green: the held notifications must be emitted **before**
  the cursor write — a failed cursor write replays the page, the replay
  dedupes and would never announce them. The page future is boxed (clippy
  `large_futures`).
- Gate 2026-09-29: fmt, 0 warnings, clippy (native all targets, wasm lib),
  1053 native tests, the browser suites, `cargo deny` — green.
- Smoke 2026-09-29 (schwarz-dev-wire, release CLI): user 2 sent 12 texts into
  the 1:1 with user 1; user 1 drained them in one sync (0.9 s) — 12 rows,
  none twice. User 1 replied three times; user 2 received all three, user 1's
  re-sync kept 15 rows, none twice. Both accounts drained their backlog (52
  and 64 notifications) with the new code first.
