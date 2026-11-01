# Step 78 — Native sign-out and account purge (`Kupferdraht::logout`, `Kupferdraht::forget_account`)

**Status: in progress 2026-11-01** on `step/78-local-account-purge`, off
`main` at the `v0.1.0` tag. Commissioned 2026-09-10 as "Local account purge
(destroy_store)" (Kalium-audit follow-up — Volker: too critical to backlog).
Re-commissioned 2026-11-01 with the consumer-side write-up of the first Rust
embedder (a TUI; its open.md §8 and §10), folded in below with the
corrections the code review of 2026-10-31 found. Closes that embedder's §8
(manager-level sign-out) and §10 (store destruction), and Kupferdraht's
open.md §72 bullet "a purge needs a live client".

## Problem

A native Rust consumer of `kupferdraht-core` cannot sign an account out
cleanly or forget it.

- `Client::logout(purge_device, password)` (`client/session.rs`) revokes the
  refresh cookie via `POST /access/logout` and, with `purge_device`, deletes
  the device at the backend first. Its doc comment states the policy: the
  library does not touch local storage in `logout`; that is the caller's
  decision. The manager (`Kupferdraht`, `manager/mod.rs`) exposes `accounts`,
  `active_account`, `login`, `register`, the SSO calls and `resume` — nothing
  that removes or deactivates an account.
- After a plain `Client::logout` the registry still names the account as
  active. The next start of a consumer that resumes on start reads
  `active_account()`, calls `resume`, and learns of the dead session only
  through a network round trip (`SessionExpired` from the `/access` rebind
  inside `resume` once the access token is stale, or from the first call
  after it) — the TUI then lands on its login form with an error notice
  instead of knowing cleanly that nobody is signed in.
- Deleting a profile is a hand-rolled directory wipe. The per-account
  keystore ids are internal to the SDK (`keystore::key_id`), so a consumer
  cannot delete the account's secrets. The TUI deletes the one id it knows,
  `global-db` — which, since Step 93 (ADR-0025), is the *legacy* id: the
  entries in effect are `kupferdraht.<home-id>.global-db`,
  `….account-<user>`, `….account-<user>-mls`. The deletion is a no-op; every
  profile deletion leaves all of the home's keychain entries behind.
- Nothing ties the device deletion at the backend to the local cleanup, so
  each consumer sequences it by hand and differently: the TUI never deletes
  the device (a deleted profile keeps occupying one of the seven device
  slots), the CLI's `logout --purge-device` wipes the *whole* home — every
  account's databases and `home.id` — and leaves the keystore entries behind
  (`docs/cli.md` said so, "until Step 78").

What already exists: Step 114 (ADR-0035) built the complete routine for
leaving a browser, `Kupferdraht::logout_client(client, PurgeOptions {
purge_device, purge_local }, password)` in `manager/purge.rs`: the backend
first (a refused password changes nothing locally), then the client is
removed from the manager's handle map and its storage closed (the store
refuses every later operation with `SessionExpired`), the account's
databases are deleted where they are the device's, and with `purge_local`
the account is forgotten (secrets, pending keystore writes flushed, registry
entry, active account). The wasm-only parts (the storage-server slot, the
server-mode branch of `deletes_databases`) are behind
`cfg(target_arch = "wasm32")` already.

**Correction to the write-up:** visibility is not the whole problem. The
module `manager::purge`, `Client::close_storage`, `MlsClient::close_storage`,
`KupferdrahtProvider::close`, `storage::account_files` and
`account_keys::forget_account_secrets` are all `cfg(any(target_arch =
"wasm32", test))` — in a native release build none of this code exists. The
step lifts those gates (keeping `delete_leftover_account_databases` and the
first-setup code gated: they stay browser-only).

## Decisions (2026-11-01, Volker)

Naming follows matrix-rust-sdk (ADR-0009), never Wire's or Kalium's
vocabulary:

| Act | Kupferdraht | matrix-rust-sdk | Mapping status |
|---|---|---|---|
| End the session at the backend; device and data stay | `Client::logout(false, None)` (existing) | `Client::logout()` | Drift (recorded) |
| Manager: end the session, mark the account signed out, drop the client, optional purge | `Kupferdraht::logout(&client, PurgeOptions, password)` — the public form of `logout_client` | `Client::logout()`; no analog for the registry | Wire extension |
| Drop everything local about an account, no session needed | `Kupferdraht::forget_account(account_id)` | the `Room::leave()` / `Room::forget()` pattern: the server act is *leave*, dropping what is stored locally is *forget* | Kupferdraht-specific |
| Delete the device at the backend | `Client::delete_device`, `PendingLogin::delete_device` (existing) | `Client::delete_devices` | Aligned |
| "Signed out" in the registry | `active_account()` is `None` after `logout` | `SessionChange::UnknownToken { soft_logout }`: token gone, local state kept | Wire extension |

1. **"Signed out" is the cleared active account (option A).** After a
   successful `Kupferdraht::logout` the active account is cleared when it
   was this one, and the manager drops the client from its cache; the
   registry entry, the databases and the keys stay. No schema change on
   `global.db` (it has no schema version — open.md M5; a status column would
   be an `ALTER TABLE` guarded by a column check, not a migration), no ADR.
   A status on `AccountEntry` (option B) waits until a multi-account picker
   needs to tell "signed out" from "not the active one". A `resume` of a
   signed-out account is the consumer's choice; it surfaces `SessionExpired`
   (the documented re-login signal) once the backend refuses the token.
   The same semantics reach the browser through the shared routine:
   `Client.logout()` without options now also clears `activeAccount`.
2. **`forget_account` is local only.** Releasing the device at the backend
   needs the account's access token, so a consumer that forgets an account
   without a session (the TUI's picker) cannot free the device slot; it
   deletes the device through a live client (`Kupferdraht::logout` with
   `purge_device`, `Client::delete_device` from another device) or leaves
   it. The browser keeps `Client.logout({ purgeLocal })` and gains no
   `forgetAccount` in this step (open.md §72 keeps that question; the tabs
   proxy parity would have to follow).
3. **Not `deactivate_account`** (the TUI's suggestion in its §8):
   `Account::deactivate` is matrix-rust-sdk's permanent server-side
   deactivation, the opposite of "keep everything". Not `destroy_account`
   (this file's earlier name): neither Matrix nor needed. Avoided as Wire or
   Kalium vocabulary: "soft/hard logout" as API names, "clear user data",
   "delete client", "deregister", "wipe".
4. **`PurgeOptions { purge_device, purge_local }` becomes public** as it is,
   the browser's `{ purgeDevice, purgeLocal }` shape since ADR-0035; a
   `uniffi::Record`, re-exported from `lib.rs`.
5. **A forgotten or unknown account is `InvalidInput`** from
   `forget_account`, as from `resume` — the registry entry is the anchor and
   goes last, so "unknown" means nothing else is left either. A half-
   forgotten account (directory already gone, keys or entry still there) is
   finished.
6. **The CLI follows the API:** `logout [--purge-device] [--purge-local]`
   on `Kupferdraht::logout` — the home-wide `remove_dir_all` goes; a new
   `account-forget --id <account>` on `forget_account`.

## Acceptance criteria

Observable through the public Rust API of `kupferdraht-core`, without the
consumer touching files or keystore ids:

- [x] **Sign out, keep data.** `Kupferdraht::logout(&client,
  PurgeOptions::default(), None)` revokes the session; afterwards
  `active_account()` is `None`, the account is still listed, its databases
  and keys are still there, and the manager no longer hands the client out.
  A later login for the same account reuses the local databases and the
  registered device.
- [x] **Forget the account.** `Kupferdraht::forget_account(account_id)`
  closes a live client this manager holds, deletes the account's databases,
  removes every account secret from the keystore (the seed where there is
  one, the two database keys, the storage-login key), removes the registry
  entry and clears the active account; afterwards `accounts()` does not
  list it. The same through `Kupferdraht::logout` with `purge_local` on a
  native (random-key) home.
- [x] **Release the device.** `Kupferdraht::logout` with `purge_device` and
  the password deletes the device at the backend first; a refused password
  leaves the local state untouched (the browser contract, ADR-0035, already
  pinned in `manager/purge_tests.rs`).
- [x] **Error contract.** `InvalidInput` for `purge_device` without a
  password, `BackendRejected` for a wrong one, `InvalidInput` for an unknown
  or already forgotten account, and the storage or keystore errors of the
  purge; nothing panics on persisted state.
- [x] **No consumer-side file knowledge.** `kupferdraht-cli logout` drops
  its `remove_dir_all`; the TUI can drop its directory wipe and its
  keystore call (consumer follow-up).
- [x] **Other accounts untouched.** Forgetting one account leaves the
  others' registry rows, keys and directories as they were.
- [x] **Browser behaviour unchanged** apart from decision 1:
  `Client.logout({ purgeDevice, password, purgeLocal })` keeps its
  semantics; the wasm gate passes.
- [x] **FFI parity:** `Kupferdraht.logout(client, options, password)` and
  `Kupferdraht.forget_account(account_id)` for Kotlin and Swift in the same
  commit.

## Tests first (red)

`manager/forget_tests.rs`, the Step 78 twin of `manager/purge_tests.rs`, on a
native home (random keys, no seed) against the login mocks:

- `logout_marks_the_account_signed_out` — a plain `logout`: the revocation
  is called once; still listed; directory and database key stay; the
  active account is `None`; the manager no longer caches the client.
- `logout_with_purge_local_forgets_a_native_account` — both database keys
  gone, directory gone, no longer listed, active account `None`.
- `forget_account_needs_no_session` — the client is dropped first; the
  account is forgotten entirely.
- `forget_account_closes_a_live_client` — the held client's store refuses
  with `SessionExpired`; the manager forgot it.
- `forget_account_finishes_a_half_forgotten_account` — the directory is
  already gone; keys and entry still go.
- `forget_account_refuses_an_unknown_account` — an unknown id and a second
  call after forgetting are `InvalidInput`.
- `forget_account_spares_the_other_accounts` — a second registry row with
  its keystore entry and directory survives.

Then the existing `purge_tests.rs` moves onto the public name, and the
Step 114 pin (`tests/step_114_browser_purge_pins.rs`) onto the renamed call.

## Implementation sketch

1. Lift the `cfg(any(target_arch = "wasm32", test))` gates on
   `manager::purge`, `Client::close_storage`, `MlsClient::close_storage`,
   `KupferdrahtProvider::close`, `storage::account_files` (the module; the
   `delete_leftover_account_databases` function stays gated) and
   `account_keys::forget_account_secrets`.
2. `manager/purge.rs`: `logout_client` → `pub async fn logout`; after the
   backend call always drop the client from the cache and clear the active
   account when it was this one; the internal `forget_account` becomes
   `forget_secrets_and_entry`; new `pub async fn forget_account(account_id)`
   = known? → close a cached live client → `delete_account_databases` where
   `deletes_databases(purge_local)` says so → `forget_secrets_and_entry`.
   `PurgeOptions` public, `uniffi::Record`, re-exported from `lib.rs`.
3. `wasm/manager.rs`: the call site moves to `logout`; `remote_resume.rs`
   to the renamed internal routine. wasm32 check + clippy (the browser
   tests run in CI — no Firefox here).
4. FFI (`ffi/manager.rs`): `logout(client, options, password)`,
   `forget_account(account_id)`.
5. CLI: `logout --purge-device --purge-local` on the manager call,
   `local_state_lines` truthful for the three outcomes; `account-forget
   --id`.
6. Docs: `docs/library.md` ("Signing out and forgetting an account" in the
   manager section), `docs/bindings.md` (the UniFFI `Kupferdraht` list and
   the browser `logout` note), `docs/cli.md` (`logout`, `account-forget`,
   the Step-78 sentence), `docs/api-mapping-matrix-rust-sdk.md` (three
   rows), `docs/ARCHITECTURE.md` (the purge paragraph), `CHANGELOG.md`
   `[Unreleased]` (Added, Changed), `open.md` (§72 bullet, M5).
7. Gate: the five plus the wasm32 check and clippy. No smoke: the backend
   surface (`POST /access/logout`, `DELETE /clients/{id}`) is unchanged and
   live-verified since Step 6e-2; the step adds local behaviour only.

## Gate (2026-11-01)

Green on the pinned toolchain (Rust 1.98): `cargo fmt --check`, 0 build
warnings, `clippy --all-targets -D warnings`, `cargo test --workspace
--no-fail-fast` (1295 passed, 0 failed — the seven new cases, the Step 114
cases on the public name, the Step 78 and Step 114 pins, the CLI's
local-state lines for all four flag combinations), `cargo deny check`,
`cargo check` and `clippy --lib -D warnings` for `wasm32-unknown-unknown`.
The browser tests run in CI (no Firefox here). No smoke (the backend
surface is unchanged). One pin moved: `tests/step_121_retired_device_pins.rs`
looked for `forget_account` in `remote_resume.rs`, which is
`forget_secrets_and_entry` now.

Found on the way (open.md §83): only `resume` enters the manager's client
cache; a client from `login` is the caller's alone, so `forget_account`
cannot close it and a `resume` right after `login` builds a second
client. The tests hold their client through `resume` and say so.

## Consumer follow-up (not this step)

The TUI re-pins to the next tag and moves `backend/session.rs` `logout` to
`Kupferdraht::logout` (its start-up path can then show the picker or the
form directly), replaces `delete_profile_task`'s directory wipe and
keystore call with `forget_account` (and, for the device, a `logout` with
`purge_device` while signed in), and closes its open.md §8 and §10.

## History

- **2026-09-10:** commissioned as "Local account purge (destroy_store)".
  The Kalium JVM audit found logout leaving `-wal`/`-shm` files with
  plaintext content behind. Kupferdraht's residue is ciphertext — the keys
  live in the OS keystore — but the structural gap is the same:
  `Client::logout(purge_device=true)` only deletes server-side state. What
  stayed behind: `base/accounts/<id>/` (ciphertext), the account's keystore
  entries (the sensitive residue: whoever holds the OS-keystore session can
  still open everything), the account's registry row. Phase-1 questions
  then: where the API lives (manager vs `Client`), the delete order
  (keys first makes the databases unrecoverable even if the directory
  delete fails), failure semantics (a half-purged account must be
  re-purgeable), FFI + CLI in the same step.
- **2026-09-30 (Step 110):** the browser's `logout({ purgeLocal: true })`
  (ADR-0035) planned as this purge's browser face rather than a second
  deletion path; Step 110 deferred it here.
- **2026-09-30 (Step 114):** the browser's purge landed on its own
  (Markus: browser only; this step adopts it natively):
  `Kupferdraht::logout_client` in `manager/purge.rs`. Its answers to the
  phase-1 questions: the backend first (a refused password changes nothing,
  the call can be repeated); the account's databases are closed through
  `Store::close` / `MlsClient::close_storage`, then deleted; keys are
  forgotten and flushed before the registry entry goes; the client is
  dropped from the manager's cache. A manager-level purge without a live
  client stayed this step's question (open.md §72).
- **2026-10-31:** the first Rust embedder's write-up (gap analysis,
  acceptance criteria, design notes) reviewed against `main` at `70fc378`;
  six corrections recorded above (the cfg gates, Step 78 itself, the
  legacy `global-db` id, the device needs a session, where `resume` fails,
  `global.db` without a schema version).
