# Per-field encryption — superseded

Superseded by ADR-0013, itself superseded by ADR-0021. The current
storage rule lives in `0021-sqlite3mc-engine-chacha20.md`.

- **Do not reintroduce the per-field AES-GCM DEK/KEK scheme** — whole-DB
  encryption replaced it (SQLCipher via ADR-0013, SQLite3MC via ADR-0021;
  the June-2026 SQLite3MC objection this rule once carried is retired by
  ADR-0021's evidence).

Full history: `docs/adrs/0003-per-field-encryption-instead-of-sqlcipher.md`.
