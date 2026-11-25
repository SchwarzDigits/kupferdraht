# Step 173 — Key material renewed: the self-update per group and key packages before they expire

**Status: commissioned 2026-11-25** (Volker): both halves, the self-update per group and
the renewal of key packages. After Step 172. The gap it closes is post-compromise
security: material that is never renewed keeps a compromise alive.

## Problem

`self_update_commit` (`encryption/group_create.rs`) has one caller, the establishing
commit of a group with a single device (`client/rooms.rs`); no device ever renews its
leaf in a group it has been in for weeks. Key packages are replenished when few are
left (Step 63), not when they expire; whether expired ones are replaced, and whether
brig-rs still hands them out, is unmeasured.

## Scope

1. **Phase 1: the interval.** The interval Wire's clients use for the self-update, read
   in the webapp or in core-crypto with the source named; without evidence, 30 days.
   The interval is configurable (`ClientBuilder`). The key-package lifetime Kupferdraht
   sets, read in the code; whether brig-rs hands out expired packages, measured on the
   stack — if it does, that is a finding for the server and goes to the coordinator.
2. **The self-update per group.** Every device commits with an update path in every
   group where it has made no own update-path commit for the interval. Checked at
   start and on every sync, no background service; the commits are spread (not every
   group in one burst) and go through the existing serialisation of own commits
   (ADR-0031). Any own commit with an update path resets the clock, whatever its
   reason. Conflicts (`409`, an epoch moved by someone else) are handled as for every
   own commit: sync, then retry. The time of the last own update-path commit is
   persisted per group (a migration).
3. **"Renew now".** A CLI command for every group or one room, for tests and after a
   suspicion; the FFI mirror.
4. **Key packages.** Expired or soon-expiring key packages are replaced, not only
   topped up when few remain (`replenish_key_packages`, Step 63, grows the rule).
5. **Docs:** an ADR (the intervals, the spread, the reset rule) + its `rules/` file;
   `docs/backend-spec.md` (Step 166's paragraph in the present tense);
   `docs/library.md`, `docs/bindings.md`, `docs/cli.md`, `docs/ARCHITECTURE.md` §7
   (the column), `THREAT_MODEL.md` (PCS), `CHANGELOG.md`.

## Out of scope

- A background scheduler; renewal of the DB key (Step 161, parked).

## TDD sketch

- with a controlled clock: a group past the interval is due, one within it is not; an
  own commit of another reason resets it; three due groups are committed spread over
  three passes, not one; a `409` on the update triggers sync and one retry — red;
- `renew-now --id <room>` commits once and resets — red;
- a key package past its lifetime is replaced on the next check although the pool is
  full — red;
- the migration over the previous schema; `migration_immutability`.

Smoke on wire-dev: "renew now" in a group with a second device or user, who reads and
writes afterwards; a Wire webapp in the same group where possible; galley-rs checks the
commit signatures since the Rust server's task N — the self-updates pass.
