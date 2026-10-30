# Step 26 — Location & Knock message variants

> **Status:** implemented. Adds two Wire `GenericMessage` content variants the
> POC wants — `Knock` (a nudge/ping) and `Location` (a shared map point) — as a
> full round-trip (receive + send). Before this step Kupferdraht's proto listed them
> only in a comment (`Knock = 4`, `Location = 13`); nothing decoded, persisted,
> exposed, or sent them. Merged: `GenericMessage` `Location` + `Knock` variants
> (send + receive), opaque MLS application messages. `bd02482` (#13). Archived from PLAN.md.
>
> **Sub-steps:** none (one branch, one commit).
>
> **Branch / commit prefix:** `step/26-*`, `feat(step-26): …`.

---

## Why

natrium already models both as consumer types (`ChatMessage.MessageValue`:
`KnockValue(hotKnock)`, `LocationValue(latitude, longitude, …)`), so the
consumer side is fixed and waiting — Kupferdraht just has to feed it. The variants
are part of Wire's textual messaging vocabulary, in scope per the functional
spec (everything Wire defines for non-call communication).

Wire shapes (authoritative — Wire's `GenericMessage` schema):

```proto
message Knock    { bool hot_knock = 1; }            // a nudge; no body
message Location { float longitude = 1; float latitude = 2;
                   optional string name = 3; optional int32 zoom = 4; }
```

(Both also carry `expects_read_confirmation` and `legal_hold_status` — both out
of scope here, see below.)

## matrix-rust-sdk analog

None directly — Matrix carries location as an `m.location` event and has no
"knock message" (its `knock` is a room-membership action, unrelated). These are
Wire-vocabulary extensions; they reuse Kupferdraht's existing content-variant
plumbing (Text / Asset), not a new abstraction.

## The variant-wiring pattern

Both variants repeat the same touch-points that `Text` and `Asset` already
follow — described once, applied to each:

- **proto** (`proto/generic_message.proto`): declare the two messages and add
  `Knock knock = 4;` + `Location location = 13;` to the `GenericMessage.content`
  oneof (the field numbers Wire reserves). prost regenerates the types.
- **model** (`timeline/items.rs`): `TimelineItemKind::Knock { hot_knock }` +
  `TimelineItemKind::Location { latitude, longitude, name, zoom }`; `from_record`
  gains `kind=="knock"` / `kind=="location"` arms (before the existing
  unknown→Text fallback).
- **event body** (`sync/event_handler.rs`): `MessageBody::Knock` /
  `MessageBody::Location` for `EventHandler::on_message`.
- **receive** (`sync/service.rs` + `sync/dispatch.rs`): a content-match arm each
  → `dispatch_inbound_knock` / `_location`, mirroring `dispatch_inbound_text`
  (persist, notify, emit `Append` when newly inserted; self-echoes skipped).
- **send** (`wire/proto.rs` + `timeline/send.rs`):
  `build_knock_generic_message` / `build_location_generic_message` +
  `Client::send_knock` / `send_location` (+ `Timeline` facade), mirroring
  `send_text` — including the Step-25a Failed-on-error persistence.
- **FFI** (`ffi/timeline.rs` + `ffi/sync.rs`): the `TimelineItemKind` /
  `MessageBody` mirrors.
- **CLI** (`kupferdraht-cli`): `knock` / `location` commands + render the new kinds
  in `timeline` and the `sync-stream` event printer.

## Persistence — no migration

The payload packs into the existing `timeline_events.body` TEXT column rather
than a new table or columns:

- **Knock** → `body = "true"` / `"false"` (the `hot_knock` flag), `kind="knock"`.
- **Location** → `body = serde_json({latitude, longitude, name, zoom})`,
  `kind="location"`.

`from_record` parses `body` back into the structured `TimelineItemKind`. Both
payloads are tiny and self-contained, so — unlike `Asset`, which needs a
separate `attachments` table for blob metadata — no migration is warranted.

## Out of scope

- `legal_hold_status` — Kupferdraht has no legal-hold path.
- `expects_read_confirmation` on knock/location — could later drive a READ
  receipt like `Text` does; ignored for now.
- Location-preview / map rendering — the consumer owns presentation.

## Dependencies

- Step 4 / 6-II (the `Text` send + inbound-dispatch + `TimelineItemKind` pattern
  these mirror).
- Step 11 (`Asset` — the second content-variant precedent).
- Step 25a (the Failed-on-send-error persistence the send path reuses).

## Tests

The new logic is the (de)serialization at each boundary; the send / receive
pipelines themselves are unchanged plumbing already covered by Step 4 (inbound
dispatch + outbound `send_text`) and Step 25a (Failed-on-send-error). Unit
coverage therefore targets the boundaries:

- proto round-trip: `build_knock` / `build_location` → decode → fields (incl.
  the optional-fields-`None` case for `Location`).
- `from_record` parses `kind="knock"`/`"location"` body back into the right
  `TimelineItemKind` (incl. `Location` with no optional fields).
- FFI parity pin: `Timeline::send_knock` / `send_location` exist on the FFI
  wrapper (`ffi_smoke.rs`).

## Verification

- Gate: `cargo fmt --check`, build 0 warnings, `clippy -D warnings`,
  `cargo test --workspace`, `cargo deny`.
- Binding smoke: Kotlin + Swift see `TimelineItemKind.Knock`/`Location` +
  `MessageBody.Knock`/`Location`.
- Live (manual): send a knock + a location in a 1:1; the recipient's `timeline`
  shows both, and the round-trip echo lands.
