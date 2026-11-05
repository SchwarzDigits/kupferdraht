# Step 142 — Atomic openmls migrations in `mls.db`

**Status: commissioned 2026-11-05** (transferred from open.md gap-B.27,
§82). Volker's order: after the Step 141 archive, before 130b; ships in
v0.2.1. The openmls side is done: the fix is filed upstream
(openmls/openmls#2322, PR openmls/openmls#2323) and backported to the
fork's branch `schwarzdigits/openmls-0.9` as `597a098eb`, tag
`openmls-v0.9.0-schwarzdigits.2` (signed, pushed 2026-11-05). This step
moves Kupferdraht onto that tag.

## Problem

1. **`mls.db` keeps the window Step 87 closed for `kupferdraht.db`.**
   `openmls_sqlite_storage` 0.3.0 builds its refinery runner without
   `set_grouped` (`storage_provider.rs`, `run_migrations`), and refinery's
   rusqlite driver commits each `execute` on its own: a migration's SQL,
   then its row in `openmls_sqlite_storage_migrations`. A process that
   dies between the two leaves the migration applied but unrecorded, and
   the next `KupferdrahtProvider::open` applies it again.
2. **Applied again, V3–V6 fail for good.** Measured 2026-11-05 with a
   trigger that aborts one history-row insert, then two more runs: V3–V6
   fail on `CREATE TABLE` ("already exists") on every later open; V2, a
   copy-and-swap, survives. The rows stay on disk, but the store cannot
   be opened without hand repair — today's only way out is to discard
   the device's MLS state. open.md §82 said "V2+"; that was read, not
   measured.
3. **When the window opens:** on the first open of every new `mls.db`
   (each login on a new device runs V1–V6) and on any later bump that
   adds migrations. The window is milliseconds, but a mobile OS kills
   apps at arbitrary points.

## Scope

1. **The pins.** `kupferdraht-core/Cargo.toml`: `openmls_sqlite_storage`,
   `openmls_rust_crypto` and `openmls_basic_credential` move from
   `openmls-v0.9.0-schwarzdigits.1` to `.2` together (one tag per site,
   ADR-0042); `Cargo.lock` re-resolves the three to `597a098eb`. The two
   crypto crates are unchanged between the tags; only `sqlite_storage`
   differs (the grouped runner, its two tests, its README).
2. **No Kupferdraht code change.** `KupferdrahtProvider::open` already
   calls `run_migrations`; the fork does the rest. The browser build
   gets the same fix through the same crate.
3. **No CLI command.** No public API changes; every login and every
   open of an account runs the path (Phase 6 noted, not skipped).
4. Docs in the same change:
   - `docs/adrs/0042-…` fork table: tag `.2` and the grouped runner in
     the openmls row's change column; `rules/0042-…` names no tag and
     stays as it is (checked on the step).
   - `open.md` §82: the tag in its first paragraph; the gap-B.27
     paragraph leaves.
   - `docs/ARCHITECTURE.md` §7: the atomic migration run covers
     `mls.db` too (Step 142), not only `kupferdraht.db` (Step 87).
   - `docs/observations.md` §78: its closing note gains the `mls.db`
     resolution.
   - `CHANGELOG.md` `[Unreleased]` under Fixed, consumer-facing.

## Out of scope

- **Repairing a store the old runner already broke.** A `mls.db` with a
  migration applied but unrecorded fails on the next open with the fix
  too (the grouped run re-applies the same migration). No such store is
  known; repair would mean guessing which migration ran. The way out
  stays discarding the device's MLS state.
- Upstream's V7 and V8 (on `main`, unreleased): they arrive with the
  next openmls release, on a new fork branch from that release tag,
  which then carries upstream's own fix if #2323 is merged.
- The other open points of §82 (branch policy, the next bump).

## TDD sketch

- **Pin (red on `.1`):** `tests/step_90_sqlite3mc_engine.rs`,
  `OPENMLS_STORAGE_SOURCE` names tag `openmls-v0.9.0-schwarzdigits.2`;
  red until the manifest and the lockfile move.
- **Behaviour (red on `.1`):** a unit test beside
  `KupferdrahtProvider::open` (`encryption/provider.rs` or a test file
  of its own): a `mls.db` in a temp dir gets refinery's history table
  `openmls_sqlite_storage_migrations` and a trigger that aborts V3's
  history-row insert, the state a crash leaves; `open` fails; reopened,
  V3's table (`vc_emulation_group_secrets`) does not exist; with the
  trigger dropped, `open` succeeds and V1–V6 are recorded. On `.1` the
  table survives the failed run and the next open fails "already
  exists".
- The existing storage suites stay green (`storage_provider`,
  `whole_db_encryption`, `storage_dyn_store`, `storage_default_config`).
  A pre-check on 2026-11-05 ran `kupferdraht-core`'s whole suite against
  the fork fix through a temporary `--config patch` (not committed): all
  green except three tests that read the lockfile's source or call
  `cargo metadata --locked` themselves, which a patch cannot satisfy and
  the real tag will.
- **Gate:** the five, plus `cargo check` and `cargo clippy` for
  `wasm32-unknown-unknown` — the crate is compiled into the browser
  build too, though no wasm-only module changes.
- **Smoke:** `kupferdraht-smoke.sh --throwaway` on the wire-server-rs
  stack, 18/18: each throwaway login opens a fresh `mls.db` and runs
  V1–V6 under the grouped runner (5.79: a change under every request
  runs the whole smoke).
