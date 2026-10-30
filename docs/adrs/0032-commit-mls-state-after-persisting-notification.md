# ADR-0032: Commit an inbound notification's MLS state only after its data is persisted

**Status:** Accepted (decision item 1 — one window per notification — superseded by ADR-0037)
**Date:** 2026-09-28
**Deciders:** Markus Troßbach

## Context

Receiving an MLS message changes two databases. `process_inbound_message`
advances the group state in `mls.db` — for an application message the
secret-tree ratchet, which deletes the message key (forward secrecy); for
a commit the epoch. Since Step 97c that is one transaction. The decrypted
content reaches `kupferdraht.db` only afterwards (timeline row, room state,
device set), and the sync cursor is advanced after that.

A crash between the two commits loses the message for good: the
notification is fetched again (the cursor never passed it), but its key
is gone and the ciphertext can no longer be decrypted (open.md §71 B9).
The two databases cannot share a SQLite transaction; they have separate
connections, keys and access layers (a synchronous MLS client behind a
short mutex, an async store).

Constraints from prior decisions: Step 97c runs every MLS operation in
one transaction and lets an operation join a transaction already open on
the connection; ADR-0031 lets only one cursor-advancing dispatch run per
account at a time.

## Options Considered

### Option A: An outbox in `mls.db`

Store the decrypted content in a table of `mls.db` inside the MLS
transaction; persist it to `kupferdraht.db`; delete the outbox row. A
redelivered notification is served from the outbox.

**Pros:**
- The MLS lock discipline stays as it is.

**Cons:**
- A second copy of every plaintext.
- Replay and cleanup logic; a table of our own in the openmls database.
- The redelivery must be matched to its outbox row.

### Option B: Persist first, commit the MLS state after (at-least-once)

Per notification, keep the MLS write transaction open while the
notification's effects are persisted to `kupferdraht.db`, then commit it, then
advance the cursor.

**Pros:**
- No plaintext copy, no replay store.
- Every crash point replays into a correct state (see Decision).
- Builds on persistence that is already idempotent per message id.

**Cons:**
- The `mls.db` write lock is held across async `kupferdraht.db` writes.
- No other MLS write may run in that window, so sends wait while a
  notification is processed; a stricter lock discipline.

### Option C: One SQLite transaction over both databases (`ATTACH`)

**Pros:**
- True atomicity.

**Cons:**
- Couples both databases and their keys in one connection.
- Bypasses the store layer; fragile across the two schemas' migrations.

## Decision

**Option B.**

1. **One window per notification.** A notification's MLS changes and the
   persistence of everything it produces happen inside one open `mls.db`
   write transaction: begin → dispatch (decrypt, persist) → commit the MLS
   transaction → advance the cursor. Not per page: the window stays
   milliseconds long and the cursor keeps advancing per notification.
2. **At-least-once, idempotent.** Persistence is idempotent per message
   id and a consumer callback fires only for a fresh effect — a
   requirement on every inbound handler, pinned by tests. On a replay, a
   message whose MLS processing is already committed — an older epoch,
   or a generation whose key was consumed — is skipped silently.
3. **Crash points.** Before the MLS commit: the notification replays with
   its keys intact, persistence dedupes. After the MLS commit, before the
   cursor: the notification replays, its MLS messages are skipped, its
   data is already stored.
4. **Failure.** A `kupferdraht.db` failure while persisting rolls the
   notification's MLS changes back and leaves the cursor before it; the
   drain stops and retries later. A message that cannot be decrypted is
   skipped as today.
5. **One MLS writer per window.** No other MLS write runs inside the
   window — a send's ratchet advance must never be rolled back with a
   notification. Every MLS write outside inbound dispatch (sends, own
   commit flows, key packages, join / leave / discard) takes an async MLS
   write gate that the drain holds per notification; reads need no gate.

## Consequences

### Positive

- No message is lost to a crash between decrypting and persisting.
- No second plaintext copy, no replay store.
- Crash recovery is plain redelivery; nothing to clean up.

### Negative

- A send waits while a notification is processed (milliseconds; one
  notification at a time).
- The `mls.db` write lock is held across async `kupferdraht.db` writes; an MLS
  operation of another process waits (`busy_timeout`).
- A read that runs inside a window may see MLS state that is still
  uncommitted.
- Every MLS write outside dispatch must take the gate — a missed call
  site is a correctness bug, so it needs a code-level guard.

### Neutral

- The per-message-id idempotence becomes a stated invariant of every
  inbound handler.
- The push paths use the same per-notification window.

## Implementation Notes

- `KupferdrahtProvider`: begin / commit / rollback of a notification window
  on the `mls.db` connection; Step 97c's operations join it; a guard
  rolls back on drop.
- `Client`: an async MLS write gate (`tokio::sync::Mutex`), taken by the
  drain per notification and by the write paths listed in item 5.
- `process_inbound_message`: a consumed generation → a "stale" outcome
  like an older epoch.

## References

- Builds on ADR-0031 — one cursor-advancing dispatch per account at a
  time; the window runs inside the drain lock.
- Related: ADR-0024 (`busy_timeout` makes another process's MLS
  operation wait), ADR-0030 and ADR-0017 (what advances the cursor).
- Step 97c (`docs/plan/97-durable-send-and-receive.md`): one transaction
  per MLS operation, joined by nested operations.
- open.md §71 B9.

## Revision History

- 2026-09-28: Created (Accepted — Markus; one window per notification,
  reads without the gate)
- 2026-09-29: Decision item 1 (one window per notification, not per page)
  superseded by ADR-0037 "Commit a drained page, not each notification";
  the other decisions stand unchanged and now apply per page
