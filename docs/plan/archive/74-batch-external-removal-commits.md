# Step 74 — batch external-removal commits

> **Status:** merged to `main` as `7a376dd` (2026-08-24; red, green
> and live smoke all same day — 3 red + 1 preservation pin, CI
> green). Scope: signal-then-flush batching of external-removal
> commits (`sync/removal_flush.rs`, client-owned pending set, flush
> at drain end + per push frame), missing-references → debug,
> external-sender-capable `PeerGroup` fixture. TDD item 5
> turned out to be a no-op: the Step-28 unit tests pin
> `commit_external_removals` semantics, not dispatch timing. Fixes gap-B.25 as narrowed by the
> live re-probe (observations §69 + §70): the per-proposal auto-commit
> storm is real and verified in code; the "permanently wedged rooms /
> blocked joins" half of the original write-up did NOT reproduce —
> the 2026-08-21 drains converged and healed every established group,
> and `rooms-join-all` re-entered 7/9 old rooms cleanly on 2026-08-24
> (the two failures were one never-established epoch-0 conversation,
> `mls-missing-group-info`, and one already-joined).

## Problem (verified)

`decrypt_to_application` (sync/service.rs) drives
`Client::commit_pending_external_removal` **per received
`ProposalQueued` event**. openmls's `commit_to_pending_proposals`
references every *locally* queued proposal, but during a backlog
drain of N external Removes (one notification each), proposals
1..k<N produce a commit while N−k are still undelivered — Wire
rejects each with 400 `mls-commit-missing-references`. Verified
consequences:

- **N−1 guaranteed rejected uploads per backlog** — wasted round
  trips and warn-spam; every 400 also surfaces to consumers via
  `on_sync_error` (the TUI renders error notices for a
  self-resolving condition).
- **Convergence is real but fragile**: `clear_pending_commit` (openmls
  0.8.1, verified in source) resets only the group state — the
  proposal store survives, so the *last* proposal's commit references
  all N and succeeds. But if that final commit fails transiently
  (network blip) or the process stops mid-drain, nothing re-triggers:
  the cursor has advanced past the proposals, and the group sits with
  a queued backlog until unrelated room activity arrives.

Not in scope (did not reproduce): external-join blocking. Whether
Wire exempts external commits from the all-pending-referenced check
is unknown — in practice established members self-heal groups before
a joiner meets a backlog.

## Design

One mechanism, three consumers:

1. **Signal instead of commit.** `decrypt_to_application` no longer
   performs the commit; the `ProposalQueued` arm reports the room to
   the `SyncService`, which records it in a service-owned
   `pending_removal_commits` set (room_ids). Moves the network call
   out of the decrypt helper — SoC win on its own.
2. **Flush per dispatch cycle.** A `flush_removal_commits` step runs
   at the natural end of each cycle: end of `drain_pending` (REST —
   one commit per room per whole drain, storm gone) and after
   `dispatch_notification` in both push frame paths (events +
   legacy). Flush drives the existing
   `commit_pending_external_removal` per recorded room.
3. **Retry by not clearing.** A room leaves the set only on
   successful commit / `SelfRemoved` / `Nothing`. Failures stay for
   the next flush (next frame, next drain — in-memory only).
   `mls-commit-missing-references` is *expected* mid-backlog on the
   push path (frames arrive one by one) → log at debug, do NOT
   surface `on_sync_error`. Other errors keep today's error surface.

Residual (documented, accepted): a process restart loses the
in-memory set; the queued proposals persist in openmls storage and
commit on the next room activity (any inbound event for the room
re-signals; a send self-heals via the epoch-retry path). No
per-iteration storage sweep — scanning every cached group each poll
cycle is not worth the rare window.

## TDD list (phase 3)

Integration (`kupferdraht-core/tests/step_74_removal_batching.rs`,
PeerGroup fixture + httpmock):

1. **One commit per drain**: two external Remove proposals as two
   queued notifications in one drain → exactly ONE
   `/mls/commit-bundles` upload; both leaves gone after merge.
2. **Push-path burst tolerance**: two frames dispatched as separate
   cycles; first flush 400s `mls-commit-missing-references` →
   NO `on_sync_error` callback, room stays pending; second flush
   commits clean (two uploads total, group merged).
3. **Transient failure retries**: flush upload 500s → error surfaces
   via `on_sync_error`, room stays in the set; next cycle's flush
   succeeds.
4. **Self-removal unchanged**: batch containing a Remove of our own
   leaf → local discard + room dropped, no upload.
5. **Step-28 suite re-pinned**: existing tests asserting the
   per-event commit timing updated to the flush model (same
   observable end state).

## CLI

No new command — `sync` / `sync-stream` exercise the path. The step
is validated by the test suite + smoke.

## Live smoke

PASSED 2026-08-24. Seeded a real 2-proposal backlog (two throwaway
user-3 devices registered, joined into 3 shared rooms via external
commit, then deleted) and drained as user 1 — twice, for a direct
A/B: the **pre-74 binary** produced 6 commits, 4
`mls-commit-missing-references` rejections and 4 consumer-visible
warns for the same load; the **batched binary** produced exactly
**3 commits (one per room), 0 missing-references, 0 dispatch
warns**. Bonus coverage: one flush hit a genuine live 409
`mls-stale-message` (a concurrent auto-commit won the race,
backend-spec §10.5) — surfaced as a single clean error, the drain
survived, and the room converged via the winner's commit (roster
correct, probe send ok). The restart residual behaved as documented:
a fresh CLI process starts with an empty set and the foreign commit
had already consumed the queued proposals.

## Out of scope

- External-join behaviour against a live backlog (unproven, not
  reproducible on demand without wedging rooms deliberately).
- A persisted pending-commit journal (gap-B.23 family).
- `rooms-join-all` classification of never-established epoch-0
  conversations (`mls-missing-group-info` counts as "failed" today;
  cosmetic).
