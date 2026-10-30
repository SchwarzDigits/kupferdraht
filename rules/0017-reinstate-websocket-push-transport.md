# Reinstate the `/v14/events` WebSocket push transport (native, opt-in)

The binding form of ADR-0017. Its ADR file was never committed (found
2026-10-07); the rationale lives in ADR-0019 §Context and
`docs/wire/api-notes.md` (`/v14/events`).

- **Push receive is opt-in via `SyncTransport::WebSocket` on the existing `sync*` family** — `Polling` stays the default and the backfill/fallback path; `sync_once` ignores the transport.
- **Ack a `/v14/events` frame only AFTER the cursor persist** — never reorder; an early ack can consume an MLS `Commit`/`Welcome` before it is durable locally.
- **Dedup redeliveries by notification `id`** (against the persisted `last_event_id`) — never by `delivery_tag`; ack the deduped frame anyway.
- **Reconnect with a fresh `sync_marker` per connection**, paced by ADR-0039 (at most one connect per pause, a drain without a socket — the capped exponential backoff is superseded, `rules/0039-…`); socket-transport errors reconnect, they never abort the loop.
- **Don't** add a second API family for push, connect the socket on wasm, or treat backend-spec §11.2–§11.5 as anything but the normative contract.

- **Superseded on the polling default** by `rules/0033-push-default-where-a-push-transport-exists.md` (ADR-0033, accepted 2026-09-29): push is the default wherever a target has a push transport.
