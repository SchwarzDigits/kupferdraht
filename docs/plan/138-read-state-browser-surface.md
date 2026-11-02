# Step 138 — Read state on the browser surface: an unread flag, a read without receipt, read state in `rooms()`

**Status: commissioned 2026-11-02** (from an embedder's spec "read state on
the browser surface — mark read / unread", measured against the public
snapshot `4f30f02` of 2026-10-30; every claim re-verified on v0.1.1 the same
day). The embedder lists rooms as a work queue: a room needs attention while
it holds an inbound message its user has not seen, opening it clears that,
and the user can put a room back by hand. Whether the other members learn
of the read is the embedder's choice per call — some deployments must not
confirm reading to the other side.

## Problem

1. **The browser cannot read the read state.** `rooms()`
   (`wasm/manager.rs:967-979`, `RoomView` at `:1575`) returns
   `{ roomId, name }`. The device-local read marker and Step 67's unread
   count exist in the store (`storage/sqlite/read_markers.rs`:
   `set_read_marker` forward-only, `read_marker`, `unread_counts` — inbound,
   not redacted, newer than the marker) and natively (`Room::unread_count`,
   `Client::unread_counts`), but have no wasm projection, and no surface
   reads the marker itself.
2. **A read always confirms.** `Timeline::mark_as_read`
   (`timeline/confirmation.rs:42-74`) moves the marker, then sends
   `Confirmation { READ }` unless the account's `WIRE_RECEIPT_MODE` is off
   (ADR-0012; Wire's per-conversation `receipt_mode` is ignored by design,
   `wire/dto.rs:256`). A deployment that must not confirm reading has only
   the account switch; the embedder cannot decide per call.
   matrix-rust-sdk's `mark_as_read(ReceiptType)` has `Read` and
   `ReadPrivate` for exactly this (`docs/api-mapping-matrix-rust-sdk.md:189`
   lists the drift).
3. **No unread flag.** A room cannot be marked unread. The marker only
   moves forward (Step 67), and a room whose last message is the user's own
   would not become unread by moving it, so a flag beside the marker is the
   shape — matrix-rust-sdk's `Room::set_unread_flag` / `is_marked_unread`.
   Wire has no message for it: the flag is local to the device and
   invisible to other clients.
4. **No signal for a local change.** The leader tab broadcasts `onSync`
   after a sync pass (`wasm/live_sync.rs:138-146`, ADR-0026; `roomsUpdated`
   is the room count, `client/sync.rs:223`), and a thin tab re-pulls
   `rooms()` on it. A read with receipt wakes the loop through the Step-122
   sent signal (`client/http_calls.rs:31`); a read without receipt or an
   unread flag uploads nothing and reaches no other tab.

## Decisions (phase 1, Volker)

1. **The core signature.** `Timeline::mark_as_read(item_id, receipt:
   ReceiptType)` with `Read` / `ReadPrivate` after matrix-rust-sdk —
   BREAKING for Rust, FFI, Kotlin and Swift (minor bump, CHANGELOG "Release
   process") — or a second method beside today's. Recommended: the matrix
   shape and the bump; the wasm call stays compatible through its
   `options`.
2. **The signal.** A dedicated `onRoomsChanged` callback of `startSync`,
   broadcast by the leader like `devices` and `storage` (`leader.js:139`),
   or the sent signal raised on a local read-state change (reuses Step 122,
   but drains the backend for a local flag). Recommended: the callback.
3. **Order against Step 129.** Before it: 129's `LastRead` then goes out on
   a private read too (it reaches the user's own devices, not the peers) —
   one line in 129's scope.

## Scope

1. **Store.** Migration V31: `marked_unread INTEGER NOT NULL DEFAULT 0` on
   `room_read_markers`; the `set_read_marker` upsert clears it, a new
   `set_unread_flag` sets it; `is_marked_unread` beside `read_marker`, with
   default bodies for custom stores (`storage/records.rs:731-764`). The
   browser store runs the same migrations.
2. **Core.** `Room::read_marker()`, `Room::set_unread_flag(bool)` and
   `Room::is_marked_unread()` (new pub, mapping rows); `mark_as_read` per
   decision 1: `ReadPrivate` moves the marker and clears the flag and sends
   nothing, `Read` as today; the receipt-mode gate stays; an item unknown
   locally moves no marker and still clears the flag — the caller's "this
   room is read" holds.
3. **wasm.** `RoomView` grows `unreadCount`, `lastReadAtMs` (`null` when
   never marked) and `markedUnread` — one grouped `unread_counts` query per
   `rooms()` call; `markAsRead(roomId, itemId, options?)` with
   `sendReceipt` defaulting to `true`; `markAsUnread(roomId)` new,
   idempotent, `InvalidInput` for a room the client does not know; the
   signal of decision 2 fires after both.
4. **Tabs.** `protocol.js` (`markAsUnread` as `mutation`), `proxy.js`,
   `index.d.ts` (`Room`, `markAsRead`, the new callback);
   `tests/step_94_tab_proxy_parity.rs` pins the table.
5. **FFI and CLI.** The `Room` and `Timeline` mirrors; `mark-read
   --no-receipt`, `mark-unread --id`; `docs/cli.md`.
6. **Docs.** `docs/library.md`, `docs/bindings.md`, the api-mapping (the
   `mark_as_read` row from Drift toward Aligned, three new rows),
   CHANGELOG (`BREAKING:` when decision 1 takes the matrix shape).

## Out of scope

- A cross-device unread flag: Wire carries none; once Step 129's rail
  exists, a Kupferdraht-own payload in the self conversation is a decision
  of its own.
- Honouring Wire's per-conversation `receipt_mode` (ADR-0012 ignores it;
  unchanged).
- Mute / archive (open.md §64), batched READ confirmations, `FullyRead`.

## TDD sketch

- Store: the flag round-trips, `set_read_marker` clears it, V31 applies on
  a V30 database.
- Core: `mark_as_read(.., ReadPrivate)` moves the marker, clears the flag
  and leaves httpmock without a `POST /mls/messages`; `Read` behaves as
  today; an unknown item leaves the marker and clears the flag;
  `set_unread_flag` on an unknown room is `InvalidInput`.
- Browser suite: `rooms()` carries the three fields and matches
  `unread_counts` / `read_marker` for every room; `markAsUnread` then
  `markAsRead` flips `markedUnread`; the signal reaches the calling tab and
  a thin tab (the Step 94 driver).
- Parity: the tab-proxy table, the FFI mirror, the doc pins.
- Smoke: two devices on the self-hosted stack — a private read on A leaves
  B's `read_by` empty, a read with receipt fills it.
