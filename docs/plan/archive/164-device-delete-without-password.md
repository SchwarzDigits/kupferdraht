# Step 164 — An SSO user deletes a device without a password

**Status: MERGED 2026-11-18** — `ea61ac5` on `step/164-device-delete-without-password`, with
`ea508fe` (main merged in after Step 163a), merged to `main` as PR #107 (`3e8b0ed`);
locally fmt, 0 build warnings, clippy native and wasm (both wasm lines), 1035 tests in
31 suites, the CLI's 34, the browser suite (18 suites, 89 tests; `step_94_leader_tab`'s
known flake once, green on the rerun), `public_vocabulary` on the merged state; CI green
on the first run. Smoke on the reference machine: 10 ok (see Smoke below).
Commissioned 2026-11-18 (Volker), from Step 162's smoke: an SSO user logged
in through the CLI and could not delete the device afterwards. Before 0.6.0.

## Problem

Wire re-authenticates a device delete by the account's password — and an account
without one, an SSO user's, by nothing (`docs/wire/kalium-readings.md` #31, read for
this step):

- an account with a password: the password must come along (`403 missing-auth`
  without it, `invalid-credentials` for a wrong one);
- an account without a password: the delete goes through without one, and a password
  sent anyway fails (`invalid-credentials`); a SAML user with a password is also
  allowed without.

Kupferdraht asks for a password everywhere outside the login handle:

- `Client::delete_device(device_id, password: &str)` refuses an empty password;
- `Client::logout(purge_device, password: Option<&str>)` refuses `purge_device` without
  one, and with it `Kupferdraht::logout` (`PurgeOptions`);
- the FFI (`Client.delete_device`, `logout`), the browser (`deleteDevice(deviceId,
  password)`, `logout({ purgeDevice })`) and the CLI (`device-delete`,
  `logout --purge-device`) follow.

`PendingLogin::delete_device` (the device-cap recovery) already sends no password for an
SSO login, and `wire/http/clients.rs::delete_client` already takes `Option<&str>`. An SSO
user can therefore free a slot at login, but not delete a device or log out with
`purge_device` later. Step 162's smoke left two devices of SSO user 2 on the reference
machine for that reason.

## Scope

1. The password becomes optional where a device is deleted: `Client::delete_device`
   and `logout(purge_device)`; `None` sends no password, and the backend decides.
   BREAKING for the FFI and the browser signatures, with 162 and 163 in 0.6.0.
2. The CLI: `device-delete` and `logout --purge-device` without a password for an
   account that has none (phase 1: how the CLI knows — a flag, an empty prompt, or the
   account itself).
3. Docs: `docs/library.md`, `docs/bindings.md`, `docs/cli.md`, the api-notes entry of
   `DELETE /v14/clients/{id}` (the re-authentication rule, with its source),
   `CHANGELOG.md` **BREAKING**.

### Phase 1

- Whether the library should know that an account has no password (an SSO account:
  `sso_id` on `/self`, not kept today) and refuse a password it would send in vain, or
  leave it to the backend's answer.
- The CLI's way to say "no password".
- Live on the reference machine: an SSO user's delete without a password `200`; with a
  password `403`; a password user's without one `403 missing-auth`.

## Phase 1 (2026-11-18)

- **The library does not know the account's kind — decided (a).** The password becomes
  optional; `None` or an empty one sends none, and the backend decides (`missing-auth`
  for an account with a password). No `sso_id` kept, no migration, no new symbol — as
  matrix-rust-sdk's `delete_devices(…, auth_data: Option<…>)`.
- **The CLI's "no password" is an empty answer — decided (a).** The prompt reads
  "Account password (empty for an account without one):"; `--password-stdin` takes an
  empty line. On `missing-auth` the CLI says how to give the password.
- **Source compatibility:** the optional parameter keeps a call with a password valid in
  Kotlin, Swift and JavaScript; BREAKING for the Rust API only.

## Smoke

2026-11-18 on the reference machine (server 3), valid client flows only: 10 ok, 0 failed
(observations §108). SSO user 2 signed in through the identity provider; a delete with
a password refused (`invalid-credentials`); the two devices Step 162's smoke left
deleted without one; `logout --purge-device` without one. Password user 3: the purge
logout without a password refused (`missing-auth`, with the CLI's hint), then with it.

## Retrospective

- The old refusal was also pinned by a unit test that asserted the error kind
  (`purge_device_needs_the_password`); the red phase's grep for the message missed it,
  the implementation found it (LEARNINGS 5.103). It now asserts that the backend's
  refusal purges nothing.
- The first smoke runs failed with "No route to host": the app the session ran in
  lacked macOS's Local Network permission. Granted, the smoke went through.
- Not covered: the SAML user with a password (allowed without one, per the reading);
  the reference machine has no such account.

## Out of scope

- The device API's names (`DeviceInfo`, `rename_device`, `device_label`): Step 163d,
  which comes after this step.

## TDD sketch

- `delete_device(id, None)` sends `DELETE /clients/{id}` without a password field; with
  `Some(pw)` as today; the current device still refused.
- `logout` with `purge_device` and no password deletes the device and logs out.
- A backend's `missing-auth` surfaces as `BackendRejected` with its label.
- FFI and browser parity; the CLI's parse tests.
- Smoke on the reference machine: SSO user 2 deletes the two devices Step 162's smoke
  left, and a `logout --purge-device` of a fresh SSO login.
