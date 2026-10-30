# Step 53 — Wire MLS-state persistence into the lifecycle (W3 complete)

> **Status:** MERGED to `main` — `a419696` (shared with Steps 51/52), PR #44
> (merge `b8dd044`, 2026-07-07). Scope: wires Step 47's encrypted openmls
> snapshot into the client lifecycle, closing **W3** — MLS state now
> survives a browser reload, device reuse re-enabled on wasm. Native
> build/clippy + 368 tests green, wasm build/clippy + the `indexeddb_mls`
> browser tests (2/2) green. Live verification pending (as of 2026-07-16)
> — the file's own Verification section: "Pending (browser): login → send
> → reload → login (reuse) restores MLS → continue messaging with no
> fresh device."
>
> **Branch / commit prefix:** committed on `wasm`; `step/53-*`.

## Goal

Make the browser MLS state survive a reload by wiring Step 47's encrypted
snapshot mechanism into the client lifecycle — closing W3 and removing the two
Step 52 interim limitations (fresh-device-per-login, no-reload-during-messaging).

## Design

The provider owns its **persistence context** so no key threads through
`Client::new`:

- `KupferdrahtProvider` (wasm) gains `persist: Option<PersistCtx { account_id, key }>`.
  `open_persistent(account_id, key)` (now takes an **owned** `DbKey`; `DbKey`
  derives `Clone`) stores it; `open_in_memory` leaves it `None`.
- Persistence is split so the async write never holds the MLS `std::sync::Mutex`:
  `snapshot_ctx(&self) -> Option<(account_id, MlsSnapshot)>` is **sync** (read
  store → serialize → AES-256-GCM encrypt under the MLS `DbKey`); the free
  `save_mls_snapshot(account_id, &snapshot).await` does the `IndexedDB` write.
- `Client::persist_mls()` (wasm) snapshots under the lock, drops the guard, then
  awaits the write. Native is a no-op (SQLCipher persists continuously).

**Restore:** `open_mls_provider` is now `async` on wasm and calls
`open_persistent` (native stays sync — the two login-pipeline callers `cfg`-pick
`.await`; `resume_mls_client` is native-only since wasm resume skips file MLS).

**Flush hooks** — `persist_mls().await` after every MLS mutation on the messaging
path: fresh-identity creation (`PendingLogin::register_device`), text send
(`Client::encrypt_and_upload`), `Client::join_room`, and once per
`Client::sync_once` (covers inbound commits/welcomes from the drain).

**Device reuse re-enabled on wasm:** the Step 52 forced-`Fresh` is gone —
`open_persistent` restores the signature keypair, so a reused device resumes its
MLS identity (same path as native). A wasm safety net: if a reused device has no
restorable snapshot (first login after this shipped, or a cleared store), the
MLS resume error falls through to fresh registration (self-heals + flushes a
snapshot); native treats it as fatal.

## Verification

- `indexeddb_mls` browser tests (snapshot round-trip + at-rest encryption) pass
  under the refactored `snapshot_ctx`/`flush()` API.
- Native (all-targets) clippy + 368 tests green; wasm (`--lib`) clippy green.
- **Pending (browser):** login → send → **reload** → login (reuse) restores MLS
  → continue messaging with no fresh device.

## Resume-with-MLS (wasm)

`ClientBuilder::resume` now restores MLS on wasm too: `resume_mls_state` is
`cfg`-split — native reopens the SQLCipher file, wasm rehydrates the openmls store
from the `IndexedDB` snapshot via `open_mls_provider().await` then
`MlsClient::resume_with_storage` (finds the signature keypair). So after a reload
the "Restore session" path yields a send-capable client — not just login. The
snapshot key matches: both login-flush and resume derive it from
`key_id::account_mls(qualified_user_id)` and key the snapshot by the same
qualified id.

## Out of scope / follow-ups

- Membership mutations not on the prototype's path (room create, invite, kick)
  should also `persist_mls()` — add when those bindings land.
- **W4 live sync + push `EventHandler`** and asset messages (W7) remain.
