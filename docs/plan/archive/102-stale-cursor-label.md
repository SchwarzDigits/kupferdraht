# Step 102 — Reset the cursor only on the backend's `not-found`

**Status: MERGED 2026-09-29** — commissioned 2026-09-28 (Markus — finding
H3 of the production-readiness review, `open.md` §71). `b6d5d80`. Merged
to `main` as PR #71 (`6e4a5d1`); CI green. Smoke 2026-09-28 against
schwarz-dev-wire (release CLI). Scope shipped: the transport layer
classifies the backend's `404 not-found` to a request with `since`; only
that resets the drain's cursor. Archived with this header.

## Problem (verified in code 2026-09-28)

The notifications drain treats any `BackendRejected` whose text contains
" 404 " as a stale cursor (`sync/drain.rs`, `is_stale_cursor_404`): it
drops the persisted cursor and refetches from the queue head. A 404 from
anything between Kupferdraht and the backend — a proxy, a load balancer, a
misrouted path, an HTML error page — does the same. The drain then
replays the whole queue from its head instead of surfacing the error;
the cursor it had is gone.

The backend's answer to a `since` it no longer holds (backend-spec §11.4,
probed live 2026-09-28): `404`, `application/json`,
`{"code":404,"label":"not-found","message":"Some notifications not found"}`.

## Decisions (2026-09-28)

- **D1 — classify in the transport layer.** `fetch_notifications` returns
  a typed outcome: a page, or "the `since` id is not found" — only for a
  request that sent `since` and got a `404` whose error envelope carries
  the label `not-found`. Every other non-2xx stays `BackendRejected` as
  today.
- **D2 — the drain resets only on that outcome,** at most once per drain
  as before; the string match goes.

## TDD

- `wire/http/notifications.rs` (unit): a `404 not-found` on a request with
  `since` is the not-found outcome; a `404` with an HTML body, a `404`
  with another label, and a `404 not-found` without `since` are
  `BackendRejected`.
- `tests/step_102_stale_cursor_label.rs`: a persisted cursor and a proxy's
  HTML `404` — the drain fails, the cursor stays, no marker-less refetch
  happens. The Step 64 test (`drain_recovers_from_stale_cursor_404`) keeps
  covering the labelled reset.

**As built (2026-09-28).**
- `wire/http/notifications.rs`: `FetchedNotifications { Page, SinceNotFound }`;
  `since_not_found_or_rejected` reads a `404`'s error envelope when the
  request sent `since`. `wire/http/mod.rs`: the envelope DTO
  (`WireErrorBody`, formerly private to `auth.rs`) and `decode_response`
  (the 401 / non-2xx / decode tail of `send_and_decode`) are shared.
- `sync/drain.rs`: `fetch_page_recovering_stale_cursor` matches the
  outcome; `is_stale_cursor_404` (the string match) is gone. A second
  `not-found` in one drain is a `BackendRejected`, as before.
- Tests: 2 red → green (the labelled outcome; the proxy 404 keeps the
  cursor); 3 unit tests pin the rejections; Step 64's reset test stays
  green.
- Smoke 2026-09-28 (schwarz-dev-wire, release CLI): fresh `login`, `sync`
  (marker-less, 33 notifications), `sync` again (with `since`, 0 new),
  `logout --purge-device` — no reset, no error. A stale cursor cannot be
  forced through the CLI; the backend's answer to one was probed live
  with a raw request (above, `docs/wire/api-notes.md`).
