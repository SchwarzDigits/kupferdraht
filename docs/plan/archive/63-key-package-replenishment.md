# Step 63 — Key-package pool replenishment

> **Status:** MERGED to `main` — `ad18682` (fast-forward, 2026-08-13).
> Scope as planned: count wrapper + `replenish_key_packages()` /
> `key_package_count()` (FFI-mirrored), V24 throttle stamp, sync-surface
> auto-check, CLI `key-packages [--replenish]`. Gate green (53
> binaries / 615 tests), live smoke 2026-08-13 against the self-hosted
> instance: real pool
> read (100/100), healthy-replenish no-op path, V24 migration applied
> to an existing DB. The smoke also caught a live protocol gap — the
> count endpoint **requires** `?ciphersuite=<decimal>` (400 without;
> fixed + query-param-pinned; `docs/wire/api-notes.md` updated — which
> had documented the parameter all along). The low-pool live drain
> stays pending until a claim source exists (blocked on the `create_dm`
> investigation, observations §66). Picked
> 2026-08-13 (Volker) from the backlog (`open.md` gap-B.7) as the
> highest-leverage reliability item for the now-standing self-hosted
> instance: long-lived devices deplete their pool silently, and the
> failure surfaces weeks later at the *peer* ("cannot invite user X"),
> not at the affected device.
>
> **Historical names:**
> - "Step 63" — proposed 2026-08-13.
>
> **Branch / commit prefix:** `step/63-*`, `feat(step-63): …`.

---

## Goal

Kupferdraht uploads `INITIAL_KEY_PACKAGE_COUNT` (100) key packages once at
device registration and never replenishes (`open.md` gap-B.7). Every
peer that adds this device to a conversation *claims* one package;
at zero, the device can no longer be added to new conversations —
existing rooms keep working, which is exactly why nobody notices in
time. This step adds a self-healing top-up.

## Contract

- `GET /v14/mls/key-packages/self/{client}/count` → `{"count": <n>}`
  (v14 swagger) — remaining unclaimed packages for this device.
- `POST /v14/mls/key-packages/self/{client}` — upload; same call the
  login pipeline uses (`LoginPipeline::upload_initial_key_packages`,
  generation via `MlsClient::generate_key_packages`).
- Thresholds: `TARGET = INITIAL_KEY_PACKAGE_COUNT (100)`,
  `MIN = TARGET / 2 (50)`. `count < MIN` → generate + upload
  `TARGET - count`; `count >= MIN` → no-op. (Wire clients use the
  same halving heuristic.)

## Public API

- `Client::replenish_key_packages() -> Result<u32, KupferdrahtError>` —
  explicit top-up; returns the number uploaded (`0` = pool healthy).
  FFI-mirrored in the same commit.
- **Auto-check in the sync surface**: a throttled
  `maybe_replenish_key_packages()` runs at the entry of `sync`,
  `sync_with_callback`, `sync_stream`, and `sync_once` — at most once
  per 24 h (stamp persisted in `sync_state`, new column via
  migration V24). Placing the hook at the entry points (not inside
  the polling iteration) means the Step-62 WebSocket transport is
  covered automatically once that stack merges — its loop variants
  route through the same entries. Failures warn and continue: a
  key-package problem must never break sync (warn-and-continue
  posture, ADR-0012 precedent).
- CLI: `key-packages` — show the server-side count + thresholds,
  `key-packages --replenish` to force a top-up. Also the diagnosis
  tool for the instance.

## TDD list (red phase)

1. `replenish_uploads_the_difference_when_pool_low` — count 40 →
   upload of exactly 60, returns 60.
2. `replenish_skips_upload_when_pool_at_min` — count 50 (boundary:
   `MIN` itself is healthy) → no upload, returns 0.
3. `sync_once_triggers_the_check_at_most_once_per_day` — two
   `sync_once` runs; the count endpoint is hit exactly once
   (throttle stamp persisted).
4. `count_endpoint_failure_does_not_break_sync` — count → 500;
   `sync_once` still returns `Ok` (warn-and-continue).

## Out of scope

- Multi-ciphersuite pools (gap-B.1/B.8 — needs the ciphersuite ADR).
- Deleting stale/expired packages server-side (`DELETE` exists;
  no consumer need yet).
- Reactive top-up on claim events (Wire sends no such event to the
  claimed device; polling the count is the only signal).

## Verification / smoke (phase 9)

Against the self-hosted instance: `kupferdraht-cli key-packages` (shows
live count), claim some packages by having the second test user
create conversations, watch the auto-top-up restore the pool on the
next sync session. Independent of the Step-62 ops flag.
