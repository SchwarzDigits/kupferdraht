# Step 174 — Conversation settings: read receipts, the message timer, mute and archive, roles, their events

**Status: commissioned 2026-11-25** (Volker), in four sub-steps, each its own branch,
commit and PR; after Step 173 **and after the Rust server's task Q** (galley-rs gets
`PUT …/message-timer`, `…/self`, `…/members/{user}` with their events; only then can
this step be tested there).

- **174a — read receipts as Wire's clients do them:** `expects_read_confirmation` on
  `Text` (and `Asset`, …) declared and set from the user's setting; READ sent only when
  the received message expects it and the own setting allows.
- **174b — the message timer:** `message_timer` read; set, Kupferdraht sends every
  message as ephemeral with it (every kind, Step 172a); set and clear it.
- **174c — mute and archive:** `otr_muted_status` / `otr_archived` read, set (`PUT
  …/self`) and honoured in the room list.
- **174d — roles and the events:** `PUT …/members/{user}` (`wire_admin` /
  `wire_member`) if Kupferdraht offers room admins, else shown correctly;
  `member-update`, `access-update`, `receipt-mode-update`, `message-timer-update` and
  `protocol-update` applied to state with a room event each, not a reload of the list.

## Problem

`message_timer`, `otr_muted_status` and `otr_archived` are ignored (`wire/dto.rs`),
`Text` has no `expects_read_confirmation` and `Asset` sets it to `false`, READ hangs on
`WIRE_RECEIPT_MODE` alone (`server_settings.rs`), and the five events only mark the
room list stale (`sync/service.rs`). In MLS conversations there is no `receipt_mode`:
Wire 5.38.9 refuses `PUT …/receipt-mode` with `403 mls-receipts-not-allowed`, galley-rs
the same since task Q — Kupferdraht never sets it and derives nothing from it.

## Scope

1. **174a:** the field in the proto; set on send from the user's receipt setting
   (`WIRE_RECEIPT_MODE`, read, never set — NG-8), in 1:1s as in groups; phase 1 reads
   how the webapp handles it in MLS groups and names the source; READ only when the
   message expects it and the setting allows.
2. **174b:** `message_timer` on the room; a set timer makes every send ephemeral with
   it; `set_message_timer` / clear through `PUT …/message-timer`; the CLI's
   `room-timer`.
3. **174c:** the two fields on the room; `Room::set_muted` / `set_archived` through
   `PUT …/self`; `rooms()` and the room list carry them (a muted room notifies nothing,
   an archived one is hidden where the embedder asks); the values checked against
   Wire's.
4. **174d:** `Room::set_member_role` if room admins are offered, the events applied in
   `sync/dispatch/room_event.rs` with their system lines (Step 145's kinds grow).
5. **Docs:** `docs/wire/api-notes.md` (every route and event, live), `docs/backend-spec.md`
   (§9.4, §9.6, Step 166's paragraph), `docs/library.md`, `docs/bindings.md`,
   `docs/cli.md`, `docs/api-mapping-matrix-rust-sdk.md` (matrix's `m.push_rules`
   mute and `m.tag` archive as analogs), `docs/ARCHITECTURE.md` §7 (columns),
   `CHANGELOG.md`.

## Out of scope

- `receipt_mode` on MLS conversations (Wire refuses it); setting the user's receipt
  setting (NG-8).

## TDD sketch

- 174a: a `Text` sent with the setting on carries `expects_read_confirmation: true`,
  off `false`; an inbound message with `false` gets no READ although the setting is on;
  one with `true` and the setting off gets none — red;
- 174b: a room with `message_timer` set sends a file as `Ephemeral` with that TTL;
  `set_message_timer` calls the route and the next send honours it — red;
- 174c: `set_muted` calls `PUT …/self` and `rooms()` shows it; an inbound
  `member-update` with `otr_muted_status` updates the row — red;
- 174d: each of the five events updates the room and adds its room event; a role
  change shows in `members()` — red.

Smoke against galley-rs after task Q, with the webapp: receipts in both directions
(1:1 and group, the user's setting on and off); a timer set by the webapp honoured by
Kupferdraht and the other way; mute and archive; a role change.
