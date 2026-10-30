# Step 87 — Atomic schema migrations (grouped refinery runs)

**Status: MERGED 2026-09-24** — commit `60ee3d1`, merged to `main` as
`f72bb7b`; built on branch `step/87-atomic-migrations` (worktree
instance, Phase-4 hard stop waived by Volker). Built as
planned: `set_grouped(true)` in the kupferdraht.db runner's builder chain
(`storage/schema.rs`), pinned by three `#[cfg(test)]` fault-injection
tests next to `run_migrations` (see "Implementation notes"). No smoke
(refactor — no backend surface; the fault-injection test is the
evidence). mls.db residual open: open.md gap-B.27, upstream issue
draft below. Corrected count: **25 of 27** migrations are re-apply-
fatal (V27, added by Step 89 after this plan was written, is an
unconditional `ALTER` + `CREATE INDEX`).

Commissioned 2026-09-15 (core-crypto-audit follow-up:
wireapp/core-crypto PR #2580 fixes the identical class — "failed
meta migrations only retry when DB is in the correct state" — by
making migration + version bump share one transaction; Kupferdraht's
instance is sharper because our migrations are not re-apply-safe).

## Problem (verified 2026-09-15)

refinery's rusqlite driver opens **one transaction per `execute`
call** (`refinery-core-0.9.1/src/drivers/rusqlite.rs:33ff` —
`self.transaction()` … `commit()` inside `execute`). In the
non-grouped default, `migrate()` sends each migration as TWO
separate executes — the migration SQL, then the
`refinery_schema_history` insert (`src/traits/sync.rs:86-100`,
the `i % 2` loop). Two separate commits per migration.

Both of our runners use that default:

- `kupferdraht-core/src/storage/schema.rs:19` — kupferdraht.db, V1–V26.
- `openmls_sqlite_storage 0.3.0` (`src/storage_provider.rs:56,65`)
  — mls.db; not configurable from outside.

A crash in the window between the two commits leaves the migration
applied with no history row. The next open re-applies it — and
**nearly all of our migration files are re-apply-fatal** (`CREATE
TABLE` without `IF NOT EXISTS`, unconditional `ALTER TABLE
ADD/DROP/RENAME COLUMN`, `DROP TABLE`): 24 of 26, including V1.
The exceptions (corrected per review 2026-09-15 — the first count
over-matched `CREATE TABLE IF NOT EXISTS`): V17 and V18 are fully
`IF NOT EXISTS`; V20 mixes a harmless `DROP TABLE IF EXISTS` with
fatal unconditional ALTERs. openmls_sqlite_storage: V1 is
`IF NOT EXISTS` (safe), V2+ are not. The consequence is unchanged:
"table already exists" / "duplicate column" on EVERY subsequent
open — the database is permanently un-openable, with no self-heal.
The window is milliseconds and only exists on the first open after
a schema update; the consequence is a brick.

## Scope

1. **`set_grouped(true)` on our runner** (`storage/schema.rs`):
   every pending migration plus its history rows in ONE
   transaction (SQLite DDL is transactional). One line; the
   version-skew window disappears for kupferdraht.db.
2. **Red test — atomicity pinned by fault injection**: a
   test-local `Runner::new(&[migration_ok, migration_failing])`
   against a scratch connection; after the failing run, assert the
   history table EXISTS BUT IS EMPTY (refinery creates it in its
   own preceding execute — absence would be the wrong assertion)
   and migration 1's table does NOT exist (grouped = full
   rollback). Today (non-grouped) migration 1 survives with its
   history row — red. The proxy is not literally "crash between
   SQL and history of one migration", but grouped puts both in one
   transaction, so the same window closes (review-confirmed).
3. **Preservation**: fresh-DB full chain still applies (existing
   suite), `migration_immutability` SHA pins untouched — grouping
   changes transaction boundaries, not migration content.
4. **Upstream issue against openmls (`openmls_sqlite_storage`)**:
   same fix (grouped runner or caller-supplied transaction, the
   core-crypto #2580 pattern); draft the text in this step. Until
   it ships, **mls.db keeps the window** — documented residual
   (the group-state blast radius is smaller: mls.db can be
   discarded and rebuilt via external-commit rejoin, kupferdraht.db
   cannot).

## Out of scope

- Healing already-bricked databases (a DB stuck in the half state
  predates the fix; recovery = the account-recreate path). If it
  ever happens in the field, a targeted repair is its own step.
- wasm: IndexedDB stores don't use refinery (snapshot model, no
  SQL migrations).

## Fix sketch

`storage/schema.rs:19`: `runner().set_grouped(true).run(conn)`.
**API trap (verified, refinery-core-0.9.1/src/runner.rs:267):**
`set_grouped(self, …) -> Runner` takes self BY VALUE — the call
must stay in the builder chain; `let mut r = …;
r.set_grouped(true);` discards the return value and silently keeps
the non-grouped default. The atomicity red test is the guard
against exactly that regression. For the upstream text: openmls's
runner setup uses `&mut self` only for `set_migration_table_name`
— the grouped flag needs the by-value chain there too.

## Implementation notes (2026-09-24)

- **Where the tests live and why.** The pins are a `#[cfg(test)]`
  module in `storage/schema.rs`, not a `tests/*.rs` file: they call
  `run_migrations` (crate-private, the seam where the fix lives) on an
  in-memory `rusqlite::Connection` with **no cipher pragmas**. That
  makes them engine-agnostic — Step 90 (SQLite3MC swap) can change the
  key pragmas without touching them — and keeps the diff to one file.
  A `tests/`-level variant through `SqliteStore::open` would have
  needed a raw keyed reopen for the pre-seed (the
  `step_1c_mls_storage_path.rs` pattern), i.e. a second pragma site
  for Step 90 to rewrite.
- **Fault model.** V2's table (`devices`) is pre-created, so V2 fails
  after V1 succeeded. `interrupted_run_leaves_no_partial_state` is the
  red test — non-grouped it fails with `refinery_schema_history` = 1
  row (V1 committed on its own); grouped it passes with 0 rows and no
  `session` table. `failed_run_can_be_retried_once_the_fault_is_gone`
  and `grouped_runner_rolls_back_the_whole_batch` (the plan's
  test-local `Runner::new(&[ok, failing])`) are green before and after
  the fix — preservation pin and refinery-semantics pin respectively,
  labelled as such in the module.
- **Why no literal crash-window test.** The window sits between two
  commits inside refinery's `migrate` loop; without hooking the driver
  no test can stop there. The failing-V2 proxy shares the transaction
  boundary the fix changes (review-confirmed in the plan).
- **Preservation** held without changes: fresh-DB full chain,
  idempotent reopen, `migration_immutability` pins untouched.
- **CLI:** no new command — the change has no API surface; every
  `kupferdraht-cli` command that opens `kupferdraht.db` runs the grouped
  migration. **FFI parity:** no `pub` consumer-facing symbol changed.

## Upstream issue draft — `openmls_sqlite_storage` (verified 2026-09-24)

Verified before drafting: `openmls/openmls` `main`
`sqlite_storage/src/storage_provider.rs` still builds the runner
non-grouped (`run_migrations`: `set_migration_table_name` only; the
deprecated `initialize` likewise); crates.io newest release 0.3.0
(2026-08-25); no existing issue matches "refinery migration
transaction". Text to file (English), Volker's call:

> **Title:** sqlite_storage: run the refinery migrations grouped — a
> crash between a migration's SQL and its history row bricks the store
>
> `SqliteStorageProvider::run_migrations` (and the deprecated
> `initialize`) run `migrations::runner()` with refinery's default
> `grouped = false`. refinery's rusqlite driver opens **one transaction
> per `execute` call** (refinery-core 0.9.1
> `src/drivers/rusqlite.rs:33-47`), and non-grouped `migrate` sends
> each migration as **two** executes — the migration SQL, then the
> `openmls_sqlite_storage_migrations` insert (`src/traits/sync.rs:85-99`).
> Two separate commits per migration.
>
> If the process dies between those two commits (crash, OOM kill, power
> loss — the window is milliseconds, but it exists on every first open
> after a crate upgrade that adds a migration), the migration is applied
> but not recorded. The next `run_migrations` re-applies it, and V2+ are
> not re-apply-safe (`CREATE TABLE openmls_group_data_new`, `DROP
> TABLE`, `ALTER TABLE … RENAME`) — `table … already exists` on every
> subsequent open, with no self-heal. The store is permanently
> un-openable; the only recovery is to discard it.
>
> **Fix:** group the run so every pending migration and its history
> row share one transaction — SQLite DDL is transactional, so refinery's
> `set_grouped` caveat (MySQL) does not apply:
>
> ```rust
> pub fn run_migrations(&mut self) -> Result<(), refinery::Error> {
>     // `set_grouped` takes `self` by value (unlike
>     // `set_migration_table_name(&mut self)`): keep it in the chain.
>     let mut runner = migrations::runner().set_grouped(true);
>     runner.set_migration_table_name("openmls_sqlite_storage_migrations");
>     runner.run(self.connection.borrow_mut())?;
>     Ok(())
> }
> ```
>
> The alternative is to accept a caller-supplied transaction (the
> pattern wireapp/core-crypto adopted in PR #2580 for the same defect
> class in their own migrations).
>
> **Repro (test-local):** `Runner::new(&[ok, failing])` on an in-memory
> connection, where `failing` duplicates `ok`'s table. Non-grouped, the
> run fails and leaves `ok`'s table plus its history row behind; with
> `.set_grouped(true)` the history table exists but is empty and the
> table is gone — full rollback. A downstream client (Kupferdraht) pinned
> exactly this for its own refinery runner and would like to drop the
> residual on the openmls store.

