# Step 142 — openmls 0.9.2 and atomic openmls migrations in `mls.db`

**Status: commissioned 2026-11-05** (transferred from open.md gap-B.27,
§82), **widened the same day to openmls 0.9.2**: the fork's branch
`schwarzdigits/openmls-0.9` merged upstream's `v0.9.2` (`2142e61e7`,
`9a228e8e9`) on top of the grouped-runner backport (`597a098eb`, filed
upstream as openmls/openmls#2322, PR openmls/openmls#2323) and the p256
0.14 providers; tag `openmls-v0.9.2-schwarzdigits.1` (signed,
2026-11-05). This step moves Kupferdraht onto that tag and onto
crates.io's `openmls` 0.9.2. Next after the commissioning chore, before
130b; ships in v0.2.1 with the features, no early release — the
embedder is not in production yet (Volker, 2026-11-05).

## Problem

1. **Kupferdraht ships openmls 0.9.0, which three advisories affect.**
   `Cargo.lock` resolves `openmls = "0.9"` to 0.9.0 (v0.1.1 and v0.2.0
   alike):
   - GHSA-gc79-23g3-8g52 (high, fixed in 0.9.1): unbounded recursion
     while decoding invalid nested extensions can abort the process —
     data Kupferdraht decodes from the backend and from peers;
   - GHSA-94v4-rg25-x625 (medium, 0.9.2): removed leaves reappear when
     an external commit truncates and regrows the ratchet tree —
     Kupferdraht joins and rejoins by external commit;
   - GHSA-p276-g3f3-wwgh (low, 0.9.2): rejected private messages advance
     the sender ratchet and block later messages from that sender.

   Neither `cargo deny` nor Dependabot flagged them: they are repository
   advisories of openmls/openmls, and the RustSec database carries none
   of them (2026-11-05).
2. **`mls.db` keeps the window Step 87 closed for `kupferdraht.db`.**
   `openmls_sqlite_storage` 0.3.0 builds its refinery runner without
   `set_grouped` (`storage_provider.rs`, `run_migrations`), and refinery's
   rusqlite driver commits each `execute` on its own: a migration's SQL,
   then its row in `openmls_sqlite_storage_migrations`. A process that
   dies between the two leaves the migration applied but unrecorded, and
   the next `KupferdrahtProvider::open` applies it again.
3. **Applied again, V3–V6 fail for good.** Measured 2026-11-05 with a
   trigger that aborts one history-row insert, then two more runs: V3–V6
   fail on `CREATE TABLE` ("already exists") on every later open; V2, a
   copy-and-swap, survives. The rows stay on disk, but the store cannot
   be opened without hand repair — today's only way out is to discard
   the device's MLS state. open.md §82 said "V2+"; that was read, not
   measured.
4. **When the window opens:** on the first open of every new `mls.db`
   (each login on a new device runs V1–V6) and on any later bump that
   adds migrations. The window is milliseconds, but a mobile OS kills
   apps at arbitrary points.

## Scope

1. **The pins.** `kupferdraht-core/Cargo.toml`:
   - `openmls_sqlite_storage`, `openmls_rust_crypto` and
     `openmls_basic_credential` move from `openmls-v0.9.0-schwarzdigits.1`
     to `openmls-v0.9.2-schwarzdigits.1` together (one tag per site,
     ADR-0042);
   - `openmls = "0.9.2"` at both sites (native and the wasm `js` line),
     so a consumer of the library cannot resolve an affected 0.9.0 or
     0.9.1; `openmls_traits` stays 0.6.0;
   - `Cargo.lock` re-resolves: crates.io `openmls` 0.9.2, the three fork
     crates at `9a228e8e9`. The step checks the diff of the three crates
     between the old and the new tag (expected: the grouped runner and
     its tests in `sqlite_storage`, nothing in the other two).
2. **No Kupferdraht code change expected.** 0.9.1 and 0.9.2 are patch
   releases; the gate shows whether the sender-ratchet change moves a
   test. `KupferdrahtProvider::open` already calls `run_migrations`;
   the fork does the rest. The browser build gets both through the same
   crates.
3. **No CLI command.** No public API changes; every login and every
   open of an account runs the path (Phase 6 noted, not skipped).
4. **The fork's own `[patch.crates-io]`.** `9a228e8e9` puts one in the
   fork's root manifest so its workspace holds one `openmls_traits`.
   Cargo honours `[patch]` only in the root manifest of the build, so it
   does not reach Kupferdraht; ADR-0042's "no `[patch]`" is about
   Kupferdraht's workspace. The fork table says so.
5. Docs in the same change:
   - `docs/adrs/0042-…` fork table: base `openmls-v0.9.2`, the new tag,
     the grouped runner and the fork's root `[patch]` in the change
     column; `rules/0042-…` names no tag and stays as it is (checked on
     the step).
   - `open.md` §82: the tag in its first paragraph; the gap-B.27
     paragraph leaves.
   - `docs/ARCHITECTURE.md` §7: the atomic migration run covers
     `mls.db` too (Step 142), not only `kupferdraht.db` (Step 87).
   - `docs/observations.md` §78: its closing note gains the `mls.db`
     resolution.
   - `vendor/README.md` (the fork line), `THIRD-PARTY-NOTICES` and
     `docs/THIRD_PARTY_LICENSES.html` (openmls 0.9.2, the new tag).
   - `CHANGELOG.md` `[Unreleased]`: **Security** — openmls 0.9.2 with
     the three advisories by id; **Fixed** — the `mls.db` migrations.

## Out of scope

- **Repairing a store the old runner already broke.** A `mls.db` with a
  migration applied but unrecorded fails on the next open with the fix
  too (the grouped run re-applies the same migration). No such store is
  known; repair would mean guessing which migration ran. The way out
  stays discarding the device's MLS state.
- Upstream's V7 and V8 (on `main`, unreleased): they arrive with a later
  openmls release line, on a new fork branch from that release tag,
  which then carries upstream's own fix if #2323 is merged.
- **Watching openmls's advisories.** `cargo deny` did not see these
  three; how Kupferdraht learns of the next one is a retrospective
  question (a proposed step if it needs code or CI), not this step.
- The other open points of §82 (branch policy, the next bump).

## TDD sketch

- **Pins (red today):**
  - `tests/step_90_sqlite3mc_engine.rs`: `OPENMLS_STORAGE_SOURCE` names
    tag `openmls-v0.9.2-schwarzdigits.1`;
  - a new pin that reads `Cargo.lock` and `kupferdraht-core/Cargo.toml`:
    `openmls` resolves to 0.9.2 or later, and both manifest sites
    require `0.9.2` — the security floor stays written down.
- **Behaviour (red on the old tag):** a unit test beside
  `KupferdrahtProvider::open` (`encryption/provider.rs` or a test file
  of its own): a `mls.db` in a temp dir gets refinery's history table
  `openmls_sqlite_storage_migrations` and a trigger that aborts V3's
  history-row insert, the state a crash leaves; `open` fails; reopened,
  V3's table (`vc_emulation_group_secrets`) does not exist; with the
  trigger dropped, `open` succeeds and V1–V6 are recorded. On the old
  tag the table survives the failed run and the next open fails
  "already exists".
- The existing storage and MLS suites stay green. A pre-check on
  2026-11-05 ran `kupferdraht-core`'s whole suite against the grouped
  runner on the 0.9.0 line through a temporary `--config patch` (not
  committed): green except three tests that read the lockfile's source
  or call `cargo metadata --locked`, which a patch cannot satisfy. It
  did not cover openmls 0.9.2.
- **Gate:** the five, plus `cargo check` and `cargo clippy` for
  `wasm32-unknown-unknown`; the browser tests run in CI.
- **Smoke:** `kupferdraht-smoke.sh --throwaway` on the wire-server-rs
  stack, 18/18: each throwaway login opens a fresh `mls.db` under the
  grouped runner, and the room, DM and push checks run on openmls 0.9.2
  (5.79: a change under every request runs the whole smoke).
