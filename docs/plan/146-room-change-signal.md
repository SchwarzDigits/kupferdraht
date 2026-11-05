# Step 146 — `onRoomsChanged` after the client's own room changes

**Status: commissioned 2026-11-05** (from the browser embedder's
requirements for a work-queue client, R8, measured there against
`4f30f02` and re-checked against `main` `9e1a11c`). Taken with Volker on
commissioning:

- the signal is `onRoomsChanged`, not `onSync`: `onSync` means "a sync
  pass ran" and `roomsUpdated` counts that pass's rooms;
- its payload grows to the room's full `rooms()` row, with
  `removed: true` for a room the change took away — additive, a reader
  of today's read-state fields reads on; in `index.d.ts` the type is
  `RoomChange`, `ReadStateChange` stays as its alias;
- the calls: `createRoom`, `createDm`, `renameRoom`, `deleteRoom`,
  `invite`, `kick`, `leave`, `joinViaLink` — each fires once after it
  succeeded; `setRoomAccess` changes no `rooms()` row and fires nothing;
- after Step 145, before 129a: this step builds the core-level report
  that 129a's sync-applied read state fires as well.

Names checked against matrix-rust-sdk (`main`, 2026-11-05): the analogs
are `Room::subscribe_to_room_info_updates` with
`RoomInfoListener::call(room_info: RoomInfo)` — a full snapshot per
change — and `RoomListEntriesUpdate::Remove` from
`RoomListEntriesListener::on_update`; Kupferdraht's browser payloads end
in `…Change` (`ReadStateChange`, `DevicesChange`).

## Problem

1. **The client's own room changes are reported late.** `onSync`
   reports `roomsUpdated` after a pass; `onRoomsChanged` (Step 138)
   fires only after `markAsRead` / `markAsUnread`, from
   `report_read_state` in `wasm/manager.rs`. A room this client just
   created, renamed, deleted or changed the members of reaches the
   signal on a later pass at the earliest. The embedder raises its own
   event after each of these calls and pulls `rooms()` again.
2. **The data is there already.** `create_room`, `rename_room`,
   `delete_room` and `leave_room` run `sync_metadata()` before they
   resolve (`client/rooms.rs`, `client/membership.rs`), the member calls
   update the cached members, so `rooms()` right after the call shows
   the new state. Only the signal is missing.
3. **The tabs need nothing.** The leader holds one `startSync` per
   client and broadcasts its `onRoomsChanged` to every tab (`leader.js`
   `emit('rooms')`, `proxy.js`); a call from another tab runs on the
   leader, so a signal fired there reaches all tabs.
4. **The payload cannot say it.** `ReadStateChange` is `{ roomId,
   unreadCount, lastReadAtMs, markedUnread }`: no name for a rename,
   nothing for a room that is gone.

## Scope

1. **A core-level report.** A crate-internal room-change hook on
   `Client`: the core methods behind the eight calls (`create_room`,
   `create_dm`, `rename_room`, `delete_room`, `invite_members`,
   `kick_members`, `leave_room`, `join_via_link`) report the room once
   after they succeeded, as changed or as removed — removed when the
   cache no longer lists the room afterwards (`delete_room`, and
   `leave_room` where the room leaves the list). `mark_as_read` and
   `set_unread_flag` report through the same hook, so Step 138's
   `report_read_state` goes. A failed call reports nothing. No `pub`
   symbol; native consumers see no change. Step 129a's sync-applied read
   state reports through it as well.
2. **The browser side.** The wasm `startSync` registers the hook; it
   builds the room's `rooms()` row (Step 138's read state, now with the
   name) and calls `onRoomsChanged`; for a removed room the row as the
   cache held it before the call, plus `removed: true`. Without a
   `startSync` nothing fires and nothing fails, as today.
3. **Types.** `index.d.ts`: `RoomChange` — the `Room` row and an optional
   `removed: boolean`; `ReadStateChange` a deprecated alias of it;
   `startSync`'s `onRoomsChanged` takes a `RoomChange`.
4. **No CLI command.** The signal exists only on the browser surface;
   the CLI has no callbacks (Phase 6 noted, not skipped).
5. **Docs:** `docs/bindings.md` (`onRoomsChanged`: when it fires, the
   payload, `removed`), `bindings/web/kupferdraht-tabs/README.md` (the
   seventh callback's description), `docs/api-mapping-matrix-rust-sdk.md`
   (the Step-138 row: from "(none)" to Drift with `RoomInfoListener` and
   `RoomListEntriesUpdate::Remove`), `CHANGELOG.md` `[Unreleased]`
   (Changed: `onRoomsChanged` also after the own room changes, its
   payload carries `name` and `removed`).

## Out of scope

- A native room-change listener (FFI): nobody asked for one; the hook
  stays crate-internal and could back one later.
- Changes that come in through sync (other members' renames, removals):
  they keep `onSync`'s `roomsUpdated`; the room events of Step 145 show
  them in the timeline.
- `setRoomAccess`, the access modes and roles of a room.

## TDD sketch

- Native, httpmock: each of the eight core methods reports the room
  exactly once after it succeeded (a test-only hook registration): a
  rename with the new name, a created room and DM, an invite and a kick
  with the room unchanged, `delete_room` and `leave_room` as removed; a
  failed call (an unknown room, a backend `403`) reports nothing;
  `mark_as_read` and `set_unread_flag` report through the hook.
- Browser (wasm tests, CI): `onRoomsChanged` fires once per call with
  the `rooms()` row incl. `name`, `removed: true` after `deleteRoom` and
  `leave`; nothing without `startSync`; `markAsRead` / `markAsUnread`
  as before, now with `name`.
- Tabs: the browser scenario — `renameRoom` called in a second tab
  resolves, and the callback has fired once in both tabs before the
  next sync pass; `step_94_tab_proxy_parity` sees `RoomChange` in
  `index.d.ts`.
- Gate: the five, plus `cargo check` and `cargo clippy` for
  `wasm32-unknown-unknown` and the `ci.yml` browser test targets.
- No smoke: no backend surface (the calls' requests are unchanged); the
  browser tests run in CI.
