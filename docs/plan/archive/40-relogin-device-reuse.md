# Step 40 — Re-login device reuse: qualified-user-id key fix

> **Status:** MERGED to `main` — `87c60b5`, PR #34 (squash-merge, no
> separate merge commit), 2026-06-22. Scope: fixes the qualified-user-id
> key bug that made re-login register a new device every time instead of
> reusing the existing one (a bug dormant until Step 39's
> `SessionExpired` → `LoggedOut` hook made re-login-after-logout actually
> happen); cross-repo analysis: natrium's
> `natrium-core/docs/relogin-device-reuse.md`. No dated live-smoke record
> in this file — its own Verification section lists a live-account check
> ("on a clean account … confirm a re-login-after-logout keeps the same
> device + chats") as a to-do, not a logged result.
>
> **Branch / commit prefix:** `step/40-*`, `fix(step-40): …`.

---

## The bug

Login tries to reuse the existing device — `establish_device_identity`
(`client/resume.rs:51`) calls `reuse_existing_device_if_valid` before falling
back to `register_and_rebind`. But the reuse path looked up the persisted session
by the **bare** Wire uuid (`self_user.id`), while everything is keyed by the
**qualified** `<uuid>@<domain>` form (Step 19; `persist_session` writes via
`UserId::as_str()`). So `load_session(bare)` returned `None`, the reuse bailed
**before** even checking Wire (`client_exists`), and a fresh device was
registered.

Dormant until now because the only caller is `reuse_existing_device_if_valid`
during a **login on an existing account** — i.e. a re-login after logout, which
never happened before Step 39's hook. Confirmed live: the test account piled up
**many** devices (one per failed-reuse re-login), heading for the 7-device cap.

## The fix (Part A)

`client/login_pipeline.rs` — use the qualified id for the store lookups that the
rows are actually keyed by:
- `reuse_existing_device_if_valid`: `load_session(self_user.qualified_id…)` (was
  `self_user.id`), and replace the stale "rows key on the bare uuid" comment.
- `resume_existing_mls_state`: `load_persisted_mls_public_key(qualified_user_id…)`
  (was `self_user.id`).

With this, a re-login finds the persisted session → `client_exists(device_id)`
(true) → rebinds → **reuses the device + MLS state**. No new device; messages
work.

## Part B — wipe the superseded device's state on a genuine replacement

When the persisted device is *genuinely* gone from Wire (`client_exists` 404s),
re-login registers a new device that must **not** inherit the old device's stale
MLS leaves — otherwise `join_all_rooms` skips the rooms (it keys on the MLS store)
and the new device sends on a dead epoch → nobody receives.

- `reuse_existing_device_if_valid` now returns a 3-way `DeviceReuse`
  (`Reused` / `Replaced` / `Fresh`) instead of `Option`, so the caller can tell a
  genuine device-gone (`Replaced`) apart from a clean first login (`Fresh`).
- On `Replaced`, after the new device + MLS identity are established,
  `establish_device_identity` calls `wipe_superseded_state`: `discard_group` for
  every old room's MLS group (via the room `group_id`s in the store), then
  `Store::clear_messaging_state` to drop rooms / members / timeline (+ reaction /
  mention / receipt / attachment overlays) / sync cursor. The room list is
  re-fetched on the next `sync_metadata`; `join_all_rooms` then re-joins each room
  cleanly with the new identity.
- **Preserved:** session / device / self-user / server-config / feature-config
  rows (login re-establishes those). **Dropped:** the old decrypted history (a
  genuinely new device has no claim to it; forward secrecy is lost regardless).
- New `Store::clear_messaging_state` (one transactional table clear) — sole impl
  `SqliteStore`. No FFI change (no binding regen).

## TDD / verification

- `tests/kupferdraht_manager.rs::kupferdraht_relogin_reuses_existing_device` (Part A) — two
  logins on one account; the reuse path must reach `GET /clients/{id}`
  (`client_exists` hit once) and not register a second device. Validated to
  **fail** with the bare key and **pass** with the qualified key.
- `storage::sqlite::wipe::clear_messaging_state_empties_rooms_and_cursor`
  (Part B) — seed rooms + sync cursor, clear, assert both gone (and the
  transaction exercises every messaging table, so the table names are checked).
- CI gate: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets
  -- -D warnings`, `cargo test --workspace --all-targets`.
- Live: on a clean account (prune the piled-up devices first), confirm a
  re-login-after-logout keeps the same device + chats and messages still send.
