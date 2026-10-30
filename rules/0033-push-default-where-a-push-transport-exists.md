# Push is the default receive transport wherever a target has one

Condensed from `docs/adrs/0033-push-default-where-a-push-transport-exists.md`.

**Accepted 2026-09-29, implemented in Step 118 (2026-10-01).** Supersedes
the polling default of `rules/0017-…` and `rules/0019-…`.

- **`SyncTransport::default()` is `WebSocket` on every target that
  compiles a push transport, `Polling` on the others** — gate the
  default on the same `cfg` as the push module, so it moves with it.
- **`sync_once` stays transport-agnostic** — a one-shot drain.
- **Don't** make the browser default to `WebSocket` before it has a
  push transport, or remove `Polling` — it stays the explicit fallback.
