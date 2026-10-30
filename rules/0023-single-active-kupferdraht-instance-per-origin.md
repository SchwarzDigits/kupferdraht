# One active Kupferdraht instance per browser origin (Web Locks lease + takeover)

Condensed from `docs/adrs/0023-single-active-kupferdraht-instance-per-origin.md`.

Supersedes decision item 7 of `rules/0022-browser-persistence-sqlite3mc-over-opfs.md`
("single writer, no multi-tab coordination — document it, don't build
it"); everything else in ADR-0022 stays in force.

- **Take the origin-scoped Web Locks lease `kupferdraht:instance` BEFORE the
  OPFS pool is installed** (`storage::install_browser_storage`), with
  `ifAvailable`. Held elsewhere → `Kupferdraht.open` fails closed with the
  distinct, recoverable "already open in another tab" error — never a
  handle error, never a silent second instance.
- **Takeover is explicit** (`Kupferdraht.open` option) and uses `steal`; the
  evicted instance marks itself evicted (every further call fails
  recoverably) and fires `onEvicted`. The **embedder terminates the
  evicted worker** — only that releases the OPFS handles; the new
  instance retries the pool install within a bounded wait.
- **One lease per origin, not per account** (pool and `global.db` are
  per origin; the manager serves several accounts from one instance).
- **Don't** register one Wire device per tab (the origin's single cookie
  jar and the device cap forbid it), **don't** host the store in a
  `SharedWorker` (no `SyncAccessHandle` there), **don't** use a lock file
  (a lease the browser releases on tab/worker death is the point).
- Bind `navigator.locks.request` by a hand-written `wasm_bindgen` extern
  block (no `unsafe`, no `web_sys_unstable_apis` cfg) unless the step's
  orientation finds the cfg preferable.
