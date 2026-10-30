# Step 52 — Browser send / receive (W3 MLS verified) — W10

> **Status:** MERGED to `main` — `a419696` (shared with Steps 51/53), PR #44
> (merge `b8dd044`, 2026-07-07). Scope: browser send/receive binding
> (`Client.sendText`/`Client.messages`), verifying **W3** (openmls
> encrypt/decrypt) through the live Wire path. Browser-verified per step
> notes ("browser-verified end to end" — two accounts on Wire-dev, A sends
> a text message, B syncs and receives it decrypted, a full MLS
> encrypt→decrypt round-trip in the browser), no dated smoke record.
>
> **Branch / commit prefix:** committed on `wasm`; `step/52-*`.

## Goal

The messaging slice: expose text send + timeline read so the browser app can run
a real MLS conversation — exercising **W3** (openmls encrypt/decrypt) through the
live Wire path with two real accounts.

## What landed

**Bindings (`src/wasm/manager.rs`)** — on the `Client` wrapper, mirroring `ffi`'s
projection-struct approach (the core timeline types aren't serde-able — they hold
`Box<AssetSummary>`):
- `Client.sendText(roomId, body)` → `{ messageId, sentAtMs }`. Routes through
  `Room::timeline().send_text_auto_join(...)`, so it joins the room's MLS group
  first if this client hasn't (avoids `MlsGroupMissing`).
- `Client.messages(roomId, limit)` → `TimelineItemView[]` — each item projected to
  `{ itemId, senderUserId, sentAtMs, kind, body, edited, redacted }` (text bodies
  come through; other kinds carry `body: null` + a `kind` label).

**Receive path (prototype):** poll-based — B calls `syncOnce()` (drains
notifications, MLS-decrypts into the store) then `messages()` re-reads. A push
`EventHandler` callback is a later building block.

**wasm device policy (`login_pipeline.rs`)** — interim: `reuse_existing_device_if_valid`
returns `Fresh` on wasm, so **login always registers a fresh device**. The MLS
signature keypair lives in the in-memory openmls store and isn't snapshot-restored
on login yet (W3 persistence), so a "reused" device can't resume its MLS identity
(`signature keypair not found`). Fresh-device gives a working in-session identity;
cost is Wire device pile-up on re-login (dev-only). Native keeps full device reuse.

## Verification

- Live E2E on Wire-dev, two accounts in separate origins (normal + incognito):
  A `sendText` → B `syncOnce` + `messages` shows A's message decrypted. This is
  the first real MLS round-trip on wasm.
- Native (all-targets) clippy + tests + wasm (`--lib`) clippy green.

## Out of scope / follow-ups (priority order)

1. **W3 MLS persistence** — wire Step 47's encrypted openmls snapshot
   (`KupferdrahtProvider::open_persistent`/`flush`) into `open_mls_provider` so MLS
   survives a reload. Removes both current limitations: the fresh-device-per-login
   policy above **and** the "no reload between login and send" constraint. Needs
   `open_mls_provider` to become async on wasm + a flush after MLS mutations.
2. **W4 live sync + push** — replace poll-based receive with the `Client.sync`
   loop (`spawn_local` + timers) delivering an `EventHandler` JS callback, so
   messages arrive without a manual Refresh.
3. **Timeline richness** — reactions, replies, edits, and asset/file messages
   (W7: OPFS/Blob) in the projection + send surface.
