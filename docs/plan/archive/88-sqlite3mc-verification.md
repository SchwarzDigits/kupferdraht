# Step 88 — SQLite3MC engine verification (decision evidence, not a swap)

**Status: MERGED 2026-09-17** — verdict **go** (P1–P4 PASS, P5
partial: no iOS SDK on the dev Mac); ADR-0021 (SQLite3MC,
ChaCha20-Poly1305) accepted 2026-09-16, ADR-0013 superseded;
harness + P4 patch live in `spike/sqlite3mc/`; merged to `main` as
`d4f0539` (worktree instance). The swap itself is Step 90.
Originally commissioned 2026-09-15 (Volker). Purpose: turn the
"nothing fundamental against SQLite3MC anymore" assessment into
**verified evidence**, one module per open point. This step does
NOT swap the engine — its deliverable is a verdict table plus a
drafted supersession ADR (via `new-adr`) if the verdict is "go",
or a documented re-affirmation entry in ADR-0013 if it is "stay".

## Context shift since ADR-0013 (2026-06-10)

- ADR-0013's driver 1 (plaintext header never engaged) is fixed
  upstream (SQLite3MC v2.3.4, issue #233).
- Volker + Markus are now main contributors to SQLite3MC and run
  it in production in their other main project — driver 3
  (unmaintained vendor patch) inverts into source-level control;
  remaining bugs can be fixed upstream by us.
- Migration cost is zero by declaration: Kupferdraht runs test-only,
  all existing chats/DBs are disposable (the Step-76 fixtures pin
  the openmls storage format, not the cipher — `keyed.db` gets
  regenerated once by its guarded generator).
- The win on a swap: vendored OpenSSL leaves every native build
  (weight, 20d cross-compile), wasm perspective opens (SQLite3MC
  ships wasm builds), and the crypto layer sits under our own
  contributorship.

## Inputs required from Volker (phase 1)

- **I1**: repo/path of the working SQLite3MC integration in the
  other main project (the template — we adopt, not reinvent).
- **I2**: cipher preference, if any (ChaCha20-Poly1305 default vs.
  SQLCipher-compatible scheme) — otherwise P3 decides on BSI
  grounds.

## Verification program

Each module states GOAL / METHOD / PASS. The harnesses are written
as tests on the spike branch — on a "go" they become the
regression pins of the swap step, not throwaway code.

### P1 — the §52 `PRAGMA key` hang (unverified, never root-caused)

- GOAL: prove `PRAGMA key` returns promptly on SQLite3MC 2.5.x in
  DEBUG builds, for BOTH cipher schemes.
- METHOD: minimal rusqlite harness against the I1-template build;
  raw-key `PRAGMA key = "x'…'"` on fresh DBs, sqlcipher scheme and
  chacha20 scheme, debug profile (the §52 trigger), timing pin.
- PASS: < 1 s in both. FAIL path: root-cause + upstream fix (the
  contributor route Volker named) — the step then pauses on the
  upstream release, not on speculation.

### P2 — plaintext header + external salt, byte-verified

- GOAL: our exact production pragma pattern works: raw 32-byte
  key, `plaintext_header_size = 32`, salt supplied EXTERNALLY —
  and the salt does NOT leak into the plaintext header (the #233
  thread observed salt-in-header behavior; SQLCipher keeps it out,
  which is why `cipher_salt` exists at all).
- METHOD: hexdump pin — first 16 bytes read "SQLite format 3",
  bytes 16–32 carry the WAL/page metadata, NO salt bytes present,
  page 1 payload is ciphertext; wrong key fails to open; reopen
  with key+salt succeeds.
- PASS: all five assertions. Bonus (non-gating, migration is free
  anyway): SQLite3MC's sqlcipher-compat mode opens our existing
  SQLCipher `keyed.db` fixture.

### P3 — cipher choice defensible against BSI TR-02102

- GOAL: a cipher recommendation with citations, ready for the ADR.
- METHOD: paper check of the CURRENT TR-02102-1 against SQLite3MC's
  schemes (ChaCha20-Poly1305, sqlcipher/AES-256-CBC+HMAC, AEGIS,
  Ascon): which are listed/recommended, with which parameters;
  plus the audit narrative ("main contributorship = fix
  sovereignty") as an ADR building block. I2 overrides taste, not
  BSI findings.
- PASS: one recommended scheme with citation; the ADR draft's
  security section writes itself from this.

### P4 — integration cost, measured not estimated

- GOAL: the I1 template builds THIS workspace: `[patch.crates-io]`
  on `libsqlite3-sys` against rusqlite 0.37 + refinery 0.9 +
  openmls_sqlite_storage 0.3, unified across kupferdraht.db, global.db
  AND mls.db.
- METHOD: spike branch, template adopted, full 70-target suite.
- PASS: suite green with zero test edits (pragma differences
  excepted — those are the swap step's work, list them). Also
  deliver: the maintenance recipe ("what to do on the next
  rusqlite/SQLite bump"), one paragraph, honest.

### P5 — iOS header behavior, bounded honestly

- GOAL: the strongest evidence available without iOS CI.
- METHOD: the P2 hexdump IS the mechanism iOS depends on
  (readable header magic + WAL bytes); additionally attempt an
  iOS-target build via the existing 20a–c XCFramework path
  (compile-only counts). Simulator run only if cheap.
- PASS: P2 green + iOS target compiles. Residual (stated in the
  ADR, not hidden): runtime behavior on-device stays unverified
  until 20d/e — same class as every other iOS claim we make.

## Timebox and exit

- Timebox: ~2 days. P1+P2 first (the two hard unknowns); a FAIL
  there without a near upstream fix ends the step early with a
  "stay, revisit on release X" verdict.
- Exit "go": verdict table green → run `new-adr` (supersedes
  ADR-0013; cipher per P3; scope of the swap step sketched) — the
  swap itself is the NEXT step, with the P-harnesses as its pins.
- Exit "stay": ADR-0013 Revision History entry recording the
  re-examination with the verdict table linked.

## Verdict (2026-09-15, spike branch `step/88-sqlite3mc-verification`)

Evidence lives in `spike/sqlite3mc/` (its own cargo root, not a
workspace member — run `./fetch-amalgamation.sh` once, then
`cargo test -- --nocapture` from that directory). Engine under test:
SQLite3MC **2.5.1** (SQLite 3.53.4, release 2026-08-27, amalgamation
pinned by SHA-256 from the signed release sums) through rusqlite 0.37 /
libsqlite3-sys 0.35.0 — the workspace's own line. I1 (the other
project's template) was not available in this window, so P1/P2 ran on a
self-built stand-in (`spike/sqlite3mc/README.md` explains the mechanism);
P4 waits for I1 by order.

| Module | Verdict | Evidence |
|---|---|---|
| P1 — §52 `PRAGMA key` hang | **PASS** | Debug profile (`cc` at `-O0`), raw key, fresh DB: `PRAGMA key` 0.5–2.3 ms, first write ≤ 1.1 ms, reopen + read ≤ 0.8 ms — sqlcipher AND chacha20 (`p1_pragma_key_timing.rs`). The exact §52 configuration (compile-time `CODEC_TYPE_SQLCIPHER`, no `PRAGMA cipher`; run via `LIBSQLITE3_FLAGS`): key + first write 1.5 ms. Informational: the passphrase PBKDF2 path at `-O0` is 245–249 ms (sqlcipher, 256 000 × SHA-512) / 49 ms (chacha20). The hang does not reproduce on 2.5.1 in any configuration; its 2.3.5 root cause stays unknown. **Decider's call (Volker, 2026-09-16): §52 is closed on that evidence — no bisect, not a consideration for the engine decision any more.** |
| P2 — plaintext header + external salt | **PASS** (5/5 both schemes) | `p2_plaintext_header_salt.rs`: bytes 0..16 = `SQLite format 3\0`; 16..32 = page size 4096 / WAL 2,2 / reserved 0x50 (sqlcipher: 16 IV + 64 HMAC) resp. 0x20 (chacha20) / 64,32,32; the 16-byte salt occurs nowhere in the file; schema SQL + row are ciphertext; wrong key fails; key + salt reopens. Controls: header encrypted without the pragma (engine engaged); Kupferdraht's SQLCipher batch is rejected on SQLite3MC ("Cipher salt can't be changed" — salt/params go BEFORE `PRAGMA key`); the 96-hex `x'<key><salt>'` form is interchangeable with `cipher_salt`. **Bonus PASS:** the Step-76 SQLCipher fixture `keyed.db` opens in the sqlcipher scheme in non-legacy AND `legacy = 4` mode (10 tables, `integrity_check` ok) — byte-compatible, no re-encryption needed. Finding: under chacha20 + raw key the salt is inert (right key + wrong salt opens). |
| P3 — cipher vs. BSI TR-02102-1 | **PASS → sqlcipher scheme** | TR-02102-1 version 2026-01 (2026-01-23): Table 3.1 AES only ("does not recommend any other block ciphers besides AES"); Table 3.2 CBC recommended, "only unpredictable initialisation vectors" (B.2: random IVs — SQLCipher's per-page random IV); Table 4.1 SHA-512 listed, Remark 4.2 SHA-1 "should also be avoided" even inside HMAC (rules out SQLCipher legacy ≤ 3); Tables 5.1/5.2 HMAC, key ≥ 128 bit, tag ≥ 128 bit (SQLCipher: 256-bit derived HMAC key, 512-bit tag); Remark A.1 / §3.3 "Encrypt-then-MAC" (SQLCipher's page construction). §3.2: "no dedicated stream ciphers are recommended" — ChaCha20, Poly1305, AEGIS and Ascon have **zero** mentions in TR-02102-1 and TR-02102-2; §52's "ChaCha20 is TR-02102-conformant" was wrong. Honest residual: the HMAC subkey derivation (PBKDF2-SHA512 × 2 over raw key + salt) is MAC-based but not B.1.1's Two-Step KDF/HKDF construction — inherited from the SQLCipher 4 format. **Decider's call (Volker, 2026-09-16, I2): chacha20** — taken with this finding on the table; ADR-0021 records the BSI gap as an accepted deviation, the sqlcipher scheme stays compiled in as the documented fallback. |
| P4 — integration cost via I1 template | **PASS** (2026-09-16, I1 = the sister project's SQLite3MC store) | The template: vendored 2.4.0 amalgamation with a SHA-256 `PROVENANCE.txt`, a crate `build.rs` compiling it via `cc`, libsqlite3-sys on its non-bundled path (`SQLITE3_STATIC` + `SQLITE3_LIB_DIR` in `.cargo/config.toml`), ChaCha20 raw key without salt, `plaintext_header_size` before the key for iOS. Its **link layer does not transfer**: `openmls_sqlite_storage 0.3.0` declares `rusqlite 0.37` with `bundled`, which feature unification forces workspace-wide — the applicable mechanism is the `[patch.crates-io]` stand-in (what the plan assumed; also the deterministic one, observations §77). Adopted on the workspace (`spike/sqlite3mc/p4-workspace-adoption.patch`: root patch, `bundled` instead of `bundled-sqlcipher-vendored-openssl`, `apply_whole_db_key` in SQLite3MC order, engine test on `sqlite3mc_version()`, one mirrored test batch): **70/70 targets green, 0 warnings, clippy, fmt, deny green; `openssl-src` gone from the tree; the Step-76 SQLCipher fixture opens through the production path unchanged.** Zero test edits beyond the two pragma differences. Same run on chacha20: 68/70 — `fixture_081_sqlcipher_mls_db_reopens` (format) and `right_key_wrong_salt_cannot_read_data` (salt inert), both scheme-semantic; the fixture converts in place by `PRAGMA rekey` (harness bonus 2). Maintenance recipe below. |
| P5 — iOS header behaviour | **PARTIAL** (P2 green; compile BLOCKED on environment) | The P2 hexdump IS the iOS mechanism (readable magic + WAL bytes) — green. The `aarch64-apple-ios` compile of the spike fails before any C is compiled: `xcrun: error: SDK "iphoneos" cannot be located` — this machine has CommandLineTools only, no Xcode.app; the existing SQLCipher path fails identically here (every `cc` build needs the SDK). Static evidence instead: the amalgamation's platform includes are vanilla SQLite's plus arch intrinsics with C fallbacks (`arm_neon.h`, `immintrin.h`), Apple RNG via `__has_include(<CommonCrypto/CommonRandom.h>)`, zlib/ICU guarded off; SQLite3MC ships official Android AAR + wasm builds; the vendored OpenSSL build — the heavy part of the current iOS cross-compile — disappears. Residual as the plan states: on-device runtime unverified until 20d/e. |

**Maintenance recipe** (from the template and the stand-in, honest):
(1) an SQLite3MC release → bump version + SHA-256 in
`fetch-amalgamation.sh` from the signed `SHA256SUMS` (the sister project does the
same by hand: replace `vendor/sqlite3mc/*` + `PROVENANCE.txt`), rerun the
P1/P2 pins and the suite; (2) a rusqlite/libsqlite3-sys bump → copy the
new libsqlite3-sys `src/` + `sqlite3/bindgen_bundled_version.rs` into
the stand-in, bump its version string, re-check its `build.rs` against
upstream's `bundled` flag set — Kupferdraht stays on rusqlite 0.37 until
openmls_sqlite_storage moves (it pins 0.37; refinery allows ≤ 0.38);
(3) never pass `SQLITE_EXTRA_INIT`; keep the engine-identity test. The
standing cost is a ~200-line vendored crate stub that tracks upstream
plus a 13 MB build input — and no OpenSSL anywhere.

**Recommendation: go — scheme per I2: ChaCha20-Poly1305. ADR-0021 Accepted 2026-09-16 (Volker), ADR-0013 Superseded; the swap is the next step.** All five modules have evidence: the two hard
unknowns (P1, P2) are closed with byte-level pins, P3 settles the cipher
on BSI grounds, P4 shows the workspace green on the engine with a
head-start patch, and the format is byte-compatible with what ships
today. P5's residual (on-device iOS) is the plan's own stated bound. ADR-0021 is drafted as **Proposed** (`new-adr` mechanics run;
supersession of ADR-0013 deliberately NOT applied — Volker decides);
ADR-0013 carries a Revision-History note and stays binding.

Gate on the branch: `cargo fmt --check` / 0 build warnings / `clippy -D
warnings` / `cargo test --workspace --no-fail-fast` / `cargo deny check`
— green (the spike is outside the workspace and additionally
fmt/clippy-pedantic clean on its own: 19 tests). One environment note:
in the sandboxed session that ran the gate, `tests/http_hardening.rs`
needed `KUPFERDRAHT_DEV_KEYSTORE=<dir>` — `login()` touches the macOS
Keychain before it validates the URL (observations §76, open.md
gap-B.26); CI on the base commit is green without it. No smoke (no
backend surface).
