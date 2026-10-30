# Step 58 — Worker-capable keystore (W2 follow-up)

> **Status:** MERGED to `main` — `ea015ca` (shared with Step 59), PR #44
> (merge `b8dd044`, 2026-07-07). Scope: **W2 follow-up** — makes the
> keystore Web-Worker-capable by resolving `SubtleCrypto` via
> `js_sys::global()` instead of `web_sys::window()` (which is `None` in a
> Worker). Browser-verified per step notes (the `indexeddb_keystore`
> browser test passes in headless Firefox: secret set → flush → reopened
> fresh → decrypted via the persisted wrap key), no dated smoke record;
> native untouched (the keystore module is wasm-only). Actually hosting
> the engine in a Worker is explicitly out of scope / not yet done (an
> app-side change), per this file's own notes.
>
> **Branch / commit prefix:** committed on `wasm`; `step/58-*`.

## Goal

Let the whole wasm engine run inside a **Web Worker** (ADR-0005 wants Kupferdraht in
a worker for memory isolation). The one blocker was the keystore reaching
`WebCrypto` via `window()`, which is `None` in a Worker.

## What landed

- **`keystore/indexeddb_keystore.rs` `subtle()`** now resolves `SubtleCrypto`
  from `js_sys::global()` — `dyn_ref::<Window>()` on the main thread,
  `dyn_ref::<WorkerGlobalScope>()` in a Worker — instead of `web_sys::window()`.
- **`Cargo.toml`** wasm `web-sys` features gain `WorkerGlobalScope`.

That was the **only** `window()` dependency in `kupferdraht-core/src` (confirmed by
grep). Everything else was already Worker-safe: the `idb` crate resolves the
`IDBFactory` via `Reflect::get(&js_sys::global(), "indexedDB")`, so all our
`IndexedDB` (keystore, W1 store, MLS snapshots) already works in both scopes.
`getrandom/js` and `js_sys::Date::now()` are scope-agnostic too.

## Verification

- `indexeddb_keystore` browser test passes (secret set → flush → reopened fresh
  → decrypted via the persisted non-extractable wrap key) — confirms the
  `global()`-based crypto access works on the main thread (`global()` *is* the
  `Window` there). wasm (`--lib`) clippy + fmt green; native unaffected.
- **Not yet done:** actually hosting the engine in a Worker — that's an app-side
  change (a Worker entry + `postMessage` bridge in the prototype/consumer), not a
  `kupferdraht-core` change. The core is now Worker-*capable*; wiring a Worker host is
  separate and optional.

## Out of scope / follow-ups

- Worker host wiring (prototype/consumer side) — optional; the core no longer
  blocks it.
- Remaining tracker items: W7 OPFS for large files, W6 cross-origin production,
  the §4 transitional cfg-shim cleanups.
