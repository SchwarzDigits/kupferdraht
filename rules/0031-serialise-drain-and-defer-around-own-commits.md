# Serialise the drain per account; defer, don't heal, around own pending commits

Condensed from `docs/adrs/0031-serialise-drain-and-defer-around-own-commits.md`.

Accepted 2026-09-28 (Step 97d implements it). Builds on
`rules/0024-native-cross-process-lock-per-account.md`, which stays
unchanged for `kupferdraht.lock`.

- **One cursor-advancing dispatch per account at a time**, guarded by
  `base/accounts/<id>/sync.lock` (advisory file lock) plus an async
  mutex per account in process; wasm: the async mutex only.
- **Fetch without the lock**; under the lock compare the persisted cursor
  with the page's `since` — moved → refetch. **Don't** hold `sync.lock`
  across network I/O.
- **Bounded wait** for `sync.lock`, then a retryable error. **Don't**
  make it fail-fast, and **don't** reuse `kupferdraht.lock` for it.
- **Lock order:** never take `kupferdraht.lock` while holding `sync.lock`.
- **Defer, don't heal**, a next-epoch message for a group with an own
  pending commit: wait for this process's in-flight commit flow (per-group
  async lock, stage → upload → merge/clear), else stop the drain before
  it; heal only once the pending commit outlived the abandonment bound.
- **Drop, don't heal**, a message from an epoch older than the group's.
