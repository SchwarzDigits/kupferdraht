# Step 46 — Persistent browser keystore (`IndexedDB` + `WebCrypto`) — W2

> **Status:** MERGED to `main` — `3cc9346`, PR #44 (merge `b8dd044`,
> 2026-07-07). Scope: the persistent browser keystore (`IndexedDB` +
> `WebCrypto`) implementing **W2** from natrium's `kupferdraht-wasm.md` §4.2;
> native keeps the OS keyring, this is the wasm arm. Browser-verified per
> step notes (a `wasm-bindgen-test` passes in headless Firefox: a secret
> set → flushed → reopened fresh is decrypted back via the persisted wrap
> key), no dated smoke record.
>
> **Branch / commit prefix:** committed on `wasm`; `step/46-*`.

## Goal

Make the wasm `SecureKeyStore` **persistent**. After Step 44/45 the wasm keystore
was `InMemoryKeyStore` (ephemeral) — a browser reload lost the DB-encryption
keys, making the (encrypted) `IndexedDB` app data unreadable. W2 persists the
keys so the storage survives a reload, and protects them at rest so W1c's
encryption is actually meaningful.

## The sync/async bridge

`SecureKeyStore` is **synchronous** (`fn get_secret/set_secret/delete_secret`),
but `IndexedDB` and `WebCrypto` are async and the browser cannot block. Bridged
by **preload + write-through** (`keystore/indexeddb_keystore.rs`):

- **`IndexedDbKeyStore::open()`** (async) loads + decrypts every stored secret
  into an in-memory `HashMap` before the sync trait is used.
- **Sync reads** serve from the map.
- **Write-through:** `set_secret`/`delete_secret` update the map synchronously
  and fire a best-effort background persist (`wasm_bindgen_futures::spawn_local`);
  **`persist_all()`** (async) is the guaranteed durability flush the consumer
  awaits after key-generating operations.

The struct holds **no `!Send` JS state** (only `Mutex<HashMap>` + config), so it
stays `Send + Sync` and satisfies the trait **unchanged** — no `cfg`-relaxation
of `SecureKeyStore` needed; all JS/async work runs inside
`open`/`spawn_local`/`persist_all`.

## At-rest protection

The DB keys are wrapped with a **non-extractable AES-256-GCM `WebCrypto` key**
(`subtle.generateKey(extractable=false)`), whose raw material never leaves the
browser's crypto subsystem — a raw `IndexedDB` dump does not yield the keys.
This is why W1c's app-data encryption is meaningful on web. Layout: DB
`kupferdraht-keystore` with a `meta` store (the non-extractable `CryptoKey`, itself
`structured-clone`d into `IndexedDB`) and a `secrets` store (`{iv, ct}` per id).

## Verification

- `cargo test -p kupferdraht-core --target wasm32-unknown-unknown --test
  indexeddb_keystore` (headless Firefox): a secret set → flushed → **reopened
  fresh** (simulated reload) is decrypted back via the persisted wrap key.
- Native + wasm build/clippy green.

## Out of scope / follow-ups

- **Wire as the wasm default.** `default_keystore()` is sync; using
  `IndexedDbKeyStore` as the default requires an **async open path** through
  `Kupferdraht::open`/`ClientBuilder` on wasm. Until then a consumer calls
  `IndexedDbKeyStore::open().await` and passes the result to `Kupferdraht::open`.
- **Web Worker support.** `WebCrypto` is reached via `window()`; a Worker
  (no `window`) needs the `WorkerGlobalScope` crypto path (ADR-0005 runs Kupferdraht
  in a worker for memory isolation).
- Durable-on-every-write (vs the explicit `persist_all` flush) — would need an
  awaitable write the sync trait can't express; the flush is the pragmatic
  contract.
