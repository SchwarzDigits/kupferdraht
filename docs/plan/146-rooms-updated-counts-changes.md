# Step 146 — `roomsUpdated` counts the rooms that changed

**Status: commissioned 2026-11-05, rewritten 2026-11-09** (from the browser
embedder's requirements for a work-queue client, R8; the embedder agreed to
this shape). Taken with Volker on the rewrite:

- no new signal. R8 asked for a callback after the client's own room changes
  "so the workaround can go", and named `onSync` with `roomsUpdated` as an
  alternative. The embedder reads `rooms()` itself and decides when; what it
  lacks is a truthful answer to "has anything changed", and the field that
  promises one does not keep the promise;
- gone with the rewrite: the core-level room-change hook, the `RoomChange`
  payload with `removed`, and `onRoomsChanged` after room calls. The file's
  former slug was `room-change-signal`;
- `onRoomsChanged` after `markAsRead` / `markAsUnread` (Step 138) stays: a
  private read and the hand-set flag put no event on the stream, so no pass
  tells another tab;
- R8's acceptance ("before the next sync pass") is not met and the embedder
  accepted that: the count comes with the pass the own event rings in;
- the changed meaning of a field is `BREAKING`; it joins Step 145 in 0.5.0;
- after Step 145, before Step 152.

## Problem

1. **The field does not do what it says.** `SyncSummary::rooms_updated` is
   documented as the rooms the refresh touched, "`0` when the cached list was
   already up to date" (`sync/settings.rs`). `sync_once` sets it to the number
   of all cached rooms, read before the drain (`client/sync.rs:223`), so it is
   above zero on every pass of an account with a room. The native push loops
   set it to `0` on every tick (`sync/push/mod.rs`, `drain_without_socket` and
   `drive_connection`; `sync/push/legacy.rs`, `tick`).
2. **Everyone reads the documented meaning.** The tabs README
   (`if (s.roomsUpdated) refreshRooms()`), `docs/bindings.md` ("`rooms()` when
   `roomsUpdated > 0`", and Step 145's "the signal to pull `messages()` again")
   and R8 itself. Step 145's test asserts `rooms_updated > 0`, which holds for
   any room; `tests/step_14b_sync_surface.rs` pins the total across two passes.
   An embedder following the docs re-reads after every pass; it cannot tell a
   pass that changed something from one that did not.
3. **Own changes do come back.** Measured on Step 145: the acting device gets
   its own `conversation.create`, `member-join`, `member-leave` and
   `conversation.rename`, on `/notifications` and as frames on its socket, at
   once. So a pass follows the call without anything new. Step 145's table has
   no `conversation.delete`.
4. **Comparing lists alone misses them.** `create_room`, `rename_room`,
   `delete_room` and `leave_room` run `sync_metadata` before they resolve, and
   the member calls update the cached members. When the echo's pass runs, the
   list has nothing left to differ.

## Scope

1. **The meaning.** `rooms_updated` is the number of distinct rooms whose name
   or members changed since this client's previous summary; a room that
   appeared or went counts. Read state does not: `onMessage` and Step 138's
   `onRoomsChanged` report it, and Step 129a adds what sync applies. The type
   stays `u32`, `SyncSummary` stays `Copy`.
2. **One set, two sources** (`client/room_changes.rs`, a set of room ids on
   `Client`; `note_room_changed`, `take_rooms_updated`):
   - **Room events.** `dispatch_room_event` notes the room when its row is new:
     a redelivered event writes nothing and counts nothing.
     `dispatch_conversation_delete` notes it whatever `remove_cached_room`
     answers, which finds nothing after an own `delete_room`.
   - **The list refresh.** `refresh_rooms_if_stale` compares the list before
     and after `sync_metadata`, on what `rooms()` and `members()` show: the set
     of ids, the name and the members (not `epoch`, `group_id` or the access
     fields). That covers what no room event announces: a Welcome, a lost
     cursor, a queue drained elsewhere, another user's profile, the five-minute
     safety net (Step 118). The own calls' `sync_metadata` and a direct call by
     an embedder compare nothing.
3. **Reporting.** `sync_once` takes the count after the drain, whose cycle ends
   with the refresh. A failed pass takes nothing, so the next one reports it.
   The three push sites take it too, so a native loop stops reporting `0`.
4. **The browser and the tabs** change nothing but a comment: the field reaches
   `onSync` and `syncOnce()` as it does, and `leader.js` broadcasts it.
5. **CLI.** `sync` prints `rooms changed:` instead of `rooms synced:`.
6. **Docs:** the doc comments of `SyncSummary` and `sync_once`, and the
   `onSync` comments in `wasm/manager.rs` and `wasm/live_sync.rs`;
   `docs/bindings.md` (`syncOnce`, the room events' signal, `onSync`, and the
   recipe: `roomsUpdated > 0` → `rooms()`, `members(roomId)`; right after an
   own call, `rooms()` directly); `bindings/web/kupferdraht-tabs/README.md`;
   `docs/library.md` (`sync_once`); `docs/api-mapping-matrix-rust-sdk.md`
   (`SyncSummary`); `docs/cli.md` (`sync`); `docs/backend-spec.md` §12.3 (the
   room events a client counts, the acting device among their recipients);
   `docs/non-goals.md` (no signal after the client's own room calls: the
   embedder holds the promise and `rooms()`); `CHANGELOG.md` `[Unreleased]`,
   `BREAKING:`.

## Phase 1

- **`deleteRoom`.** Measure on the wire-server-rs stack whether the acting
  device gets its own `conversation.delete`, for a team conversation too. The
  source says so (`crates/galley/src/convs_api.rs`, read, not measured), and
  the embedder has been told this one is still to be checked. Without the echo,
  `delete_room` notes the room itself and the next pass reports it.
- **A resumed client's first pass.** Whether the list is hydrated before the
  first refresh decides what it counts: the differences, or every room. Either
  is acceptable, an embedder reads `rooms()` at the start anyway; the docs say
  which.

## Out of scope

- Which rooms changed. A list of ids would drop `Copy` from `SyncSummary`, and
  nobody asked; an embedder that wants to know compares `rooms()`.
- A native room-change listener; `setRoomAccess`, whose fields no `rooms()` row
  shows.

## TDD sketch

- Native, httpmock:
  - two passes without an event: the first counts the mocked room, the second
    `0` (`step_14b`'s last assertion turns);
  - one rename event: `1`, and `0` on the next pass; two events of one room:
    `1`; events of two rooms: `2`; a redelivered event: `0`;
  - `rename_room` resolved, then the pass with its own echo: `1` although the
    list has nothing to differ;
  - an own `delete_room` with its echo: `1`;
  - a stale cursor with a list that differs: counted; with the same list: `0`;
  - a member's `user.update` that changes the list: `1`;
  - a pass with only messages or typing: `0`, `events_dispatched` above zero;
  - a failed pass keeps the count for the next one.
- Push: a rename frame ticks `1`; the typing frame of `step_62b` stays `0`.
- `step_145`'s `a_pass_with_a_room_event…` gains the `0` of the following pass.
- Browser: no new surface; the suite runs in CI.
- Gate: the five, plus `cargo check` and `cargo clippy` for
  `wasm32-unknown-unknown` for the touched comments.
- Smoke (two devices on the wire-server-rs stack, CLI): `room-rename` on A, then
  `sync` prints `rooms changed: 1` on A and on B and `0` on the next `sync`;
  the same for an invite and a kick; a delete as phase 1 measured it.
