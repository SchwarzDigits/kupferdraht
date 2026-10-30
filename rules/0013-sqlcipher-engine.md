# SQLCipher as the whole-DB encryption engine

Condensed from `docs/adrs/0013-sqlcipher-engine.md`. **Superseded by
ADR-0021** (2026-09-16); current rule: `rules/0021-sqlite3mc-engine-chacha20.md`.

- **Don't** reintroduce SQLCipher (Zetetic) or the
  `bundled-sqlcipher-vendored-openssl` feature — the engine swap landed
  (Step 90); **don't** add SQLCipher-specific pragmas
  (`cipher_plaintext_header_size`, `cipher_salt = "x'…'"`,
  `cipher_version`) — SQLite3MC's contract is `rules/0021`'s.
- Still valid and carried into ADR-0021: one independently generated key
  per database file (`kupferdraht.db` / `mls.db` / `global.db`), no key
  sharing, raw keys, the 32-byte plaintext header for iOS.
