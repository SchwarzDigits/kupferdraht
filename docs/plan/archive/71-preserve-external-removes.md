# Step 71 — preserve external Removes across user commits (gap-B.20)

> **Status:** merged to `main` as `c53a8c1` (2026-08-17) — archived.
> Full gate green (suite run chunked after repeated session
> interruptions; `http_hardening` needs `KUPFERDRAHT_DEV_KEYSTORE`, see
> LEARNINGS §5.12); no smoke (race not reproducible on demand).
> Closes `open.md` gap-B.20 (review S9): the blanket
> `clear_pending_proposals` in `load_group_for_commit` — necessary
> for the live-confirmed `CannotRemoveSelf` case — also discards
> queued **backend external-Remove proposals**, silently dropping
> Step 28's commit obligation. Race window: an external Remove
> arrives via the drain, and `invite_members`/`kick_members` runs
> before the drain's auto-commit — this client then never commits
> the removal; the kicked device stays a member until another
> member commits.
>
> **Historical names:**
> - "Step 71" — proposed 2026-08-16.
>
> **Branch / commit prefix:** `step/71-*`, `feat(step-71): …`.

---

## Design

The blanket clear is over-broad. openmls commits cover pending
proposals by construction (RFC 9420 §12.4), so a queued
peer-targeting Remove can simply **ride the user's Add/Remove
commit** — one bundle, obligation kept, epoch advances once.
`load_group_for_commit` becomes selective:

- **Keep**: Remove proposals targeting *peer* leaves (the Step-28
  obligation class — Wire's external sender emits only Removes).
- **Clear**: Remove proposals targeting *our own* leaf (committing
  them trips `CannotRemoveSelf`, live-confirmed 2026-04-28; if the
  backend is removing us, the server re-issues at the next epoch —
  behavior unchanged from today), and every non-Remove proposal
  class (Update/Add — Kupferdraht's commits stay atomic in the
  caller-requested set; Wire's external sender never emits these).

Mechanics: `classify`-style scan over `pending_proposals()`
(mirroring `external_removal.rs`), then
`MlsGroup::remove_pending_proposal(storage, &ProposalRef)` per
dropped proposal — no blanket `clear_pending_proposals`.

Shared test fixtures (2-member group with external-senders
extension, forged external-sender Remove proposal) move from
`external_removal.rs`'s test module into
`encryption/test_support.rs` (`#[cfg(test)]`), consumed by both.

## Live-smoke honesty

The race window (proposal queued but the drain's auto-commit not
yet run, user commit in flight) is not reproducible on demand
against the instance — backend Removes fire on device/user
deletion, and a CLI `sync` both queues and auto-commits in one
drain. Coverage is therefore fixture-based (the same forged
external-sender proposals Step 28 pinned). The combined
Add+Remove commit shape is protocol-legal (a commit covers the
proposal list, RFC 9420 §12.4) and matches what Wire's own
core-crypto clients produce; a live acceptance run of the combined
shape stays an open follow-up if a reproducible trigger appears.
→ "no smoke (race not reproducible on demand)".

## TDD list (red)

Unit tests (the fix site is `pub(super)`), fixtures from
`test_support`:

1. `add_commit_carries_pending_peer_remove` — Alice+Bob, external
   Remove(Bob) queued via the real inbound path, Alice invites
   Dave: after merge Bob is **gone** and Dave is a member (today:
   Bob survives — the proposal was cleared).
2. `kick_commit_carries_pending_peer_remove` — Alice+Bob+Charlie,
   external Remove(Charlie) queued, Alice kicks Bob: after merge
   both Bob and Charlie are gone.
3. `add_commit_still_clears_pending_self_remove` — external
   Remove(Alice) queued, Alice invites Dave: commit builds (no
   `CannotRemoveSelf`), Dave in, Alice in, **and Bob in** (an
   over-aggressive selection that evicts the uninvolved peer must
   not stay green) — pins today's live-confirmed behavior (green
   from day 0, guards the selective-clear rewrite).
4. `kick_commit_still_clears_pending_self_remove` — same guard on
   the kick path (green from day 0): the protection lives in the
   *shared* `load_group_for_commit`, and this test keeps it there —
   a refactor moving the selection into the add path only would
   slip past the peer-remove tests. (Both guard tests: 2026-08-16
   review feedback; `peer_leaf_index` also now *enforces* its
   single-peer precondition instead of documenting it.)

## Out of scope

- Committing external removes as a *separate* pre-commit (two
  uploads inside `invite_members`) — more moving parts for the
  same obligation.
- gap-B.23 (pull crash window), gap-B.22 (upstream staged join).
