# Step 134 — Deleted content and private files: `secure_delete` pinned, a truncating WAL, owner-only modes

**Status: MERGED 2026-11-17** — `a4105eb` on `step/134-secure-delete-and-private-files`,
with `e125982` (main merged in after Step 160), `dcc9d4b` (the WAL pragma back under its
call sites' wasm guard, after Step 113's source pin failed on CI) and `fdece53` (main
merged in after Step 160's archive), merged to `main` as PR #92 (`40a6a3b`); locally fmt,
0 build warnings, clippy, the library's tests, the storage, key and manager tests and
every source-scanning pin, the wasm32 check and both wasm clippy lines; CI green. No
smoke (local storage only); the CLI created a fresh home 0700 with `global.db` 0600.
Phase 1 (2026-11-16) split the
commissioned step: Volker chose (B) — deleted content and private files now, the key
rotation as Step 161, parked until a customer or an audit asks
(`docs/plan/161-db-key-rotation.md`). Decided on the way: `journal_size_limit = 0` (a).

**Commissioned 2026-11-01** (transferred from open.md gap-B.10 and §71 M5; the umask
modes of open.md §69 joined on 2026-11-02, since this step touches the open paths).

## What phase 1 found

- **`secure_delete` was already on.** SQLite3MC switches it on for every encrypted
  database (`mcAdjustBtree` calls `sqlite3BtreeSecureDelete(pBt, 1)`); the linked SQLite
  is not built with `SQLITE_SECURE_DELETE`, and a plain connection reads 0. Every
  Kupferdraht database is encrypted, natively and in the browser, so a deleted row's
  bytes have been overwritten since the engine swap (Step 90). Measured cost of
  switching it on: none natively; on the browser's local VFS about 6 % on single-row
  deletes (observations §105). Nothing to build; pinned by tests on all three open paths.
- **The WAL keeps earlier page images** of `kupferdraht.db` and `global.db` until it is
  overwritten: SQLite resets the WAL after a checkpoint but does not shorten it.
- **`global.db` needs no migration now.** Step 157 gave it `PRAGMA user_version`, and it
  has no column checks to retire; its first migration comes with its first schema change.

## Scope

1. Tests pinning `secure_delete = 1` on `global.db`, `kupferdraht.db` and `mls.db`.
2. `journal_size_limit = 0` on the native WAL databases (`kupferdraht.db`, `global.db`):
   the WAL is truncated after each checkpoint. `mls.db` and the browser keep the
   rollback journal.
3. Owner-only modes on Unix: a new home, `accounts/` and `accounts/<id>/` are created
   0700; a new `global.db`, `kupferdraht.db` and `mls.db` 0600 (created before SQLite
   opens it; SQLite gives `-wal`, `-shm` and `-journal` the file's mode). An existing
   file or directory keeps its mode. The browser has no modes.
4. THREAT_MODEL G4 says what is and is not provided; ARCHITECTURE §7; CHANGELOG;
   open.md §69's umask entry resolved.

## Out of scope

- Key rotation: Step 161.
- The asset cache's directories and files.
