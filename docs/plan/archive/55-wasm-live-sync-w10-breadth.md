# Step 55 — Live sync (W4) + binding breadth (W10) — real-time chat

> **Status:** MERGED to `main` — `8c96ed0`, PR #44 (merge `b8dd044`,
> 2026-07-07). Scope: **W4** live push sync (`Client.startSync(onMessage)`,
> replacing the prototype's poll) + **W10** binding breadth (edit/redact/
> markAsRead/createRoom/createDm/members). Native (all-targets) clippy
> green (binding `cfg`-gated, native untouched), wasm build/clippy green,
> prototype `vue-tsc` + Vite build green. Live verification pending (as of
> 2026-07-16) — the file's own Verification section: "Pending (browser):
> two accounts — messages arrive live (no manual refresh); edit/delete/
> react/reply; create a group + DM; member list renders."
>
> **Branch / commit prefix:** committed on `wasm`; `step/55-*`.

## Goal

Turn the browser prototype into a real-time chat: push messages instead of
polling (**W4**), and broaden the `wasm-bindgen` surface (**W10**) to the message
+ conversation operations a messenger needs.

## W4 — live push (`src/wasm/live_sync.rs`)

A background sync loop delivers decrypted messages to a JS callback via a real
`EventHandler` — replacing the prototype's `setInterval` poll.

- `Client.startSync(onMessage)` registers a `JsEventHandler` and `spawn_local`s a
  loop: `sync_once` → `crate::time::sleep(2.5s)` backoff, until stopped or
  `SessionExpired`. Returns a `SyncHandle` whose `.stop()` ends it.
- `JsEventHandler` holds the `js_sys::Function` in a `send_wrapper::SendWrapper`
  so a `!Send` JS callback satisfies the core `EventHandler: Send + Sync` bound —
  the browser is single-threaded, so it's only ever called on the origin thread
  (the hack stays in the binding; the core trait is untouched). `on_message`
  marshals `Message` → `{ roomId, senderUserId, messageId, body, replyToId,
  receivedAtMs }`; `on_sync_error` logs to the console.

## W10 — binding breadth (`src/wasm/manager.rs`)

New `Client` methods (all async → `Promise`, `ffi`-style projection structs):

- `editMessage(roomId, itemId, newBody)`, `redact(roomId, itemId)` (delete),
  `markAsRead(roomId, itemId)`.
- `createRoom(name)`, `createDm(peer)` → `{ roomId, name }`.
- `members(roomId)` → `{ userId, isSelf, displayName, handle }[]`.

(These reach the Step 54 W3 flush hooks — `create_room`/`create_dm`/leave now
persist MLS on wasm.) Enriched message projection (`replyTo` + `reactions`) and
`sendReply`/`toggleReaction` were added in the preceding round.

## Prototype (`kupferdraht-wasm-prototyp`, a colleague's local checkout)

Rebuilt as a two-pane messenger: room sidebar (with a **+ New conversation**
group/DM form) + message thread (bubbles, reply-quotes, reactions, per-message
**edit**/**delete** on own messages, a **members** panel) + composer. The
`setInterval` poll is gone — `startSync`'s push callback drives reloads;
`SyncHandle.stop()` runs on logout/unmount. `markAsRead` fires once per room open.

## Verification

- Native (all-targets) clippy green; wasm (`--lib`) clippy green; `wasm-pack`
  emits the typed API (`startSync`/`SyncHandle`/edit/redact/markAsRead/create*/
  members); prototype `vue-tsc` + Vite build green.
- **Pending (browser):** two accounts — messages arrive live (no manual refresh);
  edit/delete/react/reply; create a group + DM; member list renders.

## Out of scope / follow-ups

- **W7 assets** (Step 56) and **W2 Web Worker keystore** (Step 57) — next.
- `logout` (server-side `zuid` revocation) not yet bound; the prototype signs out
  locally only.
- Push callback currently triggers a full reload of rooms + the open thread;
  optimistic/append rendering is a later polish.
