# Browser persistence: SQLite3MC over OPFS (sahpool), the native store on wasm

Condensed from `docs/adrs/0022-browser-persistence-sqlite3mc-over-opfs.md`.

Supersedes Part 2 (persistence) of `rules/0015-browser-architecture-wasm-indexeddb-bindgen.md`
(2026-09-24); its Parts 1 (WASM-only delivery) and 3 (hand-written
wasm-bindgen surface) stay in force there.

- **The browser runs the same SQLite3MC store as native** — `SqliteStore`,
  `SqliteGlobalStore`, the refinery migrations and `openmls_sqlite_storage`
  over rusqlite, wrapped by SQLite3MC's cipher VFS. Don't reintroduce an
  IndexedDB record store, per-record `aes-gcm`, plaintext index keys or an
  MLS snapshot/flush. **The VFS underneath: superseded by ADR-0035**
  (2026-09-29) — `sqlite-remote-vfs` in server or local mode, no OPFS;
  current rule: `rules/0035-store-browser-databases-on-sqlite-remote-vfs.md`.
- **One amalgamation on every target:** `sqlite-wasm-rs` (`sqlite3mc`
  feature) compiles `vendor/sqlite3mc/` via `SQLITE_WASM_RS_SOURCE_DIR` in
  the root `.cargo/config.toml`; pin `sqlite3mc_version()` against
  `PROVENANCE.txt` on wasm as on native. Don't let the crate's bundled
  copy or a second stand-in become the engine.
- **rusqlite stays on the 0.37 line;** the browser path is the
  `libsqlite3-sys` stand-in's wasm arm (re-export of `sqlite-wasm-rs`, the
  two deprecated stubs, the safe `sqlite3mc_wrap_vfs`). The one `unsafe`
  FFI call lives in `vendor/`, never in `kupferdraht-core`.
- **Dedicated worker or nothing:** `wasm::Kupferdraht::open` **fails closed**
  outside a dedicated worker — no silent in-memory fallback. (The OPFS pool
  it installed is gone with ADR-0035.)
- **Rollback journal on wasm** (`delete`); `journal_mode = WAL` is
  native-only. Keep the 32-byte plaintext header on both targets. Don't
  request WAL on `sahpool` (it needs `locking_mode = EXCLUSIVE` before
  every first read or the open fails).
- **The keystore stays `IndexedDbKeyStore`** (IndexedDB + WebCrypto);
  `idb` is a keystore-only dependency. It holds the database keys as
  natively; since ADR-0035 a new key of an account is derived from that
  account's seed, stored beside them.
- **Multi-tab: superseded by ADR-0023** (2026-09-24) — one active
  instance per origin, enforced by a Web Locks lease with explicit
  takeover; current rule:
  `rules/0023-single-active-kupferdraht-instance-per-origin.md`. No
  migration of pre-ADR-0022 browser data.
- **Keep `time`'s `wasm-bindgen` feature on** (refinery's `applied_on`
  panics on wasm without it) and a wasm-capable clang in every build
  environment (`CC_wasm32_unknown_unknown`, never committed).
