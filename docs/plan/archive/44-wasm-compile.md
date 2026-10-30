# Step 44 — Make `kupferdraht-core` compile for `wasm32` (with stubs)

> **Status:** MERGED to `main` — `ebce3c1`+`3290b5d`, PR #44 (merge
> `b8dd044`, 2026-07-07). Scope: native-safe (every change `cfg`-gated;
> native build/clippy/tests stay green) — gets `cargo build --target
> wasm32-unknown-unknown -p kupferdraht-core` to green, with stubs for the
> browser-native backends landed in later steps; second step of the
> browser/WASM migration (natrium `kupferdraht-wasm.md` / `kupferdraht-wasm-storage.md`).
> No smoke (compile-only step; its own "Out of scope" note says "running
> anything in a browser — this step only establishes that the crate
> compiles for wasm").
>
> **Branch / commit prefix:** committed on `wasm`; `step/44-*`.

---

## Goal

The engine has to *link* for `wasm32` before any browser-native backend can be
built or tested. This step clears every compile wall — dependency-level and
Kupferdraht-code-level — so the wasm target builds. Paths that need a real browser
backend (persistent storage, keystore, HTTP transport specifics, background
sync, assets) are **stubbed** (`unimplemented!()` / error / in-memory), clearly
marked, to be replaced by W1/W2/W3/W5/W7/W10.

Walls were cleared **iteratively**, in the order the compiler surfaced them
(`getrandom` → tokio/reqwest features → C-deps → uuid/openmls `js` → Kupferdraht
modules → HTTP/async `Send`).

## What changed

### 1. Dependency layer (`Cargo.toml`)
- `getrandom` (`js`), `uuid` (`js`), `openmls` (`js`) — wasm needs Web Crypto /
  JS time; enabled only under `cfg(target_arch = "wasm32")`.
- `tokio` — base features `macros,sync,rt,time,io-util` (wasm-supported);
  `rt-multi-thread,fs` moved to `cfg(not(wasm32))`.
- `reqwest` — base `json`; `rustls-tls,cookies,stream` moved to
  `cfg(not(wasm32))` (wasm uses the browser `fetch` backend, which provides TLS
  and ignores those).
- `rusqlite` (bundled SQLCipher + vendored OpenSSL), `refinery`,
  `openmls_sqlite_storage`, `keyring` — **native-only** (C SQLite/OpenSSL and
  the desktop keyring don't build for wasm).

### 2. Storage (`storage/`)
- The whole `sqlite` backend + `schema` (refinery) gated `cfg(not(wasm32))` via
  one gate on `mod sqlite`/`mod schema`; `SqliteGlobalStore` likewise.
- `open_default_store` / `open_default_global_store` — **wasm stubs**
  (`unimplemented!("… IndexedDB — W1")`). `apply_whole_db_key` native-only.

### 3. Keystore (`keystore/`)
- `keyring_store` native-only; the wasm default keystore is `InMemoryKeyStore`
  (ephemeral, pending the WebCrypto/IndexedDB store — W2).

### 4. MLS provider (`encryption/provider.rs`)
- `KupferdrahtProvider`'s storage is `cfg`-aliased: `SqliteStorageProvider` (native)
  / openmls `MemoryStorage` (wasm — **non-persistent**, pending the IndexedDB
  openmls provider — W3). `open_mls_provider` on wasm returns an in-memory
  provider.

### 5. Async runtime + HTTP (`wire/http/`, `client/`)
- `TokenRefresh` trait + its boxed-future return: `Send`/`Sync` bounds
  are `cfg`-conditional. On wasm the trait carries **no** `Send`/`Sync` supertrait
  (the browser is single-threaded, and the fetch-backed refresher future is
  `!Send`; `Weak<dyn Trait>: Sync` would in turn demand `Send`).
- reqwest client builder: `connect_timeout`/`timeout` native-only (fetch has no
  timeout knobs).
- Asset streaming (`upload_asset`/`download_asset`/`stream_response_to_path`,
  `tokio::fs`, `Body::wrap_stream`, `Response::bytes_stream`) native-only; wasm
  stubs return an error (browser OPFS/Blob path — W7).
- Background loops using `tokio::spawn` — `Client::sync_stream` and the typing
  auto-stop — native-only. On wasm `sync_stream` yields a single error; the real
  single-threaded `spawn_local` sync is a later step. Explicit typing send works.

### 6. FFI (`lib.rs`, `ffi/`)
- `mod ffi` (the UniFFI C-ABI wrapper) is native/mobile-only; the browser binds
  the core via `wasm-bindgen` (a later step — W10). UniFFI *derives*
  (`Record`/`export`) elsewhere still compile on wasm, so they stay.

### 7. Misc
- `#![cfg_attr(target_arch = "wasm32", allow(dead_code))]` — the stub build
  leaves native-only helpers (key ids, `KupferdrahtHome` paths) unused; allowed on
  wasm only, native keeps strict checking.

## What does NOT work on wasm yet (the stubs)

| Area | wasm behaviour today | Real backend (later step) |
|---|---|---|
| App/global storage | `unimplemented!()` panic | IndexedDB `Store` — **W1** (see `kupferdraht-wasm-storage.md`) |
| Keystore | in-memory, ephemeral | WebCrypto + IndexedDB — **W2** |
| MLS store | in-memory (`MemoryStorage`), non-persistent | IndexedDB openmls provider — **W3** |
| HTTP transport | compiles via fetch; no timeouts/cookies/streaming | fetch semantics + `zuid` cookie model — **W5/W6** |
| Assets | error | OPFS/Blob — **W7** |
| Background sync | single error item | single-thread `spawn_local` loop — **W4** |
| Bindings | none (FFI gated out) | `wasm-bindgen` layer — **W10** |

## Verification

- `cargo build --target wasm32-unknown-unknown -p kupferdraht-core` — **green, 0
  warnings**.
- `cargo build -p kupferdraht-core`, `cargo clippy -p kupferdraht-core --all-targets -D
  warnings`, `cargo test -p kupferdraht-core` — **native green** (unchanged behaviour;
  every wasm path is behind `cfg`).

## Out of scope

- All the real browser backends (W1/W2/W3/W5/W7/W10) — each its own step.
- Running anything in a browser — this step only establishes that the crate
  *compiles* for wasm.
