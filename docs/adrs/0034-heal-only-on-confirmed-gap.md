# ADR-0034: Heal a room only on a confirmed gap, and leave a group only on the backend's word

**Status:** Accepted
**Date:** 2026-09-29
**Deciders:** Markus Troßbach

## Context

Kupferdraht "heals" a room by discarding its local MLS group and rejoining by
external commit. Healing is destructive: the external commit re-keys the
group for every member, and messages still in flight for the old epoch
can no longer be decrypted by this client. It is right when the local
group has fallen behind — a commit was missed — or is gone.

Three triggers heal or drop a group today on weaker evidence. (1) An
inbound message from a later epoch than the local group — unless it
follows an own pending commit (ADR-0031) — is taken as a missed commit.
But the epoch of a message the client cannot decrypt is not
authenticated: a malformed or forged message with a high epoch forces a
rejoin. (2) A storage error while processing an MLS message heals the
room. That signal meant a half-written merge before every MLS operation
became one transaction (Step 97c); since then it means a transient I/O
or lock failure, and ADR-0032 already defines how such a failure is
retried. (3) A Remove proposal targeting our own leaf, from any sender,
discards the local group — although in RFC 9420 a proposal removes
nobody until a commit applies it.

Constraints from prior decisions: ADR-0031 skips older-epoch messages
and defers next-epoch messages behind an own pending commit; ADR-0032
processes each notification at least once inside one MLS window that
rolls back on failure; ADR-0004 makes the backend the group's external
sender. The backend, as delivery service, orders commits and knows each
conversation's current epoch. It rejects a commit that does not
reference every pending proposal.

## Options Considered

### Option A — keep healing on every future-epoch message (status quo)

- Pro: no extra request; a real gap heals at once.
- Con: an unauthenticated epoch number decides a destructive action.

### Option B — confirm the gap with the delivery service before healing

- Pro: a real gap still heals; a message with a bogus epoch is skipped;
  one small request, only when a gap is suspected.
- Con: one extra round trip per suspected gap; a misbehaving backend can
  still report a gap (it can deny service anyway).

### Option C — buffer next-epoch messages until their commit arrives, heal only on a persistent gap

- Pro: tolerates reordering between a commit and the messages after it.
- Con: a buffer, its bounds and its expiry — much more machinery; the
  notification queue already delivers in order, so it gains little.

## Decision

**Option B, plus two narrowings.**

1. **Future epoch:** before healing a room for a message from a later
   epoch, Kupferdraht asks the backend for the conversation's current epoch
   and heals only when it is ahead of the local group. Otherwise the
   message is skipped as undecryptable — logged and reported as a sync
   error. One lookup per room per drain.
2. **Storage errors do not heal.** A storage failure while processing an
   MLS message aborts the notification like a failed persist (ADR-0032):
   its MLS changes roll back, the cursor stays, the next drain retries.
   A missing local group remains a reason to heal.
3. **Only the backend's Remove drops our group.** A Remove proposal
   targeting our own leaf discards the local group only when its sender
   is the group's external sender. A member's proposal is only a
   proposal; removal takes effect when a commit applies it. Committing
   all pending proposals when a peer's Remove is queued stays as is.

## Consequences

**Positive**

- A malformed or forged future-epoch message no longer forces a rejoin
  and re-keys nobody.
- A transient storage failure no longer costs the group; the message is
  retried instead of lost.
- A member cannot make Kupferdraht drop a group.

**Negative**

- One extra request when a gap is suspected; a real gap heals one round
  trip later.
- A persistently failing `mls.db` now shows as repeated sync errors on
  the same notification instead of a silent rejoin.
- A misbehaving backend can still force rejoins (out of scope: it can
  deny service in other ways).

**Neutral**

- Older-epoch skips and own-commit deferral (ADR-0031) are unchanged.
- The self-removal path for the backend's Remove is unchanged.

## Implementation Notes

- The current epoch comes from the conversation record
  (`POST /v14/conversations/list` for the one id), not from the rooms
  cache, which can be stale.
- An `mls-message-add` whose processing fails with `Storage` becomes a
  persist-class error (`EventError::Persist`); `MlsGroupMissing` /
  `InvalidInput` keep queuing recovery.
- The self-removal check reads the queued proposal's sender
  (`Sender::External`).

## References

- Builds on ADR-0031 (epoch triage) and ADR-0032 (at-least-once window).
- Related: ADR-0004 (the backend as external sender).
- `open.md` §71 H4; `docs/plan/106-narrow-healing.md`.

## Revision History

- 2026-09-29: Created (Accepted — Markus)
