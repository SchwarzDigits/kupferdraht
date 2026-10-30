# Create a database key once — never replace a missing one

Condensed from `docs/adrs/0029-create-database-key-once-never-replace.md`.

Accepted 2026-09-27 (Step 96 implements it). Builds on
`rules/0024-native-cross-process-lock-per-account.md` and
`rules/0025-scope-keystore-ids-and-cookie-label-per-home.md`.

- **Generate a key only for a database file that does not exist yet**
  (`global.db`, `kupferdraht.db`, `mls.db` alike). **Don't** generate or
  store a key when the database exists and no key is found under the
  home-scoped or legacy id — fail with `KupferdrahtError::KeyUnavailable`
  naming the database, and write nothing to the keystore.
- Keep ADR-0025's legacy adoption; the check sits after it, in
  `storage/mod.rs::adopt_or_create_key`.
- **Host keystore contract:** `get_secret` returns `Err` when the store
  is locked or unavailable; `None` only when the id was never stored or
  was deleted. **Don't** implement "locked" as `None`.
- Recovery of a key that is gone for good is Step 78's purge, not an
  automatic re-key.
