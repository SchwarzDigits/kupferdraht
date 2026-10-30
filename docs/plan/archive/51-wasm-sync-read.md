# Step 51 — Browser sync + read binding (W1 verified) — W10

> **Status:** MERGED to `main` — `a419696` (shared with Steps 52/53), PR #44
> (merge `b8dd044`, 2026-07-07). Scope: browser sync + read binding
> (`Client.syncOnce()`/`Client.rooms()`), verifying **W1** (`IndexedDb`
> storage) through the real sync path. Browser-verified per step notes
> (live against Wire-dev: `syncOnce()` returned `roomsUpdated: 1,
> eventsDispatched: 6`, and `rooms()` listed the room), no dated smoke
> record.
>
> **Branch / commit prefix:** committed on `wasm`; `step/51-*`.

## Goal

The next real W10 slice: expose sync + room reads so the browser app can pull
state from Wire and render it — which exercises the `IndexedDb` storage backend
(**W1**) through the real sync path, not a synthetic test.

## What landed (`src/wasm/manager.rs`)

Two methods on the `Client` wrapper (async → `Promise`, results marshalled to
plain JS objects via `serde-wasm-bindgen`):

- `Client.syncOnce()` → `{ roomsUpdated, eventsDispatched }` — one sync pass
  (`Client::sync_once`, `SyncSettings::default()`): drains `/notifications`,
  refreshes the conversations list, applies everything to the store.
- `Client.rooms()` → `{ roomId, name }[]` — the rooms known to the client, read
  from the store.

## Verification

- Live against Wire-dev from the prototype: `syncOnce()` returned
  `roomsUpdated: 1, eventsDispatched: 6`, and `rooms()` listed the room — i.e.
  the sync path wrote to `IndexedDb` (W1) and read back. This confirms Step 45's
  W1 backend through the real client, not just the isolated `wasm-bindgen-test`s.
- `sync_once` is one-shot (no `poll_interval` sleep), so it steers clear of the
  loop variants' `tokio::time`/`spawn` (those are W4, for the streaming-sync
  slice). Native + wasm build/clippy green.
- The prototype's "Sync + rooms" card also loads `rooms()` straight from the
  store after a reload+resume (no sync) — the W1 persistence read.

## Out of scope / follow-ups

- **Timeline read** (message bodies): `Room.messages()` returns rich
  `TimelineItem`s; marshalling those to JS lands with the send/receive slice
  (Slice 3 / W3), which needs them anyway.
- **Streaming sync** (`Client.sync` loop) → W4: `spawn_local` + a JS callback /
  async iterator instead of `tokio::spawn` + `tokio::time::sleep`.
- **MLS on resume** (W3): a resumed client currently has no MLS state, so
  message *content* from synced rooms can't be decrypted until the W3 snapshot is
  wired into resume.
