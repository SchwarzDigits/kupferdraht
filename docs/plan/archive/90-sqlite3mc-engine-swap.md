# Step 90 — SQLite3MC engine swap (ChaCha20-Poly1305, ADR-0021)

**Status: MERGED 2026-09-24** — commits `cf7c042` (feat) + `2bf3c70`
(retro docs), fast-forwarded onto `main` (worktree instance, Phase-4 hard
stop waived by Volker). Gate green (775 tests), **smoked 2026-09-24** on
schwarz-dev-wire with test user 3 from a fresh home + a reopen run (users
1/2 at the device cap; the smoke device was purged afterwards). Phase 12
retired `spike/sqlite3mc/` (README, P1/P2 harness, P4 patch, stand-in) —
its pins live in `kupferdraht-core/tests/step_90_sqlite3mc_engine.rs`, the
stand-in in `vendor/libsqlite3-sys/`; the spike's last revision is in git
history at `d8d46c8`. Originally commissioned 2026-09-17 (Volker); built on
`step/90-sqlite3mc-engine-swap` (inputs applied as defaulted: I1 vendor —
`vendor/sqlite3mc/` + `PROVENANCE.txt`, the release `SHA256SUMS` signature
verified with OpenSSL against its Sigstore certificate, Fulcio chain /
Rekor not checked locally; I2 chacha20; I3 **decided by Volker 2026-09-24:
hard 32 — no legacy 48-byte blob accepted**, the branch's initial
accept-by-key-half default was removed the same day). **Cross-builds (scope 7): attempted, both BLOCKED by the
build machine, not by the engine** — `aarch64-apple-ios` fails before any
C is compiled (`xcrun: SDK "iphoneos" cannot be located`, CommandLineTools
only, no Xcode.app), `aarch64-linux-android` fails on the missing NDK
toolchain (`aarch64-linux-android-clang` not found, no `cargo ndk`); the
same environment gap Step 88 P5 recorded — re-run
`bindings/android/build-android.sh` / `bindings/swift/build-xcframework.sh`
on a machine with the SDKs. `cargo tree -i openssl-src` is empty. The
Step-76 `keyed.db` fixture was converted once in place (harness bonus-2
recipe; header byte 20 0x50 → 0x20, `integrity_check` ok, no sidecars).
Executes ADR-0021
(Accepted 2026-09-16, supersedes ADR-0013): every local database is
encrypted whole-file by **SQLite3MC 2.5.x in its `chacha20` scheme**
(ChaCha20-Poly1305, sqleet format) under a **raw 32-byte key** with a
32-byte plaintext header — no salt, no KDF, no HMAC subkey in the trust
path — and SQLCipher (Zetetic) plus its vendored OpenSSL leave the tree.
Evidence and head start come from Step 88
(`docs/plan/archive/88-sqlite3mc-verification.md`, verdict table): the harness
`spike/sqlite3mc/` (P1/P2 pins) and `spike/sqlite3mc/p4-workspace-adoption.patch`
— 70/70 targets green on the engine with the sqlcipher scheme, 68/70 on
chacha20 with exactly the two scheme-semantic tests this step retires or
converts. **Depends on the Step-88 branch being merged** (spike, patch,
ADR-0021 and rules live there).

## Inputs (phase 1, from Volker)

- **I1 — amalgamation in-tree or fetched at build.** Recommendation:
  **vendor** `vendor/sqlite3mc/sqlite3mc_amalgamation.{c,h}` +
  `PROVENANCE.txt` carrying the release-zip SHA-256 verified against the
  signed `SHA256SUMS` (the sister project's practice) — deterministic, offline,
  no network in any build; 13 MB of C that changes only on a pinned
  release bump. The alternative is the spike's `fetch-amalgamation.sh`
  (SHA-pinned download into a git-ignored path). Default if unanswered:
  vendor.
- **I2 — cipher: decided** (2026-09-16): `chacha20`, set explicitly per
  connection (`PRAGMA cipher = 'chacha20'`), not left to the compile-time
  default.
- **I3 — legacy keystore blobs.** ADR-0021 default: for one release the
  keystore boundary accepts a 48-byte `key ‖ salt` blob by its key half,
  so no consumer re-provisions; new material is written as 32 bytes.
  Alternative: hard 32 (Kupferdraht is test-only; consumers wipe). Default:
  accept legacy. **Decided 2026-09-24 (Volker): hard 32.**

## Scope

1. **Build glue.** `[patch.crates-io] libsqlite3-sys = { path =
   "vendor/libsqlite3-sys" }` — the Step-88 stand-in moved out of
   `spike/` (upstream 0.35.0 `src/` + pre-generated bindings; `build.rs`
   = upstream's `bundled` flag set + `-DSQLITE_TEMP_STORE=2`; **no**
   `SQLITE_EXTRA_INIT`); amalgamation per I1; `rusqlite` features
   `bundled` at both sites (dependency and dev-dependency) instead of
   `bundled-sqlcipher-vendored-openssl`. `openmls_sqlite_storage` forces
   `bundled` workspace-wide, which is exactly why the patch is the
   mechanism (observations §77 — never libsqlite3-sys's `SQLITE3_LIB_DIR`
   route). `deny.toml` / `about.toml`: drop whatever only OpenSSL needed.
2. **Keying.** `storage::apply_whole_db_key` sends
   `PRAGMA cipher = 'chacha20'; PRAGMA plaintext_header_size = 32;
   PRAGMA key = "x'<64 hex>'";` (parameters BEFORE the key), then
   **verifies the key by a first read** (`SELECT count(*) FROM
   sqlite_master`) so a wrong key surfaces at open as a recoverable
   `KupferdrahtError::Storage`, never later mid-migration and never as a
   panic (runtime data is not a code invariant). `journal_mode = WAL`
   after keying, unchanged. The mirrored manual batch in
   `tests/step_1c_mls_storage_path.rs` follows.
3. **Key material.** `storage::db_key::DbKey` becomes the 32-byte key:
   `SALT_LEN`, `salt()` and the salt half go; `MATERIAL_LEN = 32`;
   `from_keystore_bytes` — the single length-invariant site — accepts 32
   bytes (and, per I3, a legacy 48-byte blob by its first 32);
   `to_keystore_bytes` / `generate_db_key` emit 32. Per-database
   independent keys and keystore ids stay exactly as ADR-0013 laid them
   out (`account-<id>`, `account-<id>-mls`, `global-db`). **BREAKING**
   for `pub` consumers (`DbKey::salt()` gone, keystore blob 48 → 32) —
   CHANGELOG `BREAKING:` entry, `docs/api-mapping-matrix-rust-sdk.md`
   rows for `DbKey` / `generate_db_key`, `docs/library.md` §`db_key`,
   `docs/bindings.md` (a foreign `SecureKeyStore` stores an opaque blob
   whose length changes). Seven test sites construct `[0x42; 48]`
   material (`step_1a_storage`, `step_1b_devices_storage`,
   `indexeddb_mls`, `whole_db_encryption`, `storage/sqlite/mod.rs`,
   `client/resume.rs`, `encryption/provider.rs`) — all move to 32.
4. **Tests.** Retire `whole_db_encryption::right_key_wrong_salt_cannot_read_data`
   (the salt is not in the trust path); keep its other three. The
   engine-identity test asserts `SELECT sqlite3mc_version()` starts with
   `SQLite3 Multiple Ciphers` and equals the vendored version; add the
   at-rest control (header encrypted without the pragma — the mis-linked-
   SQLite guard). The Step-88 harness pins move into
   `kupferdraht-core/tests/step_90_sqlite3mc_engine.rs` (P1 timing < 1 s in
   the debug profile; the five P2 byte assertions on chacha20 through
   `SqliteStore::open`; the compile-time-default probe; the 96-hex form
   is moot without a salt). **Step-76 fixture:** `keyed.db` is converted
   ONCE in place — open in the sqlcipher scheme with the fixture key +
   salt (bytes 0..48), then `PRAGMA cipher = 'chacha20'; PRAGMA
   plaintext_header_size = 32; PRAGMA rekey = "x'<bytes 0..32>'"`, close
   cleanly (no `-wal`/`-shm` sidecar), commit the new bytes; `keyed.pub`
   and `plain.db` untouched; `fixture_db_key` becomes the 32-byte key;
   the guarded 0.8.1 generator's keyed branch follows the new
   `apply_whole_db_key` but is not run. The fixture roundtrip pin then
   says: 0.8.1-format state survives both the openmls bump and the engine
   swap.
5. **CLI (phase 6).** `kupferdraht-cli status` prints one storage line —
   engine version, cipher scheme, header mode (e.g. `storage: SQLite3MC
   2.5.1 · chacha20 · plaintext header 32`) — through the smallest `pub`
   surface that carries it (a `StorageEngineInfo` read off the open
   store), FFI-mirrored and listed in the api-mapping.
6. **Docs (phase 7, same commit).** `docs/ARCHITECTURE.md` §6.2 + §7
   (engine, 32-byte material, pragma order, key verification),
   `docs/library.md`, `docs/THREAT_MODEL.md` G3 + the at-rest section
   (ChaCha20-Poly1305 **and the accepted BSI TR-02102-1 deviation**, per
   ADR-0021 Negative Consequences — visible, never restated as
   "conformant"), `README.md` "What it does", `docs/cli.md` storage
   table, `docs/bindings.md` (line naming SQLCipher; keystore blob
   note), `CHANGELOG.md` `[Unreleased]` with the `BREAKING:` line,
   `rules/0021` (drop the transition note), `rules/0013` (guardrail
   "while the swap is pending" → landed), `CLAUDE.md` "What NOT to do"
   (drop the "until it merges" clause). No ADR change; a surfaced gotcha
   goes to `docs/observations.md`.
7. **Cross-builds (verification, not code).** On a machine with the
   SDKs: `bindings/android/build-android.sh` (cargo-ndk, arm64-v8a) and
   `bindings/swift/build-xcframework.sh` (Xcode) — compile-only counts;
   record the outcome in this file's header. `cargo tree -i openssl-src`
   is empty. CI (ubuntu, `pkg-config` + `libsqlite3-dev` present) is
   unaffected: the `bundled` route never probes pkg-config; the lockfile
   pin below makes a silent fallback to another SQLite a red test.

## Out of scope

- The browser: `sqlite-wasm-rs`/OPFS is the sister project's path and a later
  Kupferdraht step; ADR-0015 (IndexedDB per-record) stands.
- The sqlcipher-scheme fallback: compiled in, documented in ADR-0021,
  **no consumer knob** and no `PRAGMA cipher` parameter in the API.
- DB-key rotation (open.md gap-B.10), iOS on-device behaviour (20d/e),
  any change to the openmls storage schema — `mls.db` only re-keys.
- Existing developer databases: not migrated — fresh home (test-only
  deployment, ADR-0021 context); say so in the CHANGELOG entry.

## TDD sketch (red first)

- `tests/step_90_sqlite3mc_engine.rs` — lockfile pins in the Step-76
  style (`openssl-src` and `openssl-sys` absent; `libsqlite3-sys` is a
  `path` source), engine identity (`sqlite3mc_version()`), the five P2
  byte assertions on a `SqliteStore` file (magic, page/WAL bytes,
  reserved 0x20, schema + row ciphertext, wrong key fails / key reopens),
  the encrypted-header control, the compile-time-default probe, the P1
  debug-profile timing pin, and **wrong key fails at open** (the
  first-read verification).
- `step_7a_db_key_keystore.rs` — 32-byte material end to end; a legacy
  48-byte blob resolves to its key half (I3).
- `whole_db_encryption.rs` — three tests, salt test gone.
- `step_76_openmls_upgrade.rs` + `provider.rs` fixture pins — green on
  the converted `keyed.db`.
- CLI — `status` shows the storage line (`kupferdraht-cli` test or the
  smoke transcript).

## Smoke

No new backend surface, but the storage layer under real traffic is
worth one run: `scripts/smoke-baseline.sh` against the own instance from
a **fresh home** (pre-swap databases are unreadable by design), then a
second run on the same home (reopen path: key verification at open, WAL
identification on an existing file).

## Exit criteria

Gate green (five checks), `cargo tree -i openssl-src` empty, smoke
baseline green twice (fresh + reopen), cross-builds attempted and
recorded, every doc home above updated in the same commit, CHANGELOG
`BREAKING:` entry, transition notes gone from `rules/0021` /
`rules/0013` / `CLAUDE.md`. Phase 12 folds the spike: harness pins live
in `kupferdraht-core/tests/`, `spike/sqlite3mc/` is removed (its README,
patch and stand-in retired into `vendor/` + this file's header),
`fetch-amalgamation.sh` moves to `scripts/` only if I1 chose fetch.

Size: one to two days including docs; the P4 patch is the head start.
