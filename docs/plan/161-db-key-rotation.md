# Step 161 — DB-key rotation through a staged keystore id

**Status: parked 2026-11-16** (Volker) until a customer or an audit asks for it. Split
out of Step 134 after its phase 1: rotation helps in a narrow case — a key that leaked
once, without lasting access to the keystore, and a later copy of the file — and is
the largest part of the work; copies of the file taken before a rotation stay readable
with the old key, and an attacker with lasting keystore access gets the new key too.
Until then THREAT_MODEL G4 says the keys are not rotated.

## Problem

Every database key is load-or-create (ADR-0029: created once, never replaced), so a key
that leaked from a keystore dump or a backup stays valid for the file's whole life;
SQLite3MC's `PRAGMA rekey` is unused. A compliance review of an encrypted store may ask
for rotation.

## Phase 1, measured on 2026-11-16 (Step 134; `docs/observations.md` §105)

- **`PRAGMA rekey` is atomic.** SQLite3MC rewrites every page of the file, free pages
  included, in one write transaction and commits at the end (`sqlite3_rekey_v2` in the
  vendored amalgamation); a failure leaves the old key valid. No `VACUUM` is needed.
- **In WAL mode the new pages go to the WAL first.** Right after the commit only the new
  key opens the file; the main file keeps old-key pages until a checkpoint, so
  `wal_checkpoint(TRUNCATE)` follows the rekey. On a 267 MB database (release build,
  the developer's Mac): rekey 1.1–1.2 s, the WAL grows to the file's size (268 MB, so the
  rotation needs that much free space), the checkpoint 1.0 s.
- **Another open connection must be ruled out.** ADR-0024's account lock guards only the
  token refresh and the key creation; another process may have the database open. A
  rekey while it does succeeds, and that connection's next write fails with "file is not
  a database" (the file stays intact). `PRAGMA locking_mode = EXCLUSIVE` with
  `busy_timeout = 0` and a `BEGIN EXCLUSIVE` fails at once with "database is locked"
  while any other connection is open, and succeeds when none is.

## Design (as phase 1 left it)

- `Kupferdraht::rotate_account_keys(account_id)` on the device manager, native only (the
  browser's keys derive from the seed with frozen HKDF labels, ADR-0035 — a decision of
  its own); FFI mirror; the wasm surface refuses with a clear error; CLI
  `keys-rotate --id`.
- Three guards: the manager holds no client of the account; the exclusive SQLite lock
  above (refused as `AccountLocked`, retryable); the ADR-0024 account lock held for the
  rotation — a third critical section beside the token refresh and the key creation.
- Per database (`kupferdraht.db`, `mls.db`; not `global.db`, which holds the registry and
  settings, no content): the new key under a staged id
  (`kupferdraht.<home>.account-<user>-staged`, `…-mls-staged`) → rekey →
  `wal_checkpoint(TRUNCATE)` → the new key under the live id → the staged id deleted.
  When the live key does not open an existing file, the open path tries the staged key
  and promotes it under the account lock; every crash point recovers. The Step-78 purge
  deletes the staged ids too.
- One ADR, "rotate database keys through a staged keystore id", amending ADR-0029 (the
  staged exception) and ADR-0024 (the third critical section).

## TDD sketch

- a rotated account opens with the new key and not with the old one;
- a crash after the rekey and before the promotion (a test hook) leaves an account the
  next open recovers through the staged id;
- the rotation refuses while the manager holds a client, while another connection has
  the database open, and while another process holds the account lock;
- CLI: `keys-rotate --id` names the rotated databases.
