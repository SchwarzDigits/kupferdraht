# ADR-0013: SQLCipher as the whole-DB encryption engine

**Status:** Superseded by ADR-0021
**Date:** 2026-06-10
**Deciders:** Volker Zöpfel

## Context

Kupferdraht encrypts every local database whole-file at rest. The first engine
choice was **SQLite3MC** with its default **ChaCha20-Poly1305** cipher,
linked via a hand-vendored, patched `libsqlite3-sys` (`vendor/libsqlite3-sys/`
bundling the SQLite3MC amalgamation). That choice — and the decision *against*
SQLCipher — was recorded in ADR-0003 (originally for a per-field-DEK design,
later carried into the whole-DB move) and in `CHANGELOG.md` §52 (SQLite3MC's
SQLCipher *cipher* hung on `PRAGMA key` in debug builds, so ChaCha20 was kept).

Three drivers pushed to reconsider the engine itself:

1. **iOS.** iOS kills an app that backgrounds while holding a file lock on a
   WAL-mode SQLite DB in a shared container unless it can read the SQLite
   header (magic + WAL version bytes). This needs a plaintext database header.
   SQLite3MC's `plaintext_header_size` **provably does not engage** in our
   build — verified at the raw `rusqlite` level on a brand-new from-scratch DB,
   both with and without WAL (the header stayed encrypted).
2. **Audit.** SQLCipher (Zetetic; AES-256-CBC + HMAC-SHA512) is the industry
   standard for encrypted SQLite (Signal, WhatsApp, Wire/Kalium). It is far
   easier to defend in a security audit than SQLite3MC's ChaCha20 carried on a
   self-maintained amalgamation patch.
3. **Maintenance.** The hand-vendored `libsqlite3-sys` patch is a recurring
   upkeep cost (re-vendor on every SQLite/SQLite3MC security update).

## Decision

Replace SQLite3MC with **real SQLCipher (Zetetic)**, linked via rusqlite's
`bundled-sqlcipher-vendored-openssl` feature. The vendored `libsqlite3-sys`
patch and `[patch.crates-io]` redirect are removed.

This **reverses ADR-0003's "do not introduce SQLCipher"** for the whole-DB
engine. (ADR-0003's per-field-DEK direction had already been superseded by the
whole-DB SQLite3MC move; this ADR records the concrete engine choice.)

`libsqlite3-sys` is unified across the dependency graph, so the per-account
`kupferdraht.db` (Kupferdraht's `rusqlite`), the device-level `global.db`, **and**
`mls.db` (`openmls_sqlite_storage`) are all SQLCipher-encrypted. **Each
database file has its own independently generated key**, stored under its
own keystore id: `kupferdraht.db` under `account-<id>`, `mls.db` under
`account-<id>-mls`, and `global.db` under `global-db`. No key material is
shared between databases.

The raw-key design keeps the secret out of any KDF in the trust path: the
keystore holds 48 bytes of material per database — a 32-byte AES-256 key
fed straight into `PRAGMA key = "x'<hex>'"` plus a 16-byte salt supplied
via `PRAGMA cipher_salt` (see the iOS header note below).

## Validation (spike, 2026-06-10)

- **Builds** with the vendored OpenSSL (`openssl-src` 3.6.2) on desktop; one
  cargo feature replaces the vendored patch.
- **The §52 hang is SQLite3MC-specific and gone** — `PRAGMA key` returns
  instantly on SQLCipher (raw key bypasses PBKDF2; OpenSSL AES is fast).
- **Full suite + openmls/`mls.db` pass** on SQLCipher.
- **The iOS plaintext header works** — `PRAGMA key` then
  `PRAGMA cipher_plaintext_header_size = 32` yields a readable header — with the
  salt round-tripped via `PRAGMA cipher_salt` (retrieve on create, supply on
  reopen). This is the feature that never engaged on SQLite3MC.

## Consequences

**Positive**
- Industry-standard, auditable cipher (AES-256-CBC + HMAC-SHA512).
- The iOS WAL-mode shared-container path is unblocked (follow-up step).
- The vendored `libsqlite3-sys` patch is gone; a maintained cargo feature
  replaces it.

**Negative / accepted costs**
- SQLCipher needs an **OpenSSL crypto backend** (vendored, built from source)
  on every native target — heavier cross-builds than SQLite3MC's self-contained
  C amalgamation (which needed no external crypto). Cross-compile to Android
  (cargo-ndk) and iOS must be validated with vendored OpenSSL.
- **WASM** (already deferred) gets harder, not easier: encrypted SQLite under
  WASM remains an open, separate problem — and SQLCipher's OpenSSL requirement
  makes a future WASM build materially harder than the self-contained
  amalgamation would have.

## iOS plaintext header (shipped)

The iOS plaintext header (`cipher_plaintext_header_size = 32` + per-database
salt stored in the keystore alongside the key) shipped as a follow-up step
after this engine swap. On open, the pragma order is load-bearing: `PRAGMA
key` first, then `cipher_plaintext_header_size = 32`, then `cipher_salt`
(setting the header size before the key leaves the header encrypted). The
salt is part of the 48-byte keystore material per database, so it survives
reopens without being read from the (now plaintext) header.

## Still out of scope (follow-up)

The broader iOS WAL-mode shared-container lifecycle integration (backgrounding
while holding a file lock) builds on the plaintext header but is a separate
step.

## Revision History

- 2026-06-10: Created (Accepted)
- 2026-09-15: Re-examined in Step 88 (`docs/plan/archive/88-sqlite3mc-verification.md`,
  verdict table): driver 1 fixed upstream (SQLite3MC 2.3.4, #233), the §52
  hang not reproducible on 2.5.1, the sqlcipher scheme byte-compatible with
  this ADR's format. Supersession proposed as ADR-0021 (Proposed); this ADR
  stays Accepted and binding until that decision is taken.
- 2026-09-16: Status changed to Superseded by ADR-0021 (Adopt SQLite3MC with
  ChaCha20-Poly1305 as the whole-DB encryption engine). The engine swap itself
  is the next step; until it merges the tree still links SQLCipher.
