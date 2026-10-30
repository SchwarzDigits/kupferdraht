# Step 54 — W3 flush gaps + sync-loop timer (wasm correctness)

> **Status:** MERGED to `main` — `6a20028`, PR #44 (merge `b8dd044`,
> 2026-07-07). Scope: two self-contained wasm-correctness fixes tracked in
> the [reference removed 2026-07-16: wasm-migration-open-work.md never
> existed in this repo] §1 — the streaming sync loop's `tokio` timer
> swapped for a wasm-safe one, and MLS persistence flush gaps on the
> membership/room-creation mutations closed. No new browser smoke — the
> fixes land on unbound paths; the existing `indexeddb_mls` snapshot
> round-trip already covers the persist mechanics, per this file's own
> Verification section.
>
> **Branch / commit prefix:** committed on `wasm`; `step/54-*`.

## Goal

Close the two latent wasm-correctness holes left after Step 53: MLS mutations on
the membership paths that never persisted (lost on reload), and the streaming
sync loop's `tokio` timer that would hang on wasm.

## Fix 1 — sync-loop timer (`client/sync.rs`)

`Client::sync()` (the streaming loop) paced empty drains with
`tokio::time::sleep` **ungated** — a latent hang on wasm (no tokio timer driver;
only `sync_once` is bound today, but the trap was live). Swapped to
`crate::time::sleep` (native → `tokio::time::sleep`; wasm → `gloo-timers`, W4).
The loop is now wasm-safe even before the full W4 push runtime lands.

## Fix 2 — MLS persistence flush gaps (W3)

Before, `persist_mls()` fired only after send / `join_room` / `sync_once` /
login. The membership + room-creation mutations didn't persist, so on wasm they
were lost on reload. Added, all with the guard-dropped-before-`.await` discipline
(`persist_mls` re-locks the MLS mutex):

- **Centralized** in `membership.rs::upload_and_merge_commit_bundle` (the shared
  merge helper) — one flush now covers `invite_members`, `kick_members`, and
  `create_room` (via `establish_group`, which routes through it).
- **`membership.rs::leave_room`** — explicit flush after `discard_group`.
- **`rooms.rs::create_dm`** — explicit flush after the conditional `create_group`.

(These `Client` methods aren't bound to JS yet — the binding surface is the W10
follow-up — but the persistence is now correct for when they are.) Native is a
no-op (SQLCipher persists continuously); the 368-test suite confirms no
regression.

## Verification

- Native (all-targets) clippy + 368 tests green; wasm (`--lib`) clippy green.
- No new browser test — the fixes are on unbound paths; the existing
  `indexeddb_mls` snapshot round-trip already covers the persist mechanics.

## Follow-ups

Tracked in [reference removed 2026-07-16: wasm-migration-open-work.md never
existed in this repo]: W4 (live sync/push), W7 (assets), W10 binding breadth
(which will exercise these flush paths from JS).
