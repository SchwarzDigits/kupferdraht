# Step 104 — A new database key is durable before its database exists

**Status: MERGED 2026-09-29** — commissioned 2026-09-28 (Markus — finding
H7, part 1, of the production-readiness review, `open.md` §71).
`76c8e6f`; fix `ff743fc`. Merged to `main` as PR #74 (`16e6d6e`); CI
green. Smoke 2026-09-29 in the browser (`bindings/web/opfs-smoke/`, no
backend surface). Scope shipped: browser keystore writes are tracked; a
new database key is durable before its database is created. H7 part 2
(the fixed OPFS pool) stays open in `open.md` §71. Retrospective:
`docs/LEARNINGS.md` §5.44. Archived with this header.

## Problem (verified in code 2026-09-28)

The browser keystore (`keystore/indexeddb_keystore.rs`) serves the
synchronous `SecureKeyStore` trait from memory and persists in the
background: `set_secret` fires `spawn_local` and drops the result
(`let _ = persist_one(..)`). Its guaranteed flush, `persist_all`, has no
caller and is not exported to JavaScript. The database-key paths
(`storage::load_or_create_key_locked`) generate a key, `set_secret` it,
and create the database with it in the same synchronous call. If the
background write fails (quota, private mode) or the tab closes first, the
database exists and its key does not: after a reload the open fails with
`KeyUnavailable` (Step 96) — chats, MLS state and the device are lost.

## Decisions (2026-09-28)

- **D1 — track the browser keystore's writes.** Every background persist
  reports its result; `keystore::flush_pending_writes()` awaits all
  writes still in flight and fails with the first error (a write that
  never reports is an error too). A failed write is also logged the
  moment it fails. Native keystores write synchronously and track
  nothing — the flush is a no-op there.
- **D2 — create keys ahead, durably.** Before a database is created,
  the async callers create its key (`storage::ensure_global_key_durable`,
  `ensure_account_keys_durable`) and await the flush; the synchronous
  open then finds the key. Call sites: the browser `Kupferdraht.open`
  (`global.db`), login / SSO finalize and resume (`kupferdraht.db` for the
  default provider, `mls.db`). A failed flush fails the call before any
  database file exists. A key this call wrote but the keystore could
  not store is forgotten again (`delete_secret`), so a retry generates
  and stores a fresh one instead of finding the unstored copy the
  browser keystore still serves from memory (review 2026-09-29).
- The `SecureKeyStore` trait stays unchanged (foreign implementations).

## TDD

- `keystore/pending_writes.rs`: the flush waits for a write still in
  flight; a failed write fails the flush once (and is drained); a write
  that never reports fails the flush.
- `storage/durable_keys.rs`: with a keystore whose write fails, the
  global and the account preparation fail and no database file exists;
  after a refused write no key lingers in the keystore, and the retry
  stores fresh ones.
- No browser test: the failure of an IndexedDB write cannot be forced
  deterministically there; the tracking is shared code, tested natively.

**As built (2026-09-29).**
- `keystore/pending_writes.rs` (wasm + tests): a thread-local list of
  `oneshot` receivers; `track_write` registers, `flush` drains until empty
  (writes registered meanwhile included) and returns the first error; a
  dropped sender is an error.
- `keystore::flush_pending_writes`: the flush on wasm, `Ok(())` natively.
- `IndexedDbKeyStore`: `set_secret` tracks its background persist and
  logs a failure; `delete_secret` logs a failure (not tracked —
  deletion is not a precondition of a later write).
- `storage/durable_keys.rs`: `ensure_global_key_durable` (wasm
  `Kupferdraht.open`) and `ensure_account_keys_durable` (login / SSO finalize
  via `assemble_pending_login`, now async; `ClientBuilder::resume`),
  path-unsafe account ids refused first; a failed flush forgets the
  keys the call wrote (`flush_or_forget`), so a retry stores fresh ones.
- Tests: 5 red → green; call sites covered by the existing login / resume
  suites (no dedicated test — the tracked write is crate-internal).
- Browser suites (`scripts/wasm-test.sh`): 8 suites / 22 tests green —
  run with `RUSTUP_TOOLCHAIN=1.98.0` and the LLVM `CC`/`AR` for wasm; on
  the machine's stale default toolchain every SQLite-backed suite fails
  to load in the worker, with or without this change.
- Smoke 2026-09-29 (no backend surface): `bindings/web/opfs-smoke/` with
  a release `wasm-pack` build, two headless Chrome 154 sessions on one
  fresh profile — `Kupferdraht.open` `OPEN-OK` both times; the second reopens
  the `global.db` the first created, under the key the first stored.
