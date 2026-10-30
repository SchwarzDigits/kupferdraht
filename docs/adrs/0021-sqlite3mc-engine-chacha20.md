# ADR-0021: Adopt SQLite3MC with ChaCha20-Poly1305 as the whole-DB encryption engine

**Status:** Accepted
**Date:** 2026-09-16
**Deciders:** Volker Zöpfel

---

## Context

This ADR proposes to supersede ADR-0013 "SQLCipher as the whole-DB
encryption engine". The rationale for changing course is that all three
drivers ADR-0013 rested on have inverted since 2026-06-10, and Step 88
(`docs/plan/archive/88-sqlite3mc-verification.md`, verdict table there) turned
the resulting "nothing fundamental against SQLite3MC" assessment into
measured evidence rather than assumption.

ADR-0013's drivers, then and now:

1. **iOS plaintext header.** In June the header pragma "provably did not
   engage" on SQLite3MC. That was upstream issue #233 (the sqlcipher
   scheme ignored `plaintext_header_size` in non-legacy mode), fixed in
   SQLite3MC 2.3.4 (2026-05-06); the chacha20 scheme was never affected.
   Step 88 P2 verifies at the byte level on 2.5.1: readable magic + WAL
   bytes, page payload ciphertext, wrong key rejected, key reopens — for
   both schemes.
2. **Audit defensibility.** ADR-0013 weighed "SQLCipher's AES-256-CBC +
   HMAC-SHA512" against "SQLite3MC's ChaCha20 on a self-maintained
   patch". The patch is gone (signed upstream releases, contributor
   access), and the cipher question was re-examined against the current
   BSI TR-02102-1 in Step 88 P3 — see Option B/C and the Decision.
3. **Maintenance.** The June patch was a hand-vendored one-off. Volker and
   Markus are now main contributors of SQLite3MC and run it in production
   in a sister project (the Step-88 I1 template): upstream bugs get fixed by us, releases are signed
   (`SHA256SUMS.sig`), and the 2.5.1 amalgamation no longer needs the
   `SQLITE_EXTRA_INIT` hook CHANGELOG §51 had to add by hand.

Additionally, the CHANGELOG §52 hang (`PRAGMA key` never returning on the
SQLCipher scheme in debug builds, 2.3.5) was the reason ChaCha20 was
chosen in June at all, and it was never root-caused. Step 88 P1 could not
reproduce it on 2.5.1 in the debug profile in any configuration —
including the exact §52 setup (compile-time `CODEC_TYPE_SQLCIPHER`, no
per-connection cipher pragma): `PRAGMA key` returns in about half a
millisecond. The decider closed §52 on that evidence (2026-09-16).

Constraints in effect: the dependency graph must stay permissive-only
(ADR-0010, ADR-0016 — SQLite3MC is MIT, SQLite public domain); the
`libsqlite3-sys` line is unified across `kupferdraht.db`, `mls.db`
(`openmls_sqlite_storage`) and `global.db` (rusqlite 0.37 / refinery 0.9
/ openmls_sqlite_storage 0.3 all resolve to 0.35.0), and
`openmls_sqlite_storage` forces the `bundled` feature workspace-wide;
migration cost is zero by declaration (Kupferdraht is test-only, all
databases disposable); the sister project already runs
SQLite3MC's ChaCha20-Poly1305 scheme with a raw key in production.

## Options Considered

### Option A: Stay on SQLCipher (Zetetic) with vendored OpenSSL — ADR-0013 as is

**Pros:**
- Zero work; the industry-standard name on the tin.
- The §52 record is moot; nothing to re-verify.

**Cons:**
- Vendored OpenSSL (`openssl-src` 3.6.2) in every native build: build
  weight, the Step-20d cross-compile burden, and a closed door for wasm
  (ADR-0013 recorded both as accepted costs).
- The crypto engine sits outside our maintainership; fixes wait on
  Zetetic.

### Option B: SQLite3MC, sqlcipher scheme (AES-256-CBC + HMAC-SHA512)

**Pros:**
- Same primitives and same on-disk format as SQLCipher 4 — existing
  databases open unchanged (verified on the Step-76 fixture in both
  non-legacy and `legacy = 4` mode); Step 88 P4: 70/70 targets green
  with zero scheme-related test changes.
- Every primitive is on BSI TR-02102-1 2026-01's lists: AES (Table 3.1),
  CBC with an unpredictable IV (Table 3.2 + B.2), HMAC over SHA-512
  (Tables 4.1, 5.1, 5.2), Encrypt-then-MAC (Remark A.1, §3.3). Step 88
  P3's recommendation on BSI grounds.
- Salt stays load-bearing (HMAC subkey derives from it), so `DbKey`
  and the `right_key_wrong_salt` pin carry over unchanged.

**Cons:**
- The pragma contract differs from SQLCipher's (parameters and salt
  before `PRAGMA key`; `cipher_salt` as bare hex; no `cipher_version`).
- 80 reserved bytes per page (16 IV + 64 HMAC) against chacha20's 32; the
  slowest of the measured schemes (still sub-millisecond per key).
- Diverges from the sister project's production configuration — two scheme
  configurations to keep in mind across the sister projects.

### Option C: SQLite3MC, ChaCha20-Poly1305 default scheme (sqleet format)

**Pros:**
- SQLite3MC's compile-time default; the scheme the sister project runs in
  production, so one configuration across the sister projects and the
  I1 template's runtime pattern applies as is (raw key, header pragma
  before the key, key verified by a first read).
- A modern AEAD (RFC 8439; TLS 1.3, WireGuard, age) with a 16-byte
  Poly1305 tag per page; the fastest scheme in the P1 measurements; the
  plaintext header worked here even before #233 was fixed.
- With a raw key there is no KDF and no salt in the trust path at all —
  the 256-bit key is the whole security parameter; `DbKey` loses its
  salt half and the keystore contract simplifies.

**Cons:**
- Not in BSI TR-02102-1 2026-01: §3.2 "no dedicated stream ciphers are
  recommended, but AES in counter mode can be considered a stream
  cipher"; ChaCha20/Poly1305 have zero mentions in TR-02102-1 and
  TR-02102-2. A TR-02102 checklist cannot tick this scheme; its defence
  is "state of the art", not "listed". (§52's "TR-02102-conformant"
  claim was wrong.)
- A format change: measured on the workspace, the chacha20 variant fails
  exactly two scheme-semantic tests (`fixture_081_sqlcipher_mls_db_reopens`,
  `right_key_wrong_salt_cannot_read_data`); the Step-76 fixture converts
  in place by `PRAGMA cipher = 'chacha20'; PRAGMA rekey` (Step 88 harness
  bonus 2), the salt pin is retired with the salt.

### Option D: SQLite3MC, AEGIS or Ascon-128 schemes

**Pros:**
- Modern AEADs (AEGIS: NEON/AES-NI fast paths; Ascon: NIST lightweight
  standard); plaintext header supported.

**Cons:**
- Neither appears in TR-02102-1 2026-01 — the same gap as Option C
  without its deployment track record, and not what the sister project runs.

## Decision

**Option C: SQLite3MC 2.5.x with its ChaCha20-Poly1305 scheme**, raw
32-byte key, `plaintext_header_size = 32` — the decider's call on plan
input I2 (2026-09-16), taken with Step 88 P3's finding on the table: the
scheme is not on BSI TR-02102-1's lists, and this ADR records that as an
accepted, deliberate deviation rather than hiding it. What carries the
decision: one production configuration across Kupferdraht and the sister project (the
I1 template applies unchanged), the simplest trust path (a 256-bit raw
key, no KDF, no salt, no HMAC subkey derivation), a modern AEAD with a
per-page authentication tag, and the measurable wins of the engine
change itself — vendored OpenSSL leaves every native build, the C build
is self-contained for every cross-compile, the wasm perspective opens
(SQLite3MC ships wasm builds; the sister project already runs it over OPFS), and
fix sovereignty through our own contributorship.

The sqlcipher scheme (Option B) remains compiled in and byte-compatible
with today's files; switching to it later is a per-connection `PRAGMA
cipher` plus a rewrite of the databases — should a BSI-conformance
requirement ever materialise, that is the documented path, not a new
engine decision.

## Consequences

### Positive

- `openssl-src` leaves the dependency tree; native and cross builds
  shrink to "compile one C file" (Step 88 P4: 70/70 targets, clippy,
  fmt, deny green on the engine).
- One at-rest configuration shared with the sister project; the runtime pattern
  and vendoring/provenance practice come from a production template.
- The trust path loses the salt and the HMAC-subkey derivation: `DbKey`
  becomes the 32-byte key, the keystore blob simplifies accordingly.
- Bugs in the engine are ours to fix upstream; releases are signed.
- The Step-88 harness (`spike/sqlite3mc/`) becomes the swap's
  regression pins (`PRAGMA key` < 1 s in debug; header/salt byte
  assertions; the chacha20 pins: salt inert, combined raw-key form,
  compile-time-default probe).

### Negative

- **BSI TR-02102-1 does not list the scheme.** An auditor working from
  that guideline cannot tick "recommended cipher"; the defence is
  RFC 8439 / TLS 1.3 / WireGuard state of the art plus the compiled-in
  Option-B fallback. This is the decider's accepted trade (I2); it must
  stay visible in THREAT_MODEL G3 and in the docs, never be papered over
  with the §52 wording.
- A format change: every existing database and the Step-76 `keyed.db`
  fixture are rewritten once (rekey in place; test-only installs).
  `right_key_wrong_salt_cannot_read_data` and the salt half of `DbKey`
  are retired — a keystore-material change for every consumer that
  holds a 48-byte blob (test-only today; the swap step accepts the
  legacy 48-byte form by taking its key half so nothing has to be
  re-provisioned).
- `apply_whole_db_key` and the engine-identity test change (pragma order,
  `sqlite3mc_version()`); every consumer doc that names the engine and
  the cipher changes with them (ADR-0013's freshness map: ARCHITECTURE
  §6.2/§7, library.md, THREAT_MODEL G3, README, bindings/cli notes).
- Build glue is a standing maintenance item: a rusqlite bump means
  refreshing the `libsqlite3-sys` stand-in (its `src/` and pre-generated
  bindings); an SQLite3MC release means re-pinning version + SHA-256.
  The I1 template's link layer (libsqlite3-sys's non-bundled path + a
  crate `build.rs`) is not usable here — `openmls_sqlite_storage` 0.3
  forces `bundled` — so the `[patch.crates-io]` stand-in is ours to keep
  (observations §77).
- P5's on-device iOS runtime behaviour stays unverified until 20d/e, as
  for every other iOS claim; the iOS compile could not be attempted on
  the drafting machine (no Xcode SDK) — an environment gap, not an
  engine finding.
- "Main contributorship" cuts both ways: the engine's security posture is
  now also our responsibility to audit.

### Neutral

- The scheme is SQLite3MC's compile-time default, so `PRAGMA cipher` is
  technically optional; the swap sets it explicitly anyway (the P1
  compile-time-default probe pins the default, the explicit pragma pins
  the intent).
- The sqlcipher scheme stays available for experiments and as the
  documented fallback; it is not offered to consumers.

## Implementation Notes

For the swap step (the NEXT step, not this ADR):

- **Build glue:** `[patch.crates-io] libsqlite3-sys = { path = … }` to a
  stand-in that keeps upstream 0.35.0's `src/` + pre-generated bindings
  and bundles the SQLite3MC amalgamation as `sqlite3/sqlite3.c` with
  upstream's `bundled` flag set plus `-DSQLITE_TEMP_STORE=2`. Do **not**
  pass `-DSQLITE_EXTRA_INIT=sqlite3mc_initialize` on 2.5.x — the
  amalgamation's `sqlite3_initialize` already calls it. The I1 template
  (the sister project) contributes the vendoring/provenance practice and the
  runtime pattern, not its link layer (observations §77);
  `spike/sqlite3mc/vendor/libsqlite3-sys/` is the working reference and
  `spike/sqlite3mc/p4-workspace-adoption.patch` the measured head start
  (its `P4_CIPHER_SCHEME` knob becomes the fixed `chacha20`). Pin the
  amalgamation by version and SHA-256 from the signed release sums
  (`fetch-amalgamation.sh`).
- **Keying batch** (replaces ADR-0013's):
  `PRAGMA cipher = 'chacha20'; PRAGMA plaintext_header_size = 32;
  PRAGMA key = "x'<64 hex>'";` — parameters BEFORE the key; no
  `cipher_salt`. Verify the key by a first read (`SELECT count(*) FROM
  sqlite_master`), as the template does, so a wrong key fails at open.
- **Key material:** `DbKey` becomes the 32-byte key; the keystore
  boundary (`from_keystore_bytes`, the single invariant site) accepts 32
  bytes and, for one transition, a legacy 48-byte blob by its key half.
  Zeroization unchanged (ADR-0006 / THREAT_MODEL G4).
- **Engine identity pin:** `SELECT sqlite3mc_version()` replaces
  `PRAGMA cipher_version`; add an at-rest ciphertext control (header
  without the pragma is encrypted) so a mis-linked SQLite is a red test.
- **Tests:** `whole_db_encryption.rs` minus `right_key_wrong_salt`;
  the Step-76 fixtures — `keyed.db` converted once in place
  (`PRAGMA cipher = 'chacha20'; PRAGMA rekey = "x'<fixture key>'"`
  after opening in the sqlcipher scheme; the guarded 0.8.1 generator is
  not needed), `plain.db` untouched; the Step-88 harness pins moved into
  `kupferdraht-core/tests/`.
- **Verify after the swap:** `cargo tree -i openssl-src` is empty;
  `cargo deny check` green; the Android (cargo-ndk) and iOS (XCFramework
  script) cross-builds on a machine with the SDKs; THREAT_MODEL G3 and
  the doc homes updated in the same commit.

## References

- Supersedes: ADR-0013 "SQLCipher as the whole-DB encryption engine".
- Constrained by: ADR-0010 "Decouple Kupferdraht from GPL dependency stack" and
  ADR-0016 "License Kupferdraht under GPL-3.0-only" — the permissive-only
  dependency graph (SQLite3MC: MIT; SQLite: public domain).
- Related: ADR-0015 "Browser architecture" — SQLite3MC's wasm build
  (the sister project: `sqlite-wasm-rs` + OPFS `sahpool`) may later offer an
  alternative to IndexedDB per-record encryption; not decided here.
  ADR-0003 (superseded) — the original per-field design this line of
  decisions started from.
- Evidence: `docs/plan/archive/88-sqlite3mc-verification.md` (verdict table,
  maintenance recipe), `spike/sqlite3mc/` (P1/P2 harness, P4 patch —
  retired with Step 90's archive, last at `d8d46c8`; the pins live on in
  `kupferdraht-core/tests/step_90_sqlite3mc_engine.rs`, the stand-in in
  `vendor/libsqlite3-sys/`),
  `docs/observations.md` §75–§77.
- External: SQLite3MC 2.5.1 release (2026-08-27,
  `sqlite3mc-2.5.1-sqlite-3.53.4-amalgamation.zip`, sha256
  `4125f8ff…d5e0`); utelle/SQLite3MultipleCiphers issue #233; SQLite3MC
  pragma + cipher documentation (utelle.github.io/SQLite3MultipleCiphers);
  BSI TR-02102-1 "Cryptographic Mechanisms: Recommendations and Key
  Lengths", version 2026-01 (2026-01-23) — Tables 3.1, 3.2, 4.1, 5.1,
  5.2, B.1, B.2; §3.2; Remarks 3.1, 4.2, A.1; RFC 8439 (ChaCha20 and
  Poly1305 for IETF Protocols); the sister project's SQLite3MC store
  (the I1 runtime template, internal).

## Revision History

- 2026-09-15: Created (Step 88 draft, Proposed) — Decision then Option B
  (sqlcipher scheme, P3's BSI recommendation), file
  `0021-sqlite3mc-engine-sqlcipher-scheme.md`
- 2026-09-16: P4 measured against the I1 template (the sister project); I2 decided
  by Volker — Decision changed to Option C (ChaCha20-Poly1305), file
  renamed to `0021-sqlite3mc-engine-chacha20.md`; still Proposed
- 2026-09-16: Accepted (Volker); ADR-0013 set to Superseded by ADR-0021
- 2026-09-24: Step 90 (engine swap) landed on the branch. Plan input I3
  decided by Volker: **hard 32-byte key material — no acceptance of the
  legacy 48-byte `key ‖ salt` blob** (Kupferdraht is deployed nowhere, so
  there is nothing to migrate); the "read by its key half for one
  transition" default in Negative Consequences and Implementation Notes
  above is superseded by this line, the body is left as written
- 2026-10-08: Amendment, not a supersession — the sister project that
  served as the I1 template is no longer named; its repository and file
  paths are internal. The decision is untouched.
