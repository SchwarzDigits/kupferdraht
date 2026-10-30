# Browser architecture: WASM-only delivery, wasm-bindgen surface (persistence → ADR-0022)

Condensed from `docs/adrs/0015-browser-architecture-wasm-indexeddb-bindgen.md`.

- **Deliver Kupferdraht for Web exclusively as WebAssembly** — do not write
  or accept a JavaScript/TypeScript port; WASM linear memory isolates
  sensitive data from page scripts.
- **Do not rely on WASM isolation past the JS boundary** — once a
  decrypted value crosses to JS, CSP / SRI / origin isolation are the
  consumer's responsibility.
- **Browser persistence: superseded by ADR-0022** (2026-09-24); current
  rule: `rules/0022-browser-persistence-sqlite3mc-over-opfs.md` (the
  native SQLite3MC store over OPFS). Don't reintroduce the IndexedDB
  per-record store, per-record `aes-gcm` or plaintext index keys, and
  don't reintroduce sql.js / single-blob storage either.
- **The Web surface is the hand-written wasm-bindgen layer
  (`src/wasm/`)** — parallel to `ffi/`, never UniFFI-generated. Every
  wasm-surface change updates `docs/bindings.md` in the same commit.
