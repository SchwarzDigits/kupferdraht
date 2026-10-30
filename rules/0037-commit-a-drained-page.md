# Commit a drained page, not each notification

Condensed from `docs/adrs/0037-commit-a-drained-page.md`.

Supersedes decision item 1 of
`rules/0032-commit-mls-state-after-persisting-notification.md` (one window
per notification); its other items stay in force, per page.

- **One window per page, both databases:** take the MLS write gate, open
  the `mls.db` transaction and the `kupferdraht.db` page transaction, dispatch
  the page, then commit `kupferdraht.db` → commit `mls.db` → advance the cursor
  in a write of its own. **Don't** put the cursor into the data
  transaction.
- **A savepoint per notification** in both databases: a failure to persist
  or a deferral rolls back to it, everything before commits, the cursor
  stops before it. A failed commit rolls back the whole page.
- **Other writers of `kupferdraht.db` wait for the page;** the drain's own
  writes join it; reads never wait. Every write method of the store takes
  the page gate. Lock order: MLS gate, then page — never the other way.
- **Emit notifications to the embedder after the page commits;** a
  rolled-back part emits nothing.
- **Push frames are pages of one;** don't split the backend's page.
- **Page grouping is a `Store` capability** with a do-nothing default.
