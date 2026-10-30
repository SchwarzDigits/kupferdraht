# Step 47 — Persistent browser MLS state (`IndexedDB` snapshot) — W3

> **Status:** MERGED to `main` — `8dcae70`, PR #44 (merge `b8dd044`,
> 2026-07-07). Scope: persistent browser MLS state (`IndexedDB` snapshot)
> implementing **W3** from natrium's `kupferdraht-wasm.md` §4.3; native keeps
> the SQLCipher-backed `SqliteStorageProvider`, this is the wasm arm.
> Claims "core browser-verified" per step notes (a `wasm-bindgen-test`
> passes in headless Firefox: snapshot round-trip across a simulated
> reload + at-rest-encryption check), no dated smoke record.
>
> **Branch / commit prefix:** committed on `wasm`; `step/47-*`.

## Goal

Make the wasm openmls store **survive a reload**. After Step 44 the wasm target
ran openmls on an in-memory `MemoryStorage` (C `SQLite` doesn't build for wasm),
so a browser reload dropped all MLS state — signing keys, `KeyPackage` private
halves, per-group epoch secrets. W3 snapshot-persists that store to `IndexedDB`
so a returning user keeps their groups and identity.

## The sync/async bridge (same shape as W2)

openmls's `StorageProvider` is **synchronous**, but `IndexedDB` is async and the
browser cannot block. Rather than wrap all ~40 storage methods with an
async write-through (as W1 did for our own `Store`), the MLS store is small and
opaque, so it is **snapshot-persisted around** the in-memory provider
(`encryption/provider.rs`):

- **`KupferdrahtProvider::open_persistent(account_id)`** (async, wasm-only) restores
  the whole store from the account's `IndexedDB` snapshot before use (empty if
  none yet).
- **`KupferdrahtProvider::flush(account_id)`** (async) writes the whole store back.
  Call after MLS-mutating operations to make them durable across a reload.

`MemoryStorage.values` is a public `RwLock<HashMap<Vec<u8>, Vec<u8>>>`, so the
snapshot is just that map encoded as a **list of key/value pairs via
`serde_json`** (`HashMap<Vec<u8>, …>` can't be a JSON object — non-string keys —
so a `Vec<(Vec<u8>, Vec<u8>)>` is used). `MemoryStorage`'s own
`serialize`/`deserialize` are feature-gated for KATs, hence the direct-`values`
approach.

## At-rest encryption

MLS secrets (signing keys, epoch secrets) live inside that blob, so it is
**AES-256-GCM-encrypted before it reaches `IndexedDB`**, under the MLS
[`DbKey`] — the same keystore-managed, `WebCrypto`-wrapped key material that keys
the native MLS database (via W2). The snapshot reuses W1c's per-record helpers
(`storage::indexeddb::{encrypt, decrypt}`, made `pub(crate)`), so there is one
crypto path across the whole browser storage layer, no duplication. Only
`{ct, nonce}` are stored — the plaintext key/value pairs never touch `IndexedDB`.
`open_persistent`/`flush` therefore take a `&DbKey`. Layout: DB `kupferdraht-mls`,
`snapshots` store, one `{ct, nonce}` record keyed by `account_id`.

## Verification

- `cargo test -p kupferdraht-core --target wasm32-unknown-unknown --test
  indexeddb_mls` (headless Firefox), two tests:
  - `mls_state_persists_across_reopen`: two entries put → `flush` → provider
    dropped → **reopened fresh** (simulated reload) decrypts + deserializes the
    snapshot and reads both back; an unset key returns `None`.
  - `snapshot_is_encrypted_at_rest`: flush under one `DbKey`, reopen under a
    **different** key → decrypt fails, proving the blob is not stored in the
    clear.
- Native (all-targets) + wasm (`--lib`) clippy green.

## Out of scope / follow-ups

- **Wire the flush into the MLS-mutating paths.** `open_mls_provider` still opens
  the in-memory provider (`open_in_memory`); switching it to `open_persistent`
  makes it **async on wasm**, and every MLS mutation (join/commit/message) then
  needs a trailing `flush(account_id)`. Best done end-to-end when the MLS paths
  are actually exercised in the browser (blocked on W5 fetch). Until then the
  snapshot mechanism exists but is not on the live path.
- **Snapshot-on-every-write vs explicit flush.** Whole-store re-serialization is
  fine while the store is small; if it grows, revisit toward per-record writes
  (the W1 `Store` shape) or a dirty-tracking flush.
