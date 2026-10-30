# Step 78 — Local account purge (destroy_store)

**Status: commissioned 2026-09-10** (Kalium-audit follow-up —
Volker: too critical to backlog). Also closes kupferdraht-tui
open.md §10 (the TUI's profile-delete reimplements this today).

## Problem (verified 2026-09-10)

The Kalium JVM audit found logout leaving `-wal`/`-shm` files with
plaintext content behind. Kupferdraht's residue is ciphertext — SQLCipher
encrypts WAL frames too, and the keys live in the OS keystore — but
the structural gap is the same: `Client::logout(purge_device=true)`
only deletes server-side state (`client/session.rs:117ff` — no
`remove_dir_all`, no `delete_secret`). What stays behind after a
"full" logout:

- `base/accounts/<id>/` — kupferdraht.db, mls.db (+ any WAL/SHM
  sidecars), attachment cache: all ciphertext, but keyed by…
- the account's keystore entries (kupferdraht.db key, mls.db key) —
  **the sensitive residue**: whoever holds the OS-keystore session
  can still open everything.
- the account's row in the global registry (global.db).

The account key ids are kupferdraht-internal, so a consumer CANNOT clean
up correctly from outside (kupferdraht-tui open.md §10) — the TUI's
picker-delete does a best-effort dir + one keychain entry and
misses the rest.

## Scope (design decisions for phase 1 — ask, don't guess)

1. **Where the API lives**: manager level
   (`KupferdrahtManager::destroy_account(account_id)` — works without a
   live session, the TUI's actual need) and/or
   `Client::logout(purge_device=true, purge_local=true)`.
   matrix-rust-sdk naming check in orientation (ADR-0009).
2. **What is deleted, in order**: keystore secrets first or last?
   (Deleting keys first makes the DBs unrecoverable even if the dir
   delete fails — probably the right order; the reverse leaves a
   recoverable window.) Account dir recursive incl. sidecars;
   global-registry row; in-memory caches when a live Client is
   involved.
3. **Failure semantics**: best-effort with a summary (which parts
   remain) vs. fail-fast — a half-purged account must be
   re-purgeable.
4. FFI + CLI (`logout --purge-local` / `account-destroy`) in the
   same step; kupferdraht-tui adoption is the consumer follow-up.

## TDD sketch

- Purge removes: every file under the account dir (WAL/SHM pinned
  explicitly via a dirty-connection fixture), both keystore
  secrets, the registry row; a second purge of the same id is a
  clean no-op.
- Purge with a missing dir / already-deleted secret still removes
  the rest (idempotent recovery).
- Other accounts' dirs/secrets are untouched (multi-account pin).

**Note (Step 110, 2026-09-30):** the browser's `logout({ purgeLocal: true })`
(ADR-0035 — forget the account's seed, keys, local databases and registry
entry in this browser) is planned as this purge's browser face rather than a
second deletion path; Step 110 deferred it here.

**Note (Step 114, 2026-09-30):** the browser's purge landed on its own
(Markus: browser only; this step adopts it natively):
`Kupferdraht::logout_client(client, PurgeOptions { purge_device, purge_local },
password)` in `manager/purge.rs`. Its answers to this step's questions: the
backend first (a refused password changes nothing, the call can be
repeated); the account's databases are closed through `Store::close` /
`MlsClient::close_storage`, then deleted; keys are forgotten and flushed
before the registry entry goes; the client is dropped from the manager's
cache. A manager-level purge without a live client stays this step's
question (`open.md` §72).

