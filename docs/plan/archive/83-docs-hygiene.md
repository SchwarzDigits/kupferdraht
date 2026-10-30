# Step 83 — Docs-hygiene chore (cli.md gaps, closure markers, stale ADR ref)

**Status: MERGED 2026-09-24** — commit `141d2bc` (fast-forwarded to `main`); built on `step/83-docs-hygiene` in the main checkout. Originally commissioned 2026-09-10 (transferred from open.md
old-§63, gap-B.18, def-D.1's comment half). Chore-class: mostly
docs, one one-line code comment; gate yes, no smoke.

## Worklist

1. **`docs/cli.md`: `knock` + `location` sections** (old open.md
   §63, found 2026-07-16): the Step-26 commands were never
   documented — write both sections in the established command
   format (invocation, flags, sample output).
2. **`docs/observations.md` closure markers** (gap-B.18): the file
   has no way to see which observations are superseded by later
   findings or shipped fixes. Adopt the open.md convention
   (strikethrough title + one-line pointer) for the clearly
   superseded entries — content stays (it is a findings log),
   only the marker is added. First pass: walk §1–§73 and mark the
   obviously closed ones (e.g. storm findings fixed by Step 74,
   push findings resolved by ADR-0019).
3. **def-D.1 comment half**: `wire/http/feature_configs.rs` carries
   `TODO(ADR-0013)` meaning "ciphersuite negotiation ADR" — but
   ADR-0013 is the SQLCipher ADR. Reword the comment to
   "TODO(gap-B.1: ciphersuite ADR, number TBD)". The actual
   ciphersuite ADR stays with gap-B.1 (untouched).

4. **`docs/wire/message-types.md`: the full variant catalogue**
   (added 2026-09-24): a never-landed July commit (recovered from a
   scratch worktree during housekeeping) rewrote the catalogue as a
   complete, field-number-ordered table with a per-variant
   out-of-scope verdict. Ported onto the current file, re-verified
   against `wireapp/generic-message-proto` master (new since July:
   `Multipart = 27`; `UnknownStrategy` values listed), Ephemeral
   (Step 89) and the three Kupferdraht extensions folded in; the old
   "Reserved" list is subsumed by the table. README "What it does"
   gets the one-paragraph pointer from that commit.

## Out of scope

- The broader `chore/docs-catchup` workstream (Steps 17/44–59
  doc-home audit) — separate, larger effort.

## Retrospective (2026-09-24)

- **Item 4 was not in the plan** — it surfaced because housekeeping
  swept old scratch worktrees instead of deleting them blind: a
  finished July commit (`claude/kupferdraht-tasks-list-sharing-e9c636`)
  and a dirty DELIVERED doc fix were both sitting there, both correct,
  both never landed. Learning: an isolation worktree that outlives
  its session is a place where finished work goes to die; sweep them
  by content, not by age.
- **Re-verification paid off**: the July catalogue was already stale
  (Wire added `Multipart = 27` in between). A catalogue that names a
  cross-check date is only honest if the date moves with every port.
- **Closure markers are judgement calls**: nine observations were
  struck through, two candidates deliberately left open (§64 — Step 86
  is in flight; §72 — the ADR-0018 non-admin fold hole is recorded but
  not verified fixed). The convention line at the top of
  `observations.md` says what a marker means so nobody reads a strike
  as "text deleted".
- Simplified: no test pins a doc; the gate is the compile of the one
  reworded comment plus the review. Nothing hacked, no `TODO` added.
