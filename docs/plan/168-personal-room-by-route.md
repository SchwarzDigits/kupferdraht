# Step 168 — The personal room found by its route: `GET /conversations/mls-self`

**Status: commissioned 2026-11-25** (Volker). Small; after Step 167.

## Problem

Kupferdraht finds the personal room in the rooms list (`POST /conversations/list-ids`
+ `/conversations/list`, Step 86: the room with Wire's type `1`). Wire's own clients
ask for it by its route, `GET /conversations/mls-self` (v14 and v18). The Rust server
implements that route (galley-rs creates the self conversation server-side on first
use), and nothing exercises it end to end.

## Scope

1. **`Client::personal_room` is still the local lookup.** What changes is how the
   snapshot learns of the room: the rooms refresh asks `GET /v{N}/conversations/mls-self`
   first and takes the answer as the personal room's conversation object; a `404` or any
   other error falls back to the list's type-`1` room, logged once per refresh.
2. **The `wire/` layer:** the route in `wire/http/conversations.rs`, the conversation
   object decoded like the list's entries.
3. **CLI:** `personal-room` says which way the room was found (`by route` / `from the
   list`); the smoke checks `by route`.
4. **Docs:** `docs/wire/api-notes.md` (the route, live-confirmed on both servers),
   `docs/backend-spec.md` §9.8, `docs/cli.md`, `docs/ARCHITECTURE.md` (the personal
   room paragraph), `CHANGELOG.md`.

## Out of scope

- Establishing or joining the room (Step 86, unchanged); the read position (129a) and
  the hide marker (129b).

## TDD sketch

- httpmock: the refresh calls `mls-self` once and the snapshot holds the room with the
  route's id and epoch; a `404` falls back to the list and logs; a route answer that
  disagrees with the list on the epoch takes the route's — red;
- the Step 86 tests stay green with the route mounted as `404` (their fixtures) — green
  before and after.

Smoke: `personal-room` prints `by route` on the Rust stack and on server 4 (Wire 5.38.9).
