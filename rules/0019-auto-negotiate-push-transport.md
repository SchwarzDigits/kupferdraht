# Auto-negotiate the push transport

Condensed from `docs/adrs/0019-auto-negotiate-push-transport.md`.

- **`SyncTransport::WebSocket` means "push, auto-negotiated"**:
  `server_supports_push_events()` true → `/v14/events` (ADR-0017
  rules apply), false → legacy `/await`. Exactly two enum variants;
  `Polling` stays the default and the wasm-only path.
- **Legacy socket connect**: `wss://<ws-base>/await?client=<device>`
  with `Authorization: Bearer` — never the token in the URL.
- **Frame handling and the reconnect order: superseded by ADR-0030**
  (2026-09-28) — current rule: `rules/0030-legacy-push-socket-is-a-doorbell.md`
  (connect first, then drain; a durable frame only triggers a drain;
  only the drain advances the cursor). **Don't** go back to advancing
  the cursor from a socket frame. There are no acks — never invent one.
- **Client-side pings** keep cannon's idle timeout at bay; reconnect =
  the ADR-0030 connect-then-drain cycle, paced by ADR-0039 (at most one
  connect per pause, a drain without a socket) — the capped exponential
  backoff is superseded (`rules/0039-…`).
- **Don't** add a third `SyncTransport` variant, put the token in
  the socket URL, or skip the reconnect drain (the socket alone has
  no redelivery).

- **Superseded on the polling default** by `rules/0033-push-default-where-a-push-transport-exists.md` (ADR-0033, accepted 2026-09-29): push is the default wherever a target has a push transport; the wasm-only polling path stands.
