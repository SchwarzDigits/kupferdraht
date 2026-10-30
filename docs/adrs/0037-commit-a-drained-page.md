# ADR-0037: Commit a drained page, not each notification

**Status:** Accepted
**Date:** 2026-09-29
**Deciders:** Markus Troßbach

## Context

This ADR supersedes decision item 1 of ADR-0032 "Commit an inbound notification's MLS state only after its data is
persisted" ("one window per notification … not per page"). Its other items — at-least-once with idempotent
persistence, the crash points, rolling back on failure, one MLS writer per window — stand, now per page.

ADR-0032 commits per notification: each event's data in `kupferdraht.db` (Step 105 made that one transaction per event),
the notification's MLS state in `mls.db`, the cursor in `kupferdraht.db`. Locally those are a few fsyncs. On the remote
storage of ADR-0035 every commit is a round trip. The Step 108 spike drained a page of 100 notifications at 50 ms
round-trip time: per notification it took 16.8 s and uploaded 6.4 MB (300 commits); with one MLS transaction and one
data transaction for the page it took 0.32 s and uploaded 56 KiB.

The order of ADR-0032 is what makes a crash safe: the MLS state is committed only after the data, and the cursor
only after the MLS state. A cursor committed before the MLS state could pass notifications whose MLS changes were
lost — for a commit message, the group would stay in the old epoch while the notifications that move it are never
fetched again.

`kupferdraht.db` has one connection, and the remote VFS allows one per database. A transaction held open over a page is
shared by every writer of that connection: a send's row, a read receipt, a setting written meanwhile would join it and
be rolled back with a failing page.

## Options Considered

### Option A: Per notification (ADR-0032 as is)

**Pros:**
- The shortest windows; a failing notification costs nothing before it.

**Cons:**
- Several round trips per notification on remote storage: a catch-up of a few hundred notifications takes minutes.

### Option B: Per page for the MLS state and the cursor; data per event

**Pros:**
- Simple: no long `kupferdraht.db` transaction, so other writers are not affected.

**Cons:**
- Still one round trip per event on remote storage — about 5–6 s per page at 50 ms round-trip time.

### Option C: Per page for both databases, in ADR-0032's order

One MLS window and one `kupferdraht.db` transaction for the whole page, a savepoint per notification; then the MLS commit;
then the cursor. Other writers of `kupferdraht.db` wait for the page.

**Pros:**
- Three commits per page on every target — about 0.4 s per page at 50 ms round-trip time.
- The crash points of ADR-0032 stay as they are.

**Cons:**
- Other writers of `kupferdraht.db` wait for the page; the store needs a write gate, and notifications to the embedder
  must wait for the commit.

### Option D: Per page, the cursor in the data transaction

**Pros:**
- Two commits per page.

**Cons:**
- The cursor is committed before the MLS state — the unsafe order.

### Option E: Batch only in the browser

**Pros:**
- Native behaviour unchanged.

**Cons:**
- Two drain code paths.

## Decision

**Option C, on every target.**

1. **One window per page.** For a fetched page the drain takes the MLS write gate (ADR-0032 item 5), opens the
   `mls.db` write transaction and the `kupferdraht.db` page transaction, and dispatches every notification of the page. It
   commits `kupferdraht.db`, then `mls.db`, then advances the cursor past the page in a write of its own.
2. **A savepoint per notification,** in both databases. A notification that fails to persist, or that is deferred
   behind an own pending commit (ADR-0031), rolls back to its savepoint; everything before it commits, the cursor
   stops before it, and the drain stops or waits as today. A message that cannot be decrypted is skipped, as today,
   and does not fail the page. If a commit itself fails, the whole page rolls back and a later drain processes it
   again.
3. **Other writers wait for the page.** While a page transaction is open, a write to `kupferdraht.db` from outside the
   drain waits until the page is committed or rolled back; the drain's own writes join the page. Reads never wait.
   Lock order: the MLS write gate, then the page — nothing takes them the other way round.
4. **Notifications after the commit.** What the embedder hears about a page — new messages, timeline updates, device
   changes — is emitted after the page commits; a rolled-back part emits nothing. A replay therefore never tells the
   embedder something twice.
5. **Crash points as in ADR-0032.** Before the `kupferdraht.db` commit, nothing of the page is kept. After it, before the
   MLS commit, the page replays with its keys intact and persistence dedupes. After the MLS commit, before the cursor,
   the page replays, its MLS messages are skipped, its data is already stored.
6. **Push frames are pages of one.** A notification that arrives over a push transport is processed as a page of one.
7. **The page is the backend's page** (up to 100 notifications); Kupferdraht does not split it further.
8. **A capability of the store.** Grouping a page is part of the `Store` trait, with a default that groups nothing: a
   custom store keeps per-write commits and stays correct.

## Consequences

### Positive

- On remote storage a page costs three round trips: a catch-up is about forty times faster and uploads a hundredth of
  the data.
- Natively a page costs three fsyncs instead of several per notification.
- The crash safety of ADR-0032 is unchanged.
- The embedder never hears of data that is rolled back afterwards.

### Negative

- A send, and any other write to `kupferdraht.db`, waits for the page being processed: milliseconds natively, a few hundred
  milliseconds on remote storage.
- Every write method of the store takes the page's write gate — a missed method would let a write join a page, so the
  gate sits in the one place all writes pass.
- Notifications to the embedder arrive a page later than before.
- A read inside the window may see uncommitted data of the page (ADR-0032 already accepts this for MLS state).

### Neutral

- Idempotence per message id stays the stated invariant of every inbound handler.

## Implementation Notes

- The drain's per-notification window of Step 97e becomes a per-page window around the loop over the page; the MLS
  window gains savepoints (`SAVEPOINT` / `ROLLBACK TO` / `RELEASE` on the open transaction).
- `Store` gains `begin_page`, `savepoint`, `rollback_to_savepoint`, `commit_page` and `rollback_page`, with defaults
  that do nothing. `SqliteStore` opens `BEGIN IMMEDIATE`, turns the per-event transactions of Step 105 into savepoints
  while a page is open, and gates its write methods on an async page lock that the drain's task passes (a task-local
  marker, available on every target).
- The dispatch collects the notifications to the embedder in the page and emits them after `commit_page`.

## References

- Supersedes ADR-0032 item 1 (partial).
- Builds on ADR-0031 (one cursor-advancing dispatch per account), ADR-0032 items 2–5, ADR-0035 (remote storage).
- `docs/plan/108-remote-vfs-spike.md` (measurements).

## Revision History

- 2026-09-29: Created (Accepted — Markus): per page for both databases, with a write gate for `kupferdraht.db` and
  notifications after the commit
