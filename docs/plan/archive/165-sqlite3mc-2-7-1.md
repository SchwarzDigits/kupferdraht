# Step 165 — SQLite3MC 2.7.1: the engine leaves 2.5.1, skipping 2.6

**Status: MERGED 2026-11-24** — commit `f24d6dc` with the docs fix-up `860f8b4`
(PR #121, `9bcaf52`, 2026-11-24). The fork chain the same day, in order and
signed: rusqlite `v0.37.0-sqlite3mc-2.7.1` (`fdfc1d8`), refinery
`v0.9.1-sqlite3mc-2.7.1` (`47b0abd`), openmls `openmls-v0.9.2-schwarzdigits.2`
(`f69b9b9`). Locally fmt, 0 build warnings, clippy native, 203 tests in the
step's five targets, the wasm32 check, wasm clippy `--lib` and CI's line, the
CLI's `storage_line` tests, `cargo metadata --locked`; CI green on both runs.
Smoke on server 4 (Wire 5.38.9) on 2026-11-24: two homes written by main's
CLI (engine 2.5.1), opened by this branch's CLI — `status` names 2.7.1, the
2.5.1-written timeline reads, a message each way on the 2.5.1-written MLS
state, both devices purged; 18 ok, 0 failed. Retrospective: LEARNINGS 5.107.

Commissioned 2026-11-23 (Volker), from the review of Dependabot's lockfile
group of 2026-11-22: sqlite-wasm-rs 0.6.3 began to bundle SQLite3MC 2.6.1,
which raised the question of the vendored engine's own age. Branch
`step/165-sqlite3mc-2.7.1` off `main` `b380398`. No new public symbol, no CLI
command (`status` already prints the engine).

## Problem

The whole-database encryption engine under every Kupferdraht database is
the vendored SQLite3 Multiple Ciphers 2.5.1 on SQLite 3.53.4 (release
2026-08-27, vendored by Step 90, compiled by the SchwarzDigits rusqlite fork
since Step 127, ADR-0042). Upstream has released four versions since:

- **2.6.0 / 2.6.1 (2026-10-12)** add hardware-accelerated ChaCha20 and
  Poly1305 (code adopted from libsodium), encrypt temporary files, harden the
  entropy source and verify the MAC before decrypting — and carry a defect
  upstream documents as a caution on both releases: the hardware-accelerated
  ChaCha20 carries a 32-bit block-counter overflow into the nonce where the
  scalar code wraps it (RFC 8439), so a page whose random counter start lies
  within the last blocks below 2^32 decrypts silently wrong when read with
  the other code path; the tag still verifies. About 1 in 67 million 4 KiB
  page writes. Kupferdraht never shipped 2.6.x; this step skips it.
- **2.7.0 (2026-11-13)**, "an update is strongly recommended": the counter
  overflow is prevented so every implementation encrypts alike; encryption
  errors on writing are propagated (before: dropped); a partial page read is
  an error (before: silently dropped); hot journals of an encrypted database
  are recognised and no longer removed; a deadlock with sorter worker
  threads and several mutex and out-of-memory defects are fixed; the backup
  API check is corrected (#280); key setup errors through URI parameters or
  `ATTACH` can no longer leave a database silently unencrypted; Argon2's
  return code is checked. Kupferdraht keys by `PRAGMA`, attaches nothing and
  derives no key, so the last three are not its exposure — the write-error
  and page-read fixes are.
- **2.7.1 (2026-11-15)**: `sqlite3_rekey` honours transient parameters
  (#309; Kupferdraht does not rekey — Step 161 is parked), the 64-bit wasm
  RNG abort (#308; wasm32 is unaffected).
- SQLite moves from 3.53.4 to **3.54.1** (2026-10-16), which fixes a possible
  heap-buffer overflow on 32-bit builds (64-bit builds are not affected).

The sibling project (sqlite-wasm-rs) already compiles 2.6.1 for everyone
who does not set `SQLITE_WASM_RS_SOURCE_DIR`; Kupferdraht's browser build
compiles the vendored copy and so stays on whatever `vendor/sqlite3mc/`
holds — today 2.5.1, after this step 2.7.1.

## What 2.7.1 changes for Kupferdraht's use, checked

Kupferdraht's use of the engine: the `chacha20` scheme with the
compile-time defaults, a raw 32-byte key by `PRAGMA key = "x'…'"` after
`PRAGMA cipher` and `PRAGMA plaintext_header_size = 32`, WAL on native and
the rollback journal in the browser, `secure_delete` on, temporary data in
memory (`SQLITE_TEMP_STORE=2` in the fork's `build.rs`), `sqlite3mc_version()`
as the only SQLite3MC function called. Against 2.7.0's compatibility notes:

- The raw key notation `x'…'` (SQLCipher's) is unchanged; the new `raw:hex:`
  and `raw:bin:` prefixes are additions.
- "WAL legacy mode" is removed: never set here (`mc_legacy_wal`).
- "Enable encryption before a temporary database is used": the key is the
  first thing an open connection receives, before any statement.
- `SQLITE_ENABLE_ATOMIC_WRITE` / `BATCH_ATOMIC_WRITE` disabled: not set
  by the fork's `build.rs`.
- `sqlite3mc_config` and `sqlite3mc_codec_data` direct-only: not called.
- `aes256cbc` and `kdf_iter`, Argon2 parameters, VLE: not used.
- `sqlite3_initialize` still calls `sqlite3mc_initialize` itself (verified
  in the 2.7.1 amalgamation): `SQLITE_EXTRA_INIT=sqlite3mc_initialize`
  stays forbidden (`vendor/README.md` recipe 4, `rules/0021`).
- The on-disk format of the `chacha20` scheme (sqleet) is unchanged — the
  fixture `tests/fixtures/step-91/native-chacha20.db`, written under 2.5.1
  and never regenerated, is opened by the current engine in
  `step_91_wasm_build_glue` (natively) and by the browser suite; it is the
  cross-release pin. The compile-time default cipher stays `chacha20`
  (`compile_time_default_cipher_is_chacha20`).

## Verification of the release (2026-11-23, wire-dev)

`sqlite3mc-2.7.1-sqlite-3.54.1-amalgamation.zip` (3 954 336 bytes), SHA-256
`d6bc59678e015c5ac6d3ec5d7b657c351230b36c8052b185e446c9d52ba01af0`, equals
its line in `sqlite3mc-2.7.1-SHA256SUMS`. The sums file's Sigstore signature
(`.sig`, base64 DER ECDSA) verifies with `openssl dgst -sha256 -verify`
against the public key of the accompanying certificate (`.pem`): "Verified
OK"; issuer O=sigstore.dev, CN=sigstore-intermediate; serial
74B05ED601CAB26012800102F1C4DA8DF88731D7; validity 2026-11-15T10:57:16Z –
11:07:16Z; SAN URI
`https://github.com/utelle/SQLite3MultipleCiphers/.github/workflows/build-reusable.yml@refs/heads/main`;
SHA-256 fingerprint
7A:9C:D4:00:37:05:74:66:DB:B7:9F:B9:5B:7D:AE:96:67:45:5E:38:0B:1F:74:E0:38:6E:B9:A7:21:C2:71:E2.
Not verified locally, as in 2026-09: the chain to the Fulcio root and the
Rekor inclusion (no cosign on this machine). Header defines: SQLite3MC
2.7.1, SQLite 3.54.1 / 3054001. `LICENSE` at tag v2.7.1 is byte-identical to
the vendored one. Amalgamation SHA-256: `.c`
`065561274c387230cb32443e830b2da0bb91f3c92f495b39308e416464488a4d`, `.h`
`9404d6d1974fc29fdb67089dc396ae84f89e5e85cd5ba07e94cae459a51c872b`.

**Embedded third-party code, read file by file (the Step-150 duty):** the
2.7.1 amalgamation adds twenty source sections against 2.5.1. New notices:
`hwcheck.c` (Ulrich Telle, MIT — SQLite3MC's own), `btreecopyfile.c`
(SQLite's public-domain blessing), `csprng_entropy.c` (SQLite3MC's own,
split out of the ChaCha20 code). The hardware-accelerated ChaCha20 and
Poly1305 paths are libsodium's files (`crypto_stream_chacha20.h`, `export.h`,
`private/common.h`, `src/stream_chacha20.c`, `src/onetimeauth_poly1305.c`,
`sse2/poly1305_sse2.c`, `donna/poly1305_donna.c` with its `32`/`64`
headers, `ref/chacha20_ref.c`): libsodium is ISC-licensed (Copyright (c)
2013-2026 Frank Denis), and the amalgamation reproduces none of that notice
— the only header among them is D. J. Bernstein's "Public domain" on
`chacha-merged.c`. The ISC license asks for its copyright and permission
notice in all copies, so Kupferdraht's notices name libsodium and carry the
notice (`THIRD-PARTY-NOTICES`, `about.hbs`, `PROVENANCE.txt`
`embedded-licenses`). The miniz, Argon2, fastpbkdf2, SHA-2, AEGIS and
public-domain entries are unchanged.

## Scope

1. **Vendor 2.7.1** (`vendor/sqlite3mc/`): the two amalgamation files
   replaced byte-for-byte from the verified zip, `LICENSE` unchanged,
   `PROVENANCE.txt` rewritten (every line, the verification above, the
   embedded-licenses line with libsodium's ISC).
2. **The fork chain, in ADR-0042's order** — Volker commits, tags and
   pushes in each fork (scripts in the hand-off):
   - rusqlite, branch `sqlite3mc/0.37`: the four files copied into
     `libsqlite3-sys/sqlite3mc/`, `sqlite-wasm-rs` requirement `0.6.3`
     (recipe 3; what the lockfile resolves), upstream's suite with
     `--features bundled`, tag **`v0.37.0-sqlite3mc-2.7.1`**;
   - refinery, branch `sqlite3mc/0.9`: the rusqlite tag at both sites that
     name it, `refinery_core/Cargo.toml` and the workspace's
     `examples/Cargo.toml` (one graph; two tags are two `libsqlite3-sys`
     with the same `links` value and the check fails), tag
     **`v0.9.1-sqlite3mc-2.7.1`**;
   - openmls, branch `schwarzdigits/openmls-0.9`: the rusqlite and refinery
     tags in `sqlite_storage/Cargo.toml`; the branch tracks its `Cargo.lock`,
     which moves the two fork sources and `sqlite-wasm-rs` 0.6.2 → 0.6.3
     with its `sqlite3mc-src`, nothing else; tag
     **`openmls-v0.9.2-schwarzdigits.2`**.
   The earlier tags stay: v0.6.0 names them.
3. **Kupferdraht**: the six tag sites of `kupferdraht-core/Cargo.toml`
   (rusqlite twice, refinery, the three openmls crates), `Cargo.lock`, the
   notices (`THIRD-PARTY-NOTICES`, `about.hbs`, `docs/THIRD_PARTY_LICENSES.html`,
   the fixture strings of `third_party_notices.rs`), the docs that name the
   release (`vendor/README.md`, `.cargo/config.toml`, `docs/library.md`,
   `docs/cli.md`, `docs/bindings.md`, `docs/api-mapping-matrix-rust-sdk.md`,
   `storage/sqlite/engine.rs`'s doc comment, the CLI's `storage_line` test
   fixture, open.md §82, `step_91`'s doc comment on the bundled copy — now
   behind, not ahead), observations §109 (what 2.7.1 changed for the use
   above, the libsodium notice gap), `CHANGELOG.md` `[Unreleased]`
   (`Security` and `Changed`; a Rust consumer that names the fork tags in
   its own manifest moves them — `docs/library.md` "Consuming from Rust").
4. **Gate**, local half: `cargo fmt --check`, 0 build warnings, clippy
   native, the step's tests (`step_90_sqlite3mc_engine`,
   `step_91_wasm_build_glue`, `third_party_notices`, the storage unit tests),
   the wasm32 check (the wasm engine compiles the changed vendor directory).
   CI half: the full suite, `cargo deny`, the wasm job with the browser
   tests (the browser's own databases under 2.7.1; the 2.5.1 fixture is
   read natively only), the compliance scans.
5. **Smoke on server 4** (valid client flows only): a home created with the
   v0.6.0 CLI (engine 2.5.1, a login and a room with a message), then the
   same home opened by this branch's CLI: `status` names 2.7.1, the rooms
   list and a message round trip work on the 2.5.1-written files — the
   upgrade path on real data.
   Run 2026-11-24 (`handoff/165/smoke165.sh`, both CLIs built from this
   machine, main at b380398 for the 2.5.1 side): 18 checks, 0 failed —
   two homes written under 2.5.1 (login, room, invite, a message read by
   the other device), opened under 2.7.1: `status` names 2.7.1 on both,
   the 2.5.1-written timeline reads, a message in each direction on the
   2.5.1-written MLS state, both devices purged.

## TDD list (phase 3)

- `step_90_sqlite3mc_engine`: `lockfile_resolves_the_sqlite3mc_line_to_the_forks`
  and `forks_amalgamation_matches_the_provenance_record` on the new tags
  (red until the forks are tagged and the manifest moves); new
  `engine_release_is_at_least_2_7_1` — a floor (SQLite3MC ≥ 2.7.1, SQLite
  ≥ 3.54.1), red on 2.5.1, independent of the equality `PROVENANCE.txt`
  pins; the provenance pins go red with the new record and green with the
  files.
- `step_91_wasm_build_glue`: `the_forks_sys_crate_carries_the_wasm_arm` on
  the new tag; the 2.5.1-written fixture stays as the cross-release pin.
- `third_party_notices`: the outside-graph check follows the new record
  (libsodium named in both files).

## Phase 1 — decisions

- **(a) Tag names.** rusqlite `v0.37.0-sqlite3mc-2.7.1` (the fork's own
  scheme `v<rusqlite>-sqlite3mc-<mc>`); refinery `v0.9.1-sqlite3mc-2.7.1`
  (the engine release joins the name, as a refinery tag now exists per
  engine release); openmls `openmls-v0.9.2-schwarzdigits.2` (the branch's
  counter, as Step 142's `.1`).
- **(b) libsodium's notice.** Named in both notices files with the ISC
  text, as the SHA-2 BSD notice is. Upstream could carry the notice in the
  amalgamation itself; whether to report that is Volker's call (the
  organisation's SQLite3MultipleCiphers fork is the staging area for
  upstream pull requests, open.md §82).
- **(c) The fork's `sqlite-wasm-rs` requirement** moves from `0.6.1` to
  `0.6.3` in the same fork commit: the version the lockfile resolves and the
  one whose `build.rs` was verified to honour `SQLITE_WASM_RS_SOURCE_DIR`.

## Numbers

observations §109, LEARNINGS 5.107 (retrospective). No ADR: ADR-0021 and
ADR-0042 stand; the release moves inside their rules.
