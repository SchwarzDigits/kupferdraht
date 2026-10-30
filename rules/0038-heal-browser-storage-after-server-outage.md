# Heal the browser's storage after the storage server was unreachable

Condensed from `docs/adrs/0038-heal-browser-storage-after-server-outage.md`.

Accepted 2026-09-30 (Step 113 implements it). Builds on
`rules/0035-store-browser-databases-on-sqlite-remote-vfs.md` and
`rules/0032-commit-mls-state-after-persisting-notification.md`.

- **The VFS heals, not Kupferdraht:** after `Failure::Unreachable` the VFS heals a
  database in place once the storage server answers again. **Don't** close,
  reopen or swap a connection for an outage, and **don't** keep keys in
  memory for it.
- **Check both databases before each operation** (`kupferdraht.db` in
  `SqliteStore::lock()`, `mls.db` in the MLS provider) with
  `storage::check_database`: while `Unreachable`, one cheap read lets the VFS
  heal or fail — failed → `ServiceUnavailable` at once, never a block.
- **Report the state:** the account is `reconnecting` while any of its
  databases is unreachable; `client.storageState` and the sixth optional
  `startSync` callback `onStorage(state)`; the tabs leader broadcasts
  `storage`.
- **Keep the rollback journal in the browser** (never `journal_mode` MEMORY or
  OFF) and roll back after an error inside a transaction — the healing's
  guarantee that a failed commit is not committed rests on both.
- **Don't** buffer sends — they fail with `ServiceUnavailable`; the reconnect
  timeout is 3 s.

## Revision History

- 2026-09-30: Created (Accepted — Markus)
