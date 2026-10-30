# Native cross-process lock: per account directory, fail-fast

Condensed from `docs/adrs/0024-native-cross-process-lock-per-account.md`.

Accepted 2026-09-25 (Step 85 implements it).

- **Guard exactly two critical sections** on native with the advisory
  file lock `kupferdraht.lock` (`std::fs::File::try_lock`): the token refresh
  + persist, and the first-time creation of a database key. Nothing
  else takes the lock; an existing key never does.
- **Granularity is the account directory** (`base/accounts/<id>/`), plus
  the home (`base/`) for the device-registry key only. **Never**
  machine-wide, never a location outside the home.
- **Fail fast:** a held lock is `KupferdrahtError::AccountLocked` (payload:
  scope + holder pid when known), retryable. **Don't** block, **don't**
  hold a lock for the process lifetime, **don't** add a takeover.
- **Adopt before rebinding:** under the lock re-read the persisted
  session and adopt a rotation another process persisted (newer stamp
  **and** a different access token) instead of calling `/access`;
  re-read the keystore before generating a key.
- **Every native open path** (`kupferdraht.db`, `global.db`, `mls.db`) sets
  `busy_timeout = 5000` and `synchronous = FULL` explicitly. **Don't**
  drop to `NORMAL` — the rotated `zuid` and MLS epoch advances must
  reach disk.
- **Don't** build this on wasm — ADR-0023's lease is the browser's
  answer. Two different homes on one keystore never meet since
  ADR-0025 (keystore ids carry the home id); the home lock also guards
  the creation of `base/home.id` — see `rules/0025-*`.
