# Step 66 — Join consistency: stage external commits, real epochs

> **Status:** MERGED to `main` — `c4c5932` (+ group_id read order in
> `469f543`), fast-forward 2026-08-14. Originally implemented on
> `chore/review-triage` (2026-08-14),
> live-smoked 2026-08-14 against the self-hosted instance together
> with the whole review-fix series: resume + token refresh (auth
> fix), key-package pool read (63), a 42-notification real-queue
> drain (64 happy path), reply with a real wire quote plus the
> missing-parent hard error (65), and `rooms-join-all` regression
> (3 already_joined; the known groupinfo-404 room stays tolerated —
> it fails before the bundle build, untouched by the discard path,
> which itself is httpmock-pinned since a live upload failure cannot
> be forced). Implementation notes:
> 2/2 contract tests green; the step_3 join fixture was aligned to
> production (group id = the conversation's `group_id`) since the
> epoch now reads from the openmls group under exactly that id.
> Originally: in progress (2026-08-13,
> branch-per-step waived for the review-fix series). Design pass
> done: openmls's `ExternalCommitBuilder` persists **and merges**
> atomically in `finalize` — true upload-before-merge is impossible
> without an upstream `StagedExternalJoin` (tracked as gap-B.22).
> **Decision (Volker): Option A — discard-on-upload-failure**: a
> failed bundle upload immediately discards the just-merged local
> group, so `group_exists` is false again and `join_all_rooms`
> retries normally; the residual crash window (between finalize and
> discard) is covered by Step 64's tolerant `resync_room`. S16 reads
> the post-join epoch from the openmls group instead of cache+1.
> From the external code review (triaged in `open.md` §66; findings
> S2, S16, verified against the source at `f17abbd`).
>
> **Historical names:**
> - "Step 66" — proposed 2026-08-13.
>
> **Branch / commit prefix:** `step/66-*`, `feat(step-66): …`.

---

## Goal

1. **External commit: upload before merge (S2).** `join_room` merges
   the external-commit join into `mls.db` the moment the builder
   finalizes; the bundle upload comes after
   (`client/membership.rs:544` — the doc comment itself records the
   consequence: "a non-2xx response leaves local state ahead of the
   server"). `join_all_rooms` then sees `group_exists` and never
   retries. Bring the join in line with the add/remove-commit
   precedent (`upload_and_merge_commit_bundle`: upload first, merge
   on success). Design question for the red phase: openmls's
   external-join builder wants to write group state on finalize —
   either stage-and-discard on upload failure (discard_group +
   re-join on retry), or a pending-join marker that
   `join_all_rooms` treats as "not joined".
2. **Epoch from the group, not the cache (S16).**
   `membership.rs:547` persists `room.epoch() + 1` from the possibly
   stale rooms cache. Read the authoritative epoch from the openmls
   group state after the join instead; stale-detection stays
   reliable without a prior `sync_metadata`.

## Non-goals

- Changing the add/remove commit path (already staged correctly).
- Automatic retry loops for failed joins — `join_all_rooms` re-run
  stays the retry surface; it just must be able to *see* the failed
  join.

## TDD candidates

- join with upload-500: local group absent afterwards (or marked
  pending), `join_all_rooms` retries it, second attempt succeeds.
- joined room persists the epoch reported by the openmls group, not
  cache+1 (fixture: stale cached epoch).
