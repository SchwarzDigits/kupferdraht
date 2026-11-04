# Step 140 — sqlite-remote-vfs 0.6.0: the plaintext guard

**Status: in progress 2026-11-03** — red tests reviewed and approved, green
phase on `step/140-sqlite-remote-vfs-0-6`; commissioned the same day
(Volker: the SchwarzDigits pull requests are merged upstream and v0.6.0 is
tagged), before Step 130a, whose idempotency-key table lives in the
VFS-backed databases.

**Taken 2026-11-03 (Volker):** D1–D4 as designed — the pin on the tag's
commit, the guard in both modes, `Failure::Plaintext` as `Storage` (no new
error variant), no other change; THREAT_MODEL unchanged (the guard is
defence in depth behind A3, named in ARCHITECTURE and bindings); solo,
no teammates for a step this size.

## Problem

sqlite-remote-vfs 0.6.0 (tag `v0.6.0`, commit `8e4e7e2`) adds one interface
item and one robustness fix:

- **`Config::refuse_plaintext`** (default `false`): a read or a write of
  page 1 that carries an unencrypted SQLite header fails with `SQLITE_PERM`
  and the VFS reports `Failure::Plaintext` — only that read or write is
  refused, a write transaction rolls back, the database is not broken. It
  guards against a build that linked plain SQLite instead of SQLite3MC
  (`PRAGMA key` silently ignored), a forgotten or empty key on a connection
  through `encrypted_name()`, and a database stored in plaintext on the
  server. The check needs a plaintext header of at most 72 bytes:
  Kupferdraht's is 32 (`PRAGMA plaintext_header_size = 32`,
  `storage/mod.rs`).
- The server's page size and page count are followed only within SQLite's
  limits — the crate's own change, no interface.

Kupferdraht pins 0.5.0 (`2c6930f`, Step 123) and builds `Config` at two
sites without the guard — `storage/remote/server.rs` (`Config::new(Store::
Server(server))`) and `storage/browser.rs` (`Config::local(home_id)`). A
build defect or a lost key would store an account's databases in the clear
on the storage server, and nothing today would notice: THREAT_MODEL A3 and
G2 assume SQLite3MC encrypts, and `storageEngine()` reports what the engine
says, not what reached the server. `check_database` (`storage/browser.rs`)
matches `TakenOver`, `Denied` and `Unreachable` and lets every other failure
through as "reachable", so a refused page would surface as the bare
`SQLITE_PERM` of the next statement.

## Design

- **D1 — pin `rev = "8e4e7e2"`** (v0.6.0) instead of `2c6930f`; the lockfile
  changes the crates of that repository (and what they bumped).
- **D2 — `refuse_plaintext = true` at both `Config` sites**, server mode
  and local mode alike, with the header size named in the comment.
- **D3 — `Failure::Plaintext` → `KupferdrahtError::Storage`** in
  `check_database`, naming the cause (the SQLite build in use does not
  encrypt, or the database on the server is plaintext) — runtime data, a
  recoverable error, never an assertion. An open that meets the refusal
  reports the same.
- **D3a — the open path asks the VFS first** (found by PR #26's CI run,
  2026-11-03). The VFS's own message never reaches rusqlite: SQLite
  reports the bare `SQLITE_PERM` text ("access permission denied").
  SQLite3MC reads page 1 when it opens a file (sqlite-remote-vfs
  `539bff7`), so a database stored in the clear fails the open with
  `SQLITE_PERM` — which `browser_open_error` maps to `DeviceRetired`
  (Step 120, another device's storage slot), and `DeviceRetired` makes a
  login forget the account's keys (Step 121). `open_browser_connection`
  therefore consults `Failure::Plaintext` before the code; both sites
  share `storage::plaintext_refused`. The browser suite writes a
  database in the clear through a second VFS without the guard and
  pins that Kupferdraht's open fails `Storage`, not `DeviceRetired`.
- **D4 — no other code change**; no CLI (browser only); no new `pub`
  symbol, so no FFI mirror and no api-mapping row.

## Tests (red first)

- **Pin, `tests/step_91_wasm_build_glue.rs`:** the lockfile's
  sqlite-remote-vfs is 0.6.x (red against 0.5.0).
- **Pin, `tests/step_111a_local_vfs_pins.rs`:** the manifest pins
  `rev = "8e4e7e2"`.
- **Pins, `tests/step_140_plaintext_guard_pins.rs`:** both `Config` sites
  set `refuse_plaintext = true`; `check_database` names
  `Failure::Plaintext`; the doc homes (ARCHITECTURE, bindings, CHANGELOG)
  name the guard; CI's wasm clippy line and `scripts/wasm-test.sh` list the
  browser test.
- **Browser, `tests/step_140_browser_plaintext_guard.rs`** (dedicated
  worker, local mode): a connection on a prepared database through the
  VFS without `PRAGMA key` cannot create a table — the write is refused
  and the error names the plaintext guard; a keyed open of the same
  database afterwards works (the refused transaction rolled back, nothing
  plaintext was stored).

## Docs

`docs/ARCHITECTURE.md` (the browser storage paragraph: SQLite3MC encrypts
above the VFS, and since 0.6.0 the VFS refuses plaintext on its own),
`docs/bindings.md` (the server mode: a build that does not encrypt fails
with `Storage`, nothing reaches the server in the clear), `CHANGELOG.md`
(Security: the plaintext guard; Changed: sqlite-remote-vfs 0.6.0).

## Smoke

Server mode against a stage running storage server 0.6.0, if one exists
(a login, a resume, a send, a read); else "no smoke (the local mode is
covered by the browser test in CI; no storage server locally)".
