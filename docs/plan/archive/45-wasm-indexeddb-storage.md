# Step 45 — Browser storage backend (`IndexedDB`) — W1

> **Status:** MERGED to `main` — `4a903ed`, PR #44 (merge `b8dd044`,
> 2026-07-07). Scope: the real `IndexedDB`-backed `Store` / `GlobalStore`
> implementing **W1** from natrium's
> [`kupferdraht-wasm-storage.md`](../../../natrium-ion/natrium-core/docs/kupferdraht-wasm-storage.md),
> replacing the Step 44 wasm stubs; native is untouched (SQLCipher stays;
> every change is `cfg`-gated); third step of the browser/WASM migration.
> No dated smoke record in this file — its Verification section describes
> a `wasm-bindgen-test` suite plus a "browser smoke: login → persist
> session/rooms/timeline → reload → data survives (once W2 persists the
> key)" as the test plan, not a logged completed run.
>
> **Branch / commit prefix:** committed on `wasm`; `step/45-*`.

---

## Goal

Step 44 got the crate to *compile* for wasm with `unimplemented!()` storage
stubs. This step makes storage actually *work* in the browser: implement the
`Store` (46 methods) and `GlobalStore` (7 methods) traits over `IndexedDB`, with
per-record encryption, and wire them in as the wasm `StorageProvider` — so
login → persist → reload works in a browser.

Native keeps SQLCipher/`rusqlite` (ADR-0013); this is the wasm arm only. The
seam already exists (`StorageProvider` / `Store` / `GlobalStore`), and the
contract is "the concrete store does its own encryption" — so the two backends
coexist cleanly.

## Sub-steps

- **W1a — `Send`-bound relaxation. ✅ DONE.** `IndexedDB` handles/futures are
  `!Send`; the `#[async_trait]` storage traits defaulted to `+ Send`. Introduced
  `storage::MaybeSendSync` (a marker = `Send + Sync` on native, nothing on wasm)
  as the supertrait of `Store`/`GlobalStore`, and made the `async_trait` attr
  `cfg`-conditional (`?Send` on wasm). Native + wasm both build green;
  behaviour/`Send`ness unchanged on native.
- **W1b — `IndexedDB` foundation + `GlobalStore`.** DB open + versioned schema
  (`onupgradeneeded`), the per-record encryption helper, and the 7-method
  `IndexedDbGlobalStore` end-to-end. Proves the whole stack (idb + async +
  aes-gcm + `wasm-bindgen-test` in a headless browser) on the small store first.
- **W1c — `IndexedDbStore` (the 46 methods).** One object store per table,
  secondary indexes per query pattern, cursor-based timeline pagination. The
  index catalogue (which fields stay plaintext) comes from
  `kupferdraht-wasm-storage.md` §4–§5.
- **W1d — wire + conformance.** Replace the wasm `open_default_store` /
  `open_default_global_store` stubs with the `IndexedDB` provider; a
  `wasm-bindgen-test` suite + (ideally) a trait-level conformance suite shared
  with the native backend so the two don't drift.

## Decisions

- **Crate — `idb`.** Use the `idb` crate (an ergonomic async wrapper over
  `web-sys`'s `IndexedDB`) rather than raw `web-sys` — far less callback
  boilerplate; wasm-target dependency only. (D-storage-2.)
- **Encryption — Rust `aes-gcm`, key from the keystore.** Per-record AES-256-GCM
  using the `aes-gcm` crate (already a dependency, wasm-clean), with the key
  fetched from the `SecureKeyStore`. No WebCrypto on the Rust side — keeps the
  crypto in Rust and honours the "store does its own encryption" contract. (The
  *key persistence* is W2's WebCrypto/IndexedDB keystore; until then the wasm
  keystore is in-memory, so a browser reload loses the key — acceptable for W1
  bring-up, closed by W2.)
- **Which fields stay plaintext — OPEN (D-storage-1, needs Volker).** `IndexedDB`
  indexes require readable keys, so the identifier/relationship graph + timeline
  `sent_at_ms` must be plaintext; sensitive content (message body, room name,
  display names, tokens) is encrypted. The full catalogue + the honest metadata-
  exposure delta is in `kupferdraht-wasm-storage.md` §5. W1b/c ship a sensible default
  (encrypt the historical `_ct` set, plaintext PK/index fields); revisit with
  Volker before it's load-bearing.

## Design (mirrors `kupferdraht-wasm-storage.md`)

- **One object store per table** (`sessions`, `rooms`, `room_members`,
  `timeline_events`, `receipts`, `reactions`, `mentions`, `attachments`,
  `feature_configs`, `user_properties`, `sync_state`, …), keyed by the table's
  primary key (composite keys as `keyPath` arrays).
- **Secondary indexes** per query: `timeline_events` by `(room_id, sent_at_ms)`
  for `load_timeline_events`, by `message_id` for lookups; relationship tables by
  `(room_id, message_id)`; etc. — derived by walking the 46 methods.
- **Cursors** replace SQL `ORDER BY … LIMIT` (timeline pagination by a `before`
  cursor on the `(room_id, sent_at_ms)` index).
- **Transactions:** `IndexedDB` transactions auto-commit at the end of the
  event-loop turn; multi-step atomic writes (notably the Step-39/41 session
  rotate-then-persist) must complete within a single transaction turn — flagged
  per method.
- **Encryption boundary:** sensitive columns stored as `{ct, nonce}` blobs;
  plaintext index/PK columns stored verbatim.

## Verification

- Native: `cargo build`/`clippy`/`test` stay green (W1a already confirmed).
- wasm: `cargo build --target wasm32-unknown-unknown` green; `wasm-bindgen-test`
  (headless Chrome) covering each store method; a browser smoke: login → persist
  session/rooms/timeline → reload → data survives (once W2 persists the key).

## Out of scope

- The WebCrypto/`IndexedDB` **keystore** (W2) — key persistence across reloads.
- The `IndexedDB` **openmls** provider (W3) — MLS state persistence (Step 44 uses
  in-memory `MemoryStorage`).
- fetch HTTP semantics (W5/W6), OPFS assets (W7), `wasm-bindgen` bindings (W10).
