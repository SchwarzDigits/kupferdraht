# Step 157 — Code and data match: a database newer than the code is refused by name, and tabs of different versions do not mix

**Status: MERGED 2026-11-11** — `7bbff88` on `step/157-code-and-data-match`, merged to
`main` as PR #72 (`d3968b8`); the first step under the two-half gate: locally fmt, 0
build warnings, clippy, the step's tests and the whole library suite (911), the wasm32
check, `clippy --lib` and CI's wasm clippy line; the browser suite green locally (18
suites, 87 tests, headless Firefox); the Kotlin bindings and `swift test` (5 of 5)
green; CI's full suite, doctests and deny green, its browser job green on a rerun after
the CI-only "timeout waiting for leader-changed" hit the new
`the_leader_names_its_version`. No smoke and no CLI command of its own (every command
that opens a home meets the error). Scope: `KupferdrahtError::IncompatibleStore {
database, reason }` (BREAKING) for a database newer than the code or divergent from
it, `global.db`'s `PRAGMA user_version` 1, the tabs' `KUPFERDRAHT_VERSION` with
`relation` and `VersionMismatch` on both sides, a candidate that stops on a final
failure. Phase 1 decisions and red tests approved by Volker on 2026-11-10: one BREAKING variant
`IncompatibleStore { database, reason }` (`"newer"`, `"divergent"`); `global.db`
gets its schema version here (`PRAGMA user_version` 1, Step 134 keeps the first real
migration); the tabs module carries `KUPFERDRAHT_VERSION` pinned to `Cargo.toml`,
while Rust, Kotlin, Swift and the wasm package already read the crate version; both
sides of the tab protocol refuse another release; `relation` is this tab against the
leader; `serveTab({ version })` checks the wasm package after `load`, so thin tabs
still load no wasm; `PROTOCOL_VERSION` stays 1.

**Commissioned 2026-11-10** (Volker), from a report of the browser embedder:
after an update, a tab still open on the old version stays the leader, the new tab
sends it requests it does not understand, and the user sees a login that fails or
features that do nothing, with no hint that a background tab is the cause. Before
Step 156. Decided on commissioning:

- **The rule, on every platform:** a database opens only when its schema is the
  code's. The one exception is a migration forward: an older schema meets newer code,
  is migrated, and then matches. A database newer than the code, or one whose applied
  migrations differ from the code's, is refused.
- **Kupferdraht supports, the app decides.** Kupferdraht makes a mismatch recognisable
  in its own parts (the databases, the tab protocol). Who takes over, when an old tab
  reloads, and what an app version means is the embedder's: no automatic takeover or
  reload, no app build numbers in the SDK.

## Problem

1. **The refusal exists but cannot be told apart.** `kupferdraht.db` migrates through
   refinery with its defaults (`storage/schema.rs`, `abort_missing` and
   `abort_divergent` both `true`): an applied migration the code does not know is
   `MissingVersion`, one with another checksum `DivergentVersion`; `mls.db` migrates
   through openmls's runner with the same defaults. Both reach the caller as
   `Storage("migration failed: …")`. Neither an app nor `kupferdraht-tabs` can tell
   "this code is too old for these data" from any other storage failure, and no test
   pins the four cases.
2. **Tabs of different versions mix silently.** The leader's announcement carries only
   `PROTOCOL_VERSION`, which has been `1` since Step 94, and no Kupferdraht version. A
   newer tab follows an older leader; a method the leader lacks fails with
   `UnknownMethod`, and a call whose shape changed (a BREAKING release) answers in the
   old shape.
3. **An old tab keeps trying.** A deposed old tab queues again with its old code. Once
   it wins the lease after a newer leader migrated the databases, its open fails, and
   its candidate retries every few seconds instead of saying why.

## Scope

1. **A typed error for data newer than the code:** a database newer than the code, or
   divergent from it, is its own `KupferdrahtError` variant naming the database, on the
   native API, the FFI and the browser (`err.name`), instead of `Storage`.
2. **The four cases pinned** for `kupferdraht.db`, `mls.db` and `global.db` (phase 1
   checks how `global.db` migrates): equal opens; older is migrated and then opens;
   newer is refused; divergent is refused.
3. **The tab protocol names the version:** the leader's announcement carries
   `kupferdraht_version()`; the host shows the leader's version (on `leader-changed`
   and as a getter). A thin tab whose version differs from the leader's gets a typed
   error for its calls instead of an answer in another shape.
4. **No endless retry:** a candidate whose open fails with the error of 1 stops queuing
   and reports it through the host's `error` event with that name.
5. **A recipe in the tabs README,** about ten lines, for what the app may do with it: a
   newer tab calls `host.takeOver()`; an older one, on `evicted` or the typed errors,
   calls `host.close()` and reloads, or shows a banner while it holds unsent input.

## Out of scope

- Automatic takeover or reload by the SDK; app build numbers.
- Changing refinery's or openmls's migration semantics.

## Decisions phase 1 brings to Volker

- The error's name and payload (for instance `StorageNewerThanCode { database }`),
  and whether "divergent" shares it.
- Whether the version comparison in the tabs is "equal or not" or ordered (newer,
  older), which the recipe needs to pick who takes over.
- Whether a change to `PROTOCOL_VERSION` stays meaningful beside the version, or the
  version replaces it.

## TDD sketch

- Native: a database with one more applied migration than the code is refused with the
  new error; one with a changed checksum too; an older one is migrated; per database.
- Browser (`step_94` suites with the stub): a leader of another version is visible on
  the host; a thin tab's call to it fails typed; a candidate refused by newer data
  stops queuing and reports the error.
- FFI: the error variant on the generated Kotlin and Swift.
- No smoke beyond the browser suite: the failure needs two builds side by side, which
  the suite's stub plays.

## Docs

`docs/library.md` (the error), `docs/bindings.md` and the tabs README (the version, the
errors, the recipe), `docs/ARCHITECTURE.md` §7 (the rule), ADR-0026 or a new ADR if
phase 1 finds the tab rule is a decision, `CHANGELOG.md`.
