# ADR-0031: Serialise the drain per account and defer, not heal, around own pending commits

**Status:** Accepted
**Date:** 2026-09-28
**Deciders:** Markus Troßbach

## Context

Two gaps on the receive path lead to the out-of-order healing
(`recover_room` → discard + external rejoin, open.md §71 H4), which
costs epoch churn and loses the message that triggered it:

1. **Own commits, one process.** A commit is staged, the MLS lock is
   released for the upload, and the merge follows only after the HTTP
   response. A message from the new epoch that the sync loop receives
   in between meets the old local epoch and is healed around.
2. **Two processes on one account** — supported since ADR-0024 (e.g. the
   CLI next to an app, or two `Client`s). Both drain from the same
   persisted cursor and process the same MLS messages; the second meets
   an older epoch or a consumed message key. The same happens to an old,
   redelivered commit.

Since Step 97c every MLS operation is one SQLite transaction, but a
drain (fetch → decrypt → persist → cursor) spans many. ADR-0024's
`kupferdraht.lock` is fail-fast and is held across `POST /access`. The
browser runs one instance per origin (ADR-0023). Plan D6
(`docs/plan/97-durable-send-and-receive.md`) proposed a lock around
stage → upload → merge.

Constraints from prior decisions: ADR-0024 (the advisory file-lock
primitive, `kupferdraht.lock` stays fail-fast), ADR-0023 (one browser
instance per origin), ADR-0030 and ADR-0017 (what advances the cursor).

## Options Considered

### Option A: One lock around every multi-step MLS flow (plan D6)

**Pros:**
- One rule for all MLS work.

**Cons:**
- Held across the upload — every other MLS operation and drain of the
  account waits a network round trip.
- Does not stop two drains from processing the same notification.

### Option B: Reuse `kupferdraht.lock` for the drain

**Pros:**
- One lock file, existing code.

**Cons:**
- Fail-fast by contract; held across `POST /access`, so a drain queues
  behind a token refresh; changes what its callers rely on.

### Option C: A lease row in a database

**Pros:**
- No extra file.

**Cons:**
- Stays behind after a crash until it expires; needs an expiry.

### Option D: A held SQLite write transaction as the lock

**Cons:**
- Blocks the process's own per-operation MLS transactions (Step 97c) —
  it would wait on itself.

### Option E: A short-held drain lock plus deferring around own pending commits

**Pros:**
- The kernel releases a file lock when the process dies.
- Held only for local work, never across the network.
- Fixes both gaps and H4's redelivery case.

**Cons:**
- Bounded waiting instead of fail-fast.
- A page is refetched when the other process advanced the cursor.
- A deferred message stalls the drain until the commit is merged.

## Decision

**Option E.**

1. **One cursor-advancing dispatch per account at a time.** A separate
   advisory lock file `base/accounts/<id>/sync.lock` (`File::try_lock`,
   the ADR-0024 primitive) guards every dispatch that advances the
   cursor: a REST drain page and a `/v14/events` frame. The page is
   fetched without the lock; under the lock the persisted cursor is
   compared with the page's `since` — moved → release and refetch;
   unchanged → decrypt, persist, advance, release. Never held across
   network I/O.
2. **Bounded wait, not fail-fast.** A held `sync.lock` is retried for a
   bounded time, then a retryable error. In process an async mutex per
   account sits in front of it; wasm has the async mutex only (ADR-0023).
3. **Lock order.** A holder of `sync.lock` never takes `kupferdraht.lock`;
   everything that needs the network (token refresh, healing rejoin,
   removal flush, key-package upload) runs outside it.
4. **Defer, don't heal, around own pending commits.** A message from the
   next epoch for a group with an own pending commit is not healed. If
   this process's commit flow for that group is in flight (stage →
   upload → merge/clear, under a per-group async lock), the drain waits
   for it and retries the message. Otherwise the drain stops before the
   message (the cursor stays) and retries on the next drain. A pending
   commit still there after a bound is abandoned: cleared, then healed
   as today.
5. **Old epochs are dropped.** A message from an epoch older than the
   group's is logged and skipped, never healed.

`kupferdraht.lock` keeps ADR-0024 unchanged. The wait bound and the
abandonment bound are constants in code, not part of this decision.

## Consequences

### Positive

- Two processes never process the same notification.
- An own commit's upload window and a redelivered old message no longer
  trigger a rejoin.
- A crashed process leaves no lock; `sync.lock` is held for local work
  only.

### Negative

- A second lock posture (bounded wait) next to ADR-0024's fail-fast.
- A page is refetched when the other process advanced the cursor.
- A deferred message stalls the whole drain (the cursor is linear) until
  the commit is merged, at worst until the abandonment bound.
- The abandonment bound is a heuristic; a genuinely missed commit is
  still healed as today.

### Neutral

- wasm is unchanged in effect (one instance per origin).
- Resolving an abandoned pending commit against the server's GroupInfo
  instead of healing is a later step.
- A notification extension (a second process with a time budget) needs
  more than this — a receive-only mode among others; not decided here
  (open.md).

## Implementation Notes

- `sync/service.rs::drain_pending`: page loop fetch → lock → cursor
  check → dispatch → release; `end_dispatch_cycle` (network) outside.
- `storage/lock.rs`: a waiting acquire next to `try_acquire`.
- `client/membership.rs::upload_and_merge_commit_bundle` and its
  callers: stage → upload → merge/clear under the group's async lock.

## References

- Builds on ADR-0024 — the advisory file-lock primitive; `kupferdraht.lock`
  and its fail-fast posture stay as decided there.
- Constrained by ADR-0023 — one browser instance per origin, so wasm
  needs no cross-process lock.
- Related: ADR-0030 (only the drain advances the cursor on `/await`),
  ADR-0017 (`/v14/events` frames advance the cursor before the ack).
- Replaces plan D6 of `docs/plan/97-durable-send-and-receive.md`;
  open.md §71 B8 (serialisation) and H4 (healing too broad, in part).

## Revision History

- 2026-09-28: Created (Accepted — Markus; scoped to the gaps that exist
  without a notification extension)
