# Step 164 — An SSO user deletes a device without a password

**Status: commissioned 2026-11-18** (Volker), from Step 162's smoke: an SSO user logged
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
