# Step 134 — DB-key rotation, `secure_delete`, and a schema version for `global.db`

**Status: commissioned 2026-11-01** (transferred from open.md gap-B.10 and
§71 M5; the threat model's G4 claim, Step 133, depends on it; the umask
modes of open.md §69 joined on 2026-11-02, since this step touches the open
paths).

## Problem

1. **gap-B.10.** Every database key is load-or-create
   (`storage/account_keys.rs`; ADR-0029: a key is created once and never
   replaced), so a key that leaked from a keystore dump or a backup stays
   valid for the file's whole life; SQLite3MC's `PRAGMA rekey` is unused.
   Rotation is what makes local forward secrecy (THREAT_MODEL G4) true after
   a key compromise, limits the damage of a keystore dump, and is what a
   compliance review expects of an encrypted store.
2. **M5.** `PRAGMA secure_delete` is set nowhere, so a deleted message's
   ciphertext stays in free pages until overwritten — readable only with
   the key and the file, but readable; and after a rotation the old pages
   are the new key's as soon as they are rewritten, not before. `global.db`
   (`storage/global.rs`) has `CREATE TABLE IF NOT EXISTS` and no schema
   version, so every change to it is an `ALTER TABLE` guarded by a column
   check.
3. **DB files carry umask default modes** (open.md §69, transferred
   2026-11-02). `open_connection` and `create_parent_dir` (`storage/mod.rs`)
   create `kupferdraht.db`, `mls.db`, `global.db` and `accounts/<id>/` with
   the process umask, typically 0644 / 0755; the keystore's files are
   0600 / 0700 (`keystore/file_store.rs`, `storage/atomic_file.rs`). The
   content is ciphertext and the keys sit in the OS keystore, so this is
   defence in depth — accepted on 2026-09-10 for whichever step next
   touches the open paths, which this one does (scan review 2026-11-02, L3).

## Design (to confirm in phase 1)

- `Kupferdraht::rotate_account_keys(account_id)` on the device manager;
  CLI `keys-rotate --id`. Native first. No live client of the account
  during the rotation: the manager refuses while it holds one, the
  ADR-0024 account lock guards against another process.
- Per database (`kupferdraht.db`, `mls.db`): generate the new key, store
  it under a staging keystore id in the ADR-0025 scheme (the suffix is a
  phase-1 decision), `PRAGMA rekey`, then promote — write the new key under
  the live id, delete the staging entry. A crash between rekey and promote
  leaves a file that opens with the staging key and not with the live one,
  so the open path tries the staging id when the live key fails on a file
  that exists. ADR-0029's rule stays: the key is never replaced blindly —
  the staging key *is* the key of that file.
- `secure_delete = ON` on every open, native and wasm (measure the cost on
  the browser VFS first). Only a `VACUUM` rewrites old pages under the new
  key: decide whether a rotation vacuums.
- `global.db` gets `PRAGMA user_version` and a first migration; the
  column-check pattern retires.
- Browser: the keys derive from the per-account seed with frozen HKDF
  labels (ADR-0035), so rotation there means a versioned label or a new
  seed with a new recovery export — a decision of its own, out of this
  step; the native path must not depend on it.

## Scope

1. Phase 1 questions: the staging-id suffix; crash atomicity of rekey +
   promote (SQLite3MC's rekey is a full rewrite — measure it on a large
   fixture); vacuum yes / no; the `secure_delete` cost on the wasm VFS;
   whether `global.db` is rotated too (it holds no content;
   recommendation: no); one ADR, "rotate database keys through a staged
   keystore id", which revises ADR-0029 with the staging exception.
2. The rotation, the CLI command, `secure_delete`, the `global.db`
   version.
3. The FFI mirror; the wasm surface refuses with a clear error until the
   browser decision lands.
4. Unix modes on the open paths: 0700 for `accounts/<id>/` (and the home
   when this step creates it), 0600 for the three database files; SQLite
   gives `-wal` / `-shm` the main file's mode. An existing file keeps its
   mode (no chmod sweep); wasm has no modes.

## Out of scope

- Browser rotation (the ADR-0035 labels) — its own decision.
- Rotating the device's MLS signature key or the storage-login key
  (ADR-0035): different objects, a different step.

## TDD sketch

- a rotated account opens with the new key and not with the old one (key
  material compared, file re-opened);
- a crash after rekey and before promote (a test hook) leaves an account
  the next open recovers through the staging id;
- the rotation refuses while the manager holds a client, and while another
  process holds the ADR-0024 lock;
- `secure_delete` is ON on every opened connection (PRAGMA read-back);
- on Unix, a fresh account's directory is 0700 and its database files
  0600 (metadata read-back, like `keystore/file_store.rs`'s tests);
- `global.db` reports a `user_version`, and a second open migrates nothing
  twice;
- CLI: `keys-rotate --id` names the rotated databases; the Step-88 engine
  pins stay green.
