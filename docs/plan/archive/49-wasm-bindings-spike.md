# Step 49 — `wasm-bindgen` binding spike + browser prototype — W10 (start)

> **Status:** MERGED to `main` — `1800210`, PR #44 (merge `b8dd044`,
> 2026-07-07). Scope: `wasm-bindgen` binding toolchain spike + browser
> prototype — first slice of **W10** (the browser binding layer);
> `wasm-pack build --target web` produces a loadable ES module. Live
> verification pending (as of 2026-07-16) — the file's own Verification
> section says manual load in the prototype was "pending a Node toolchain
> on this machine", and its Out-of-scope/follow-ups section confirms the
> Node toolchain "is not installed on this machine" and the prototype run
> is "blocked".
>
> **Branch / commit prefix:** committed on `wasm`; `step/49-*`.

## Goal

Stand up the JS/TS binding toolchain so the Kupferdraht wasm engine can be driven
from a real browser app — the vehicle for verifying the whole wasm migration
(W1/W2/W3/W5/W6) end to end, and the natural way to de-risk **W6** (see below).

## What landed

- **`src/wasm/mod.rs`** — a new wasm-only `#[wasm_bindgen]` module (`pub mod
  wasm`, gated `#[cfg(target_arch = "wasm32")]`), parallel to `ffi` (the core
  types carry no `UniFFI` annotations, so the two binding layers don't collide).
  Two trivial exports to prove the boundary: `kupferdraht_version()` and
  `kupferdraht_now_ms()` (the latter through the W8 `js_sys::Date::now()` shim).
- **Toolchain proof:** `wasm-pack build --target web` compiles cleanly and
  **coexists with the unconditional `uniffi::setup_scaffolding!`** (the main
  unknown) — it emits `kupferdraht.js` + `kupferdraht_bg.wasm` + `kupferdraht.d.ts` with both
  exports typed (`kupferdraht_now_ms(): number`, `kupferdraht_version(): string`).

## The browser prototype

Lives **outside this repo** in a colleague's local checkout (`kupferdraht-wasm-prototyp`) — a
Vite + Vue 3 + TypeScript app that loads the pkg (`init()` once, then calls the
exports) and shows a live `now_ms()` tick. `npm run build:wasm` regenerates the
pkg from this crate.

**Why it de-risks W6:** the app talks to `/api/...` and Vite's dev-server proxy
forwards to the real Wire origin, so the browser sees **one origin**. The
browser cookie jar + `fetch(credentials:'include')` then carry the `zuid`
refresh cookie automatically — without Wire needing `SameSite=None; Secure` or
CORS-with-credentials. That turns W6 from a Wire-blocked change into something
testable locally.

## Verification

- `wasm-pack build --target web --dev` → pkg with typed exports (manual load in
  the prototype pending a Node toolchain on this machine).
- Native (`--lib`) + wasm (`--lib`) clippy green; the binding is `cfg`-gated so
  native is untouched.

## Out of scope / follow-ups

- **Node toolchain.** Not installed on this machine; the prototype's
  `npm install && npm run dev` is blocked until Node 20+ is available.
- **Grow the binding (W10 proper).** Next exports: a storage/keystore round-trip
  (drive W1/W2 from JS), then `Kupferdraht.login(...)` → the real W6 verification via
  the proxy. `async` core fns map to `Promise` via `wasm-bindgen-futures`.
- **W6 auth cfg-split.** Once login is exposed: wasm auth uses
  `fetch_credentials_include()`, drops the explicit `Cookie` header, and
  `extract_zuid` becomes native-only (the browser jar holds `zuid`). Steps
  39/40/41 (rotate-then-persist) go native-only.
