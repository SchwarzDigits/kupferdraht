# ADR-0022: Persist browser data in SQLite3MC over OPFS instead of IndexedDB per-record encryption

**Status:** Accepted (decision item 7 — no multi-tab coordination — superseded by ADR-0023; item 3 — OPFS
`sahpool` — superseded by ADR-0035)
**Date:** 2026-09-24
**Deciders:** Volker Zöpfel

## Context

This ADR supersedes **Part 2 (Browser persistence)** of ADR-0015 "Browser
architecture — WASM-only delivery, IndexedDB per-record persistence,
hand-written wasm-bindgen surface". Parts 1 (Web is WASM-only, no JS port)
and 3 (the hand-written wasm-bindgen surface in `src/wasm/`) stand
untouched; ADR-0015 stays Accepted for them, its status line names the
partial supersession. The rationale for changing course: ADR-0015 Part 2
chose native IndexedDB object stores with per-record AES-256-GCM and
**plaintext key and index fields** because the native engine of the day
(SQLCipher with vendored OpenSSL, ADR-0013) had no wasm build — the
metadata exposure (record counts, store names, key/index values observable
at rest) was a deliberate trade for indexability. ADR-0021 replaced the
engine with SQLite3MC, which ships wasm builds, and a sister project runs exactly
that in production over OPFS (`sqlite-wasm-rs` + the `sahpool` VFS).
Meanwhile the second storage schema has drifted as ADR-0015 predicted: on
the IndexedDB store the task cache, the Step-89 expiry sweep and
`storage_engine` are default no-ops, and MLS state is a whole-store
snapshot rewritten at every `persist_mls` call site (twelve of them).

Verified on 2026-09-24 (`spike/sqlite3mc-wasm/`, headless Firefox and
Chrome 153, dedicated worker, 7/7 pins): `sqlite-wasm-rs` 0.6.1 bundles
the byte-identical SQLite3MC 2.5.1 / SQLite 3.53.4 amalgamation vendored
in `vendor/sqlite3mc/` (SHA-256 equal) and honours
`SQLITE_WASM_RS_SOURCE_DIR`, so one file is the engine on every target.
rusqlite 0.37, refinery 0.9 and `openmls_sqlite_storage` 0.3 — Kupferdraht's
exact lines — compile and run on `wasm32-unknown-unknown` once the
`libsqlite3-sys` stand-in (ADR-0021) re-exports `sqlite-wasm-rs` on that
target. The production keying batch (`cipher = 'chacha20'` →
`plaintext_header_size = 32` → `key`, then the first read) works
unchanged; a wrong key fails at the first read; bytes exported from the
OPFS pool carry the Step-90 P2 layout (readable magic, page size, 32
reserved bytes per page, ciphertext schema and rows); a file written by
the native engine opens in the browser and reads its row back; refinery
stamps `applied_on` once `time`'s `wasm-bindgen` backend is on. The
`sahpool` VFS needs a **dedicated worker** (OPFS `SyncAccessHandle`),
implements no locking, has no shared-memory WAL (WAL is accepted only under
`locking_mode = EXCLUSIVE`, which every later open must set before its
first read or the open fails with `SQLITE_CANTOPEN`), defaults to a
rollback journal, and pre-allocates a fixed pool of file handles
(default 6). The wasm engine's default page size is 8192 (native: 4096).
Raw release size: today's `kupferdraht.wasm` 6.08 MB; the spike module
(engine + rusqlite + refinery + openmls_sqlite_storage + VFS) 2.10 MB.

Constraints in effect: the dependency graph stays permissive-only
(ADR-0010, ADR-0016 — `sqlite-wasm-rs`, `sqlite-wasm-vfs`, `rsqlite-vfs`
are MIT); no `unsafe` in hand-written Kupferdraht code (CLAUDE.md) — the one
FFI call the browser path needs (`sqlite3mc_vfs_create`, the cipher wrap of
a VFS registered after SQLite3MC initialised; the sister project's
bugfix finding) needs a home; rusqlite is
pinned to 0.37 by `openmls_sqlite_storage` 0.3 (rusqlite's own browser
path arrived in 0.38; refinery 0.9.2 caps rusqlite at 0.39); the new
crates need wasm-bindgen ≥ 0.2.128 / js-sys ≥ 0.3.105 (Kupferdraht was on
0.2.118 / 0.3.95); the C engine is compiled for wasm by `cc`, so every
build machine and CI needs a wasm-capable clang (Apple clang has none;
Homebrew LLVM / distro clang do); `createSyncAccessHandle` exists since
Chrome 102, Firefox 111, Safari 15.2 (MDN compat data); THREAT_MODEL.md
does not yet cover the browser at-rest model (ADR-0015 follow-up, Step 82).

## Options Considered

### Option A: Keep ADR-0015 Part 2 — IndexedDB object stores, per-record AES-256-GCM

**Pros:**
- Shipped and working; runs in every browser context (main thread and
  workers), IndexedDB transactions, no C compile for wasm, no worker
  requirement, no new dependencies.

**Cons:**
- Metadata at rest stays observable (store names, record counts, every
  primary-key and index value — room ids, message ids, account ids).
- A second 46-method storage implementation to keep in step with SQLite;
  already behind (task cache, Step-89 sweep, `storage_engine` are no-ops).
- MLS persistence is a whole-store snapshot rewrite per mutation; a crash
  between mutation and flush loses MLS state.
- A separate crypto path (`aes-gcm` per record) beside the native engine.

### Option B: SQLite3MC over OPFS `sahpool` — the native store code on wasm

The wasm build runs `SqliteStore`, `SqliteGlobalStore`, the refinery
migrations and `openmls_sqlite_storage` unchanged over rusqlite 0.37; the
`libsqlite3-sys` stand-in gains a wasm arm that re-exports `sqlite-wasm-rs`
(pointed at `vendor/sqlite3mc/`), stubs the two deprecated entry points
rusqlite 0.37 still names (`sqlite3_trace` / `sqlite3_profile`, compiled
out of the wasm engine) and wraps the one FFI call behind a safe function;
`wasm::Kupferdraht::open` installs the pool and the cipher wrap once, in a
dedicated worker.

**Pros:**
- Whole-file encryption in the browser — the metadata exposure of Part 2
  closes; the at-rest bytes are the native layout (fixture-proven).
- One storage code path: every `Store` method, the migrations, the task
  cache, the Step-89 sweep and `storage_engine` run on wasm by
  construction; `storage/indexeddb.rs`, the per-record helpers and the
  MLS snapshot machinery (`open_persistent` / `flush` / `persist_mls`) are
  deleted.
- MLS state is persisted continuously by `openmls_sqlite_storage`, as on
  native.
- One amalgamation, one provenance record, one engine-identity pin
  (`sqlite3mc_version()` = 2.5.1) on every target.
- The sister project's precedent: production-proven runtime pattern.

**Cons:**
- A dedicated Web Worker becomes mandatory for persistence; the main
  thread cannot host the store.
- Single connection per file, no cross-tab coordination (the VFS has no
  locks; a second tab's open of the same file fails on handle exclusivity).
- Browser floor Chrome 102 / Firefox 111 / Safari 15.2.
- A wasm-capable clang on every build machine and in CI; +~2 MB raw wasm.
- No WAL on wasm (rollback journal); header bytes 18/19 differ from native.
- The stand-in grows a second target arm — more code that is "ours to keep"
  (ADR-0021), including the `unsafe` FFI wrapper and two stubs.
- wasm-bindgen / js-sys / web-sys bump; `time/wasm-bindgen` must stay on
  for refinery (a silent panic otherwise).
- Reload survival is not provable in the headless harness (fresh profile
  per run) — a manual browser smoke stays part of every storage change.

### Option C: SQLite3MC over an IndexedDB-backed VFS (`relaxed-idb`)

`sqlite-wasm-vfs` 0.2 ships a VFS that stores SQLite pages as IndexedDB
blocks — no worker requirement.

**Pros:**
- Runs on the main thread; same engine and code path as Option B.

**Cons:**
- Relaxed durability by design (writes may be lost on a crash) — wrong for
  MLS state.
- Removed in `sqlite-wasm-vfs` 0.3; keeping it pins Kupferdraht to 0.5.5 and
  its bundled SQLite3MC 2.3.3, breaking the one-amalgamation property.

### Option B′: Option B with rusqlite bumped to ≥ 0.38 for its own browser path

**Pros:**
- No wasm arm in the stand-in; upstream's browser path.

**Cons:**
- Blocked by `openmls_sqlite_storage` 0.3's rusqlite 0.37 pin and
  refinery's ≤ 0.39 cap: needs a second stand-in (openmls_sqlite_storage)
  plus a refreshed libsqlite3-sys stand-in for the new sys line — far more
  vendored code than one target arm.

## Decision

**Option B: the browser persists through the same SQLite3MC store the
native targets run, over the OPFS `sahpool` VFS**, in this shape:

1. **Engine:** `sqlite-wasm-rs` 0.6.1 (`sqlite3mc` + `wasm-bindgen`
   features) with `SQLITE_WASM_RS_SOURCE_DIR` set to `vendor/sqlite3mc/`
   in the root `.cargo/config.toml` — the vendored, PROVENANCE-pinned
   amalgamation is the engine on every target; the wasm engine-identity
   test pins `sqlite3mc_version()` against `PROVENANCE.txt` like native.
2. **rusqlite path:** rusqlite stays 0.37; the `libsqlite3-sys` stand-in
   gains its wasm arm (re-export, the two deprecated stubs, the safe
   `sqlite3mc_wrap_vfs`). The one `unsafe` FFI call lives in `vendor/`,
   never in `kupferdraht-core`.
3. **VFS and context:** `sqlite-wasm-vfs` 0.3 `sahpool`, installed once by
   `wasm::Kupferdraht::open` (capacity 24 — three databases per account, main
   plus journal, with headroom) and wrapped by the cipher VFS as the
   default. Outside a dedicated worker `Kupferdraht.open` **fails closed** with
   a clear error — no silent in-memory fallback (MLS state must never be
   silently volatile).
4. **Same code:** `SqliteStore`, `SqliteGlobalStore`, migrations and
   `openmls_sqlite_storage` (`mls.db`) on wasm; `KupferdrahtHome` paths become
   the pool's file names; `storage/indexeddb.rs`, the per-record `aes-gcm`
   helpers, the MLS snapshot and every `persist_mls` site go.
5. **Journal:** rollback journal (`delete`) on wasm; WAL stays native-only
   (its iOS shared-container rationale does not exist in the browser, and
   the exclusive-locking ordering trap buys nothing). The 32-byte
   plaintext header stays for format parity.
6. **Keystore unchanged:** `IndexedDbKeyStore` (IndexedDB + WebCrypto)
   remains the browser keystore — a non-extractable `CryptoKey` can only
   be persisted in IndexedDB, and the database keys cannot live in a
   database they key. `idb` stays for the keystore only.
7. **Multi-tab:** single writer per file; V1 documents the constraint
   (`docs/bindings.md`) and builds no cross-tab coordination.

## Consequences

### Positive

- The browser's at-rest model equals native: whole-file ChaCha20-Poly1305,
  no plaintext index keys, one THREAT_MODEL description (Step 82).
- One storage implementation; the documented wasm no-ops (task cache,
  Step-89 sweep, `storage_engine`) close by construction.
- MLS state persisted continuously; the snapshot rewrite and its crash
  window are gone; ~1,700 lines of wasm-only storage code deleted.
- Format parity proven both ways in tests: a native file opens in the
  browser, and the P2 byte pins run against OPFS bytes.

### Negative

- Consumers must host Kupferdraht in a dedicated Web Worker; a main-thread
  `Kupferdraht.open` fails. The wasm surface contract changes.
- Browser floor Chrome 102 / Firefox 111 / Safari 15.2.
- No multi-tab: a second tab cannot open the same account; V1 offers no
  coordination, only documentation.
- Every wasm build needs a wasm-capable clang (Homebrew LLVM on macOS —
  Apple clang cannot); CI gains that dependency.
- Raw wasm grows by roughly 2 MB (measured after the swap).
- The stand-in now carries a second target arm — the two deprecated stubs
  and the `unsafe` wrapper are Kupferdraht-authored code under `vendor/`, a
  deliberate placement that must stay documented in `vendor/README.md`.
- wasm-bindgen / js-sys / web-sys bump (0.2.128 / 0.3.105), and the
  `time/wasm-bindgen` feature becomes load-bearing for refinery on wasm.
- Reload survival cannot be pinned in the headless harness; a manual
  browser smoke is part of the step and of every later storage change.
- No migration of existing browser data (test-only deployment, ADR-0021
  precedent) — a fresh origin storage, said plainly in the CHANGELOG.

### Neutral

- The pool stores files under an opaque directory with random names, but
  each pool file's 4 KiB header carries the SQLite file name in plaintext
  (`/kupferdraht/accounts/<account-id>/kupferdraht.db`) — the same exposure as the
  native on-disk path, no more.
- Page size 8192 on wasm (the crate's tuned default) vs. 4096 native;
  files self-describe, both open on both targets.
- Header bytes 18/19 are (1,1) on wasm and (2,2) on native; the P2 port
  asserts per target.
- ADR-0015 Parts 1 and 3 are untouched; the `MaybeSendSync` relaxation
  stays.

## Implementation Notes

- Root `Cargo.toml` keeps the `[patch.crates-io]` stand-in; root
  `.cargo/config.toml` gains `[env] SQLITE_WASM_RS_SOURCE_DIR = { value =
  "vendor/sqlite3mc", relative = true }`. The clang path is never
  committed (shell env; `docs/bindings.md` build section names it).
- `kupferdraht-core/Cargo.toml` wasm block: `sqlite-wasm-rs` 0.6.1,
  `sqlite-wasm-vfs` 0.3 (`sahpool`), `time` (`wasm-bindgen`); `rusqlite`,
  `refinery`, `openmls_sqlite_storage` move out of the native-only block;
  `idb` stays for the keystore; `aes-gcm` leaves if nothing else uses it.
- `storage/mod.rs`: the `cfg(not(wasm32))` gates on `schema`, `sqlite`,
  `apply_whole_db_key`, `open_default_store` collapse to one arm; the
  wasm arm of `open_default_store` / `open_default_global_store` /
  `open_mls_provider` skips `create_dir_all`; `SqliteStore::open` sets
  `journal_mode = WAL` on native only.
- `wasm::Kupferdraht::open`: install the pool + cipher wrap before the
  keystore; the worker check is the pool's `NotSupported` error mapped to
  a recoverable `KupferdrahtError::Storage`.
- Tests: `tests/step_91_*.rs` under `run_in_dedicated_worker` — the P2
  pins on exported pool bytes, wrong key at open, the native fixture
  import, refinery + openmls migrations, the Step-89 sweep on wasm
  (the plan's acceptance), `storage_engine`; the existing IndexedDB store
  tests are deleted, the keystore test stays.
- CI (W2, decided 2026-09-24): `cargo check --target wasm32-unknown-unknown`
  plus a headless-Firefox `wasm-pack test` job.
- Doc homes: ARCHITECTURE §3/§6.2/§7, bindings.md (worker requirement,
  build, the retired no-op note), library.md, README, THREAT_MODEL (via
  Step 82), CHANGELOG `BREAKING:` (worker requirement, no data migration),
  `rules/0015` + `rules/0022`, `vendor/README.md`, open.md gap-B.28.

## References

- Supersedes: ADR-0015 Part 2 (persistence) only; Parts 1 and 3 stand.
- Builds on: ADR-0021 (engine, pragma order, 32-byte `DbKey`, the
  stand-in), ADR-0002 (single crate), ADR-0009 (naming).
- Constrained by: ADR-0010 / ADR-0016 (permissive-only dependency graph),
  ADR-0015 Parts 1 + 3.
- Related: ADR-0003 (superseded — the per-field origin of this line).
- Evidence: `spike/sqlite3mc-wasm/` (7/7 pins, Firefox + Chrome 153,
  2026-09-24 — retired with Step 91's archive, last at `6bb5277`; the
  pins live on in `kupferdraht-core/tests/step_91_*.rs`); the sister
  project's SQLite3MC store, build notes and cipher-wrap bugfix finding
  (internal);
  sqlite.org `persistence.md` (opfs-sahpool); MDN compat data for
  `FileSystemFileHandle.createSyncAccessHandle`.

## Revision History

- 2026-09-24: Created (Step 91 orientation, Proposed)
- 2026-09-24: Accepted (Volker) — all six open spots decided as proposed
  (partial supersession of ADR-0015, fail-closed outside a dedicated
  worker, rollback journal + default page size on wasm, keystore stays
  IndexedDB + WebCrypto, the FFI wrapper lives in the vendored stand-in,
  the wasm-bindgen line bump); W2 = CI compile check + headless Firefox
- 2026-09-24: Step 91 merged (`6bb5277`) and archived; References
  repointed to where the spike's evidence lives on — no content change
  to the decision
- 2026-09-24: Decision item 7 (single writer, no cross-tab coordination in
  V1) superseded by ADR-0023 "Enforce one active Kupferdraht instance per
  browser origin with a Web Locks lease and explicit takeover"; the other
  decisions stand unchanged
- 2026-09-29: Decision item 3 (the OPFS `sahpool` VFS; the dedicated-worker
  requirement stands) superseded by ADR-0035 "Store an account's browser
  databases on sqlite-remote-vfs, with keys from a per-account seed that
  Kupferdraht seals with a PIN"; the other decisions stand unchanged
- 2026-10-08: Amendment, not a supersession — the sister project that
  served as the template is no longer named; its repository, file paths
  and note ids are internal. The decisions are untouched.
- 2026-10-31: The stand-in's wasm arm moved, unchanged, into the
  `libsqlite3-sys` of the SchwarzDigits rusqlite fork (ADR-0042, Step
  127); `vendor/sqlite3mc/` stays the amalgamation `sqlite-wasm-rs`
  compiles. The decisions are untouched.
