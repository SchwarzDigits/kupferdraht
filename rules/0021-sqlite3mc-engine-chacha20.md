# SQLite3MC (ChaCha20-Poly1305) as the whole-DB encryption engine

Condensed from `docs/adrs/0021-sqlite3mc-engine-chacha20.md`.

Supersedes `rules/0013-sqlcipher-engine.md` (2026-09-16). Landed in the
tree with Step 90 (engine swap, 2026-09-24).

- **Encrypt every local database whole-file with SQLite3MC in its
  `chacha20` scheme** (ChaCha20-Poly1305, sqleet format — SQLite3MC's
  default and the sister project's production configuration). Decider's call on
  I2; the scheme is NOT on BSI TR-02102-1's lists — keep that deviation
  visible in THREAT_MODEL G3 and the docs, never restate §52's
  "conformant" claim.
- **Key material:** per-database 32-byte raw key under its own keystore
  id; no key sharing; no KDF, no salt, no HMAC subkey in the trust path.
  `DbKey` is the key alone; the legacy 48-byte `key ‖ salt` blob is
  rejected like any other length (I3 decided 2026-09-24: hard 32, no
  transition path — nothing is deployed).
- **Pragma order is SQLite3MC's:** `cipher = 'chacha20'` →
  `plaintext_header_size = 32` → `key = "x'<64 hex>'"`, then verify the
  key by a first read. Parameters BEFORE the key — never the SQLCipher
  order; no `cipher_salt`.
- **Engine identity via `SELECT sqlite3mc_version()`**, not
  `PRAGMA cipher_version`; keep an at-rest ciphertext control test.
- **Build:** `[patch.crates-io]` on `libsqlite3-sys` to a stand-in that
  bundles the amalgamation, pinned by version + SHA-256 from the signed
  release sums; upstream's `bundled` flags + `SQLITE_TEMP_STORE=2`.
  **Don't** pass `SQLITE_EXTRA_INIT=sqlite3mc_initialize` on 2.5.x.
  **Don't** reintroduce vendored OpenSSL. **Don't** use libsqlite3-sys's
  non-bundled `SQLITE3_LIB_DIR` route (`openmls_sqlite_storage` forces
  `bundled`; the route probes pkg-config first).
- **Fallback, not a free choice:** the sqlcipher scheme stays compiled
  in as the documented path should BSI conformance become a requirement
  — switching is a new decision plus a database rewrite, not a knob.
- **Don't** treat P5 (iOS on-device) as verified — an open validation
  item named in the ADR.
