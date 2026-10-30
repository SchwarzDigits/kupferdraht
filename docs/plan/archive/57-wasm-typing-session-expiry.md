# Step 57 — Typing indicators (W4) + session-expiry surfacing (W10)

> **Status:** MERGED to `main` — `51aa3f5`, PR #44 (merge `b8dd044`,
> 2026-07-07). Scope: **W4** typing indicators end to end
> (`startTyping`/`stopTyping` + inbound `onTyping`) + **W10**
> session-expiry surfacing (`onExpired` callback on `startSync`). Native
> (all-targets) clippy + fmt green, wasm build/clippy green, prototype
> `vue-tsc` + Vite build green. Live verification pending (as of
> 2026-07-16) — the file's own Verification section: "Pending (browser):
> two accounts — one types, the other sees '… typing…'; expiry banner on
> a dead session."
>
> **Branch / commit prefix:** committed on `wasm`; `step/57-*`.

## Goal

The remaining useful W4/W10 leftovers: typing indicators end to end (the sync
drain already dispatches inbound `conversation.typing` events —
`sync/service.rs:220`), and surfacing a dead session to the UI instead of the
sync loop dying silently.

## W4 — typing

- **Bindings** (`src/wasm/manager.rs`): `startTyping(roomId)` / `stopTyping(roomId)`
  → `Client::send_typing_started/stopped`.
- **Receive** (`src/wasm/live_sync.rs`): `JsEventHandler` gained an optional
  `on_typing` callback; `EventHandler::on_typing` projects `TypingEvent` →
  `{ roomId, typingUsers }` and fires it. Delivered through the same
  `spawn_local` drain that pushes messages (W4).
- **Not wired:** the core ~11 s auto-stop timer stays native-only (`typing.rs`
  uses `tokio::spawn`); the prototype manages start/stop explicitly and times out
  the received indicator client-side (6 s), so no `spawn_local` timer is needed.

## W10 — session-expiry surfacing

`startSync` now takes two extra optional callbacks: `onTyping` and `onExpired`.
When the loop stops because `sync_once` returned `SessionExpired` (dead refresh
token), it fires `onExpired` before breaking — the UI drops to the login gate
instead of silently stopping.

New `startSync(onMessage, onTyping?, onExpired?)` signature.

## Prototype

- Typing: `@input` on the composer fires `startTyping` (once) + a 3 s debounce to
  `stopTyping`; `stopTyping` also on send / room switch. Inbound typing shows a
  "… typing…" line under the thread, auto-cleared after 6 s.
- Expiry: `onExpired` shows a banner + logs out to the gate.

## Verification

- Native (all-targets) clippy + fmt + wasm (`--lib`) clippy green; `wasm-pack`
  emits the typed `startTyping`/`stopTyping` + 3-arg `startSync`; prototype
  `vue-tsc` + Vite build green. Native untouched (binding `cfg`-gated).
- **Pending (browser):** two accounts — one types, the other sees "… typing…";
  expiry banner on a dead session.

## Out of scope / follow-ups

- Core typing auto-stop on wasm (`spawn_local` timer) — only needed if the UI
  stops managing start/stop; low value.
- `sync_stream` core variant stays a wasm stub — redundant with `startSync`.
- W2 Web Worker keystore + W7 OPFS for large files remain (tracker).
