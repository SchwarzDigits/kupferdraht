# Step 59 — Narrow the wasm lint relaxations (§4 shim cleanup)

> **Status:** MERGED to `main` — `ea015ca` (shared with Step 58), PR #44
> (merge `b8dd044`, 2026-07-07). Scope: narrows the crate-wide wasm lint
> relaxation block to the one permanent allow
> (`clippy::arc_with_non_send_sync`), localizing the rest as targeted
> per-site allows (§4 shim cleanup). Native (all-targets) clippy + fmt +
> 368 tests green, wasm (`--lib`) clippy green. No smoke (lint/cleanup
> refactor only; no runtime behaviour change — verified empirically via
> the clippy/fmt/test gate per this file's own Verification section).
>
> **Branch / commit prefix:** committed on `wasm`; `step/59-*`.

## Goal

The crate-wide `#![cfg_attr(target_arch="wasm32", allow(...))]` block masked four
lints (`dead_code`, `unused_async`, `unnecessary_wraps`, `needless_pass_by_value`)
across the whole wasm build, so real issues could hide. Narrow it to the one
permanent relaxation and localize the rest.

## What landed

- **`lib.rs`** — the wasm allow block is now just
  `allow(clippy::arc_with_non_send_sync)` (permanent: single-threaded browser,
  `Arc` of `!Send` JS state). `dead_code` / `unused_async` / `unnecessary_wraps` /
  `needless_pass_by_value` are **removed** — wasm is back at per-lint `deny`.
- The handful of genuine cases carry a **targeted** `#[allow]`/`#[cfg_attr]` at
  the site instead:
  - `storage/home.rs` — `global_db`/`account_db`/`account_mls` (SQLite-path
    helpers, native-only) → `#[cfg_attr(wasm, allow(dead_code))]`.
  - `wire/http/assets.rs` — the wasm `upload_asset`/`download_asset` **stubs**
    (kept only so the shared `http_*` wrappers compile) → `#[allow(unused_async)]`.
  - `client/sync.rs` — `sync_stream`'s `settings` arg, unused in the wasm stub →
    `#[cfg_attr(wasm, allow(needless_pass_by_value))]`.
  - `storage/indexeddb.rs` — `err(e: idb::Error)` (owned by-value required by
    `.map_err(Self::err)`) → `#[allow(needless_pass_by_value)]`.
- **Removed genuinely dead code:** `IndexedDbStore::all_in` (never called).
- `unnecessary_wraps` was no longer triggered at all (dropped for free).

## Verification

Empirically driven: reduced the block to only `arc_with_non_send_sync`, read the
exact lint sites clippy reported, localized each, then removed the block-wide
allows. wasm (`--lib`) clippy + native (all-targets) clippy + fmt + 368 tests
green. Native behaviour unchanged (the `cfg_attr`s are no-ops off wasm).

## Out of scope

- Other §4 markers stay as-is (structural, not masking): `open_in_memory`'s
  `Result` (native signature parity), `resolve_keystore`'s per-fn
  `cfg_attr(..., allow(unused_async))`, the reused-device self-heal fallback, the
  dual sync/async `open_mls_provider`.
- **W7 OPFS for large files** — deferred (blocked by the fetch-buffer upload; the
  in-memory path suffices for images/small files).
