# Step 120 — One remote device per user: the storage slot

**Status: MERGED 2026-10-02** — commissioned 2026-10-02 (Markus — the
device concept agreed with the storage side; design approved the same
day). `eb4a810` (test isolation), `70ed54b`. Merged to `main` as PR #109
(`fbd171a`); CI green. Smoke 2026-10-02 against the test stage (storage
server and VFS 0.4.0, release wasm). Scope shipped: the account's storage
slot — `GET /v1/slot` before recovery, a new setup claims it and retires
the old device, the slot labelled with the client id, `DeviceRetired`,
recovery data v2 bound to the device, `purgeDevice` releases the slot
(ADR-0040). Retrospective: `docs/LEARNINGS.md` §5.63. Archived with this
header.

## Problem

ADR-0035 made the account the unit: one seed per account, the device
registered through the native login path with its device reuse, and no
remote database deleted for it. The storage side now keeps one remote
device per user — the Wire client Kupferdraht registers — as a **slot** on the
storage server: the slot names the key that may open the user's
databases, and its label is the client id. A new device replaces the old
one and deletes its databases; an old key can no longer open them
(`SQLITE_PERM`). Kupferdraht has to claim the slot, label it, answer "is there
a device, and which" before recovery, bind recovery data to the device,
and tidy up the replaced device.

## Design (approved 2026-10-02)

- **D1 — the slot before recovery.** A login without the account's seed
  asks the storage server `GET /v1/slot` with the PIN token (the URL is the
  storage WebSocket's, `https` for `wss`, `/v1/slot` for `/v1/ws`):
  `{"slot":null}` — no device; a label — the device; `""` — a device being
  set up (it has no recovery data yet). Every other answer (`404`, `5xx`,
  network) is `ServiceUnavailable`, never "no device" — that would replace
  the real one. `recoveryExpected` keeps its name and is now "the slot has
  a label" (before: a `Kupferdraht (Web)` client in the device list).
- **D2 — first setup and start over** (`registerDevice({ newSetup: true
  })`): new seed → the VFS registered → `claim_slot()` (replaces another
  device and deletes its databases) → the databases open → a new Wire
  client registered → `set_slot_label(client id)` → the replaced Wire
  client deleted, only if its `model` is `Kupferdraht (Web)`; with an empty
  replaced label (the old device crashed during its setup) every other
  `Kupferdraht (Web)` client of the user is deleted. Deleting uses the login's
  password when there is one, none otherwise (single sign-on).
- **D3 — recovery data v2**, bound to the device: the header carries the
  client id (as 8 bytes) and the creation time (ms) beside the account
  hash; no server address (configuration, not the user's file). `recover`
  compares the id with the slot's label **before** the PIN service is
  asked: another device's data — or no device, or one being set up — is
  `InvalidRecoveryData { reason: "oldDevice" }` and costs no attempt. v1
  data is `unsupported`; no migration.
- **D4 — `DeviceRetired`.** Opening a database the slot no longer admits
  (`SQLITE_PERM`: another device holds it) is the new error
  `DeviceRetired`; `resume` then forgets the browser's seed and keys of
  the account (as `purgeLocal` does), so the next login sees `keys:
  "missing"` and offers recovery or a new setup.
- **D5 — `purgeDevice`** deletes the Wire client (as today), closes the
  databases, then `delete_slot()` — the slot and all databases of the key
  — instead of deleting the two databases one by one.
- **ADR-0040** records D1–D5 and supersedes the points of ADR-0035 they
  change: the unit is the device (one seed per device); a new setup
  replaces the old device and deletes its databases; recovery data is
  bound to the device.
- **Not here:** `clientId` in the token exchange (not needed — the label
  comes from `set_slot_label`); an admin's central deletion; a PIN epoch.

## Tests (red first)

- **Unit, `recovery/data.rs`:** v2 round-trips with the client id and the
  creation time (145 bytes, version 2); v1 bytes are `unsupported`; client
  ids round-trip as Wire writes them (lower-case hex, no padding);
  anything else is refused.
- **Unit, `recovery/flow.rs`:** data of another device is refused as
  `oldDevice` before any request; the existing cases with the device.
- **Unit, `storage/remote/slot.rs`:** the slot URL from the WebSocket URL;
  `GET /v1/slot` — a slot, `null`, an empty label, `401` →
  `StorageAccessDenied`, `404` / `5xx` / unreachable → `ServiceUnavailable`.
- **Unit:** which clients a new setup deletes (the replaced one only as a
  `Kupferdraht (Web)` client; with an empty label every other `Kupferdraht (Web)`
  client; never the own one); `recoveryExpected` from the slot.
- **Unit, `storage/mod.rs`:** `SQLITE_PERM` on open is `DeviceRetired`.
- After 0.4.0: the flows (D2, D4, D5) against the test stage — **smoke**:
  first setup in a fresh browser (the slot gets the client id), recovery in
  a second browser, start over (the old client deleted, the old browser's
  `resume` → `DeviceRetired`), `purgeDevice` (slot and databases gone).

## As built (2026-10-02)

- sqlite-remote-vfs `rev = "01688f2"` (v0.4.0): `claim_slot()` without a
  label, `set_slot_label()`, `delete_slot()`. Both pins follow.
- `recovery/data.rs`: format version 2 — the client id as a 16-byte field
  (its text, zero-padded: the shared login mocks register `new-client-id`,
  and a byte compare with the slot's label needs no assumption about Wire's
  id format) and the creation time; 105-byte header, 153 bytes in all.
- `recovery/flow.rs`: `RecoveryDevice { account_id, client_id }` for sealing
  (the client's device id) and opening (the slot's label);
  `RecoveryServices` gains `slot_url`; `account_slot` buys a PIN token and
  asks the slot.
- `storage/remote/slot.rs`: `slot_url`, `fetch_slot`, `recovery_expected`,
  `clients_to_retire`.
- `storage/browser.rs`: `claim_account_slot`, `label_account_slot`,
  `delete_account_slot` on the server database's VFS — `is_slot_taken` →
  `DeviceRetired`, a refused token → `StorageAccessDenied`, else
  `ServiceUnavailable`. `browser_open_error`: `SQLITE_PERM` →
  `DeviceRetired`.
- `manager`: the login asks the slot (`FirstSetup.slot`); a new setup claims
  it after preparing the databases and retires the replaced client before
  registering (the pending login holds the password; best effort);
  `complete_registration` and `resume_with` label the slot (best effort,
  idempotent); `resume_with` forgets the account on `DeviceRetired`;
  `logout_client` releases the slot after `purgeDevice` deleted the
  databases.
- Tests changed: the manager's recovery tests ask a slot mock — on the token
  exchange's mock server: a third server per test exhausted httpmock's
  pool and hung the parallel run. `seed_login_tests`' device-list test
  became "without a PIN service no recovery is expected".
- Found on the way: `keystore::file_store`'s warning-capture test failed
  about once in thirty runs while it shared the unit-test binary (a test
  thread meets the warning's callsite with no subscriber; `tracing` caches
  that). It moved to its own binary, `tests/step_77_file_keystore_warning.rs`
  — 0 failures in 40 runs.

## Smoke (2026-10-02, test stage — storage server and VFS 0.4.0, release wasm)

Account 2, fresh browser profiles A–D plus the browser of earlier smokes
(a pre-slot seed):

| step | result |
|---|---|
| A: first setup | `recoveryExpected=false` (no slot yet); client `<client-id-1>` registered; slot claimed and labelled, no warning |
| A: `enable` | recovery data of 153 bytes (version 2) |
| B: `recover` | `recoveryExpected=true`; the device check passed — the slot's label is A's client id; `RECOVER-OK` |
| pre-slot browser: `resume` | `DeviceRetired`; reopened: `accounts=0` — forgotten |
| C: new setup | `recoveryExpected=true`; `DELETE /v14/clients/<client-id-1>` (A retired), new client `<client-id-2>` |
| B: `resume` | `DeviceRetired` |
| C: `purgeDevice` | `PURGE-DEVICE-OK` |
| D: fresh login | `recoveryExpected=false` — the slot is gone; D purged again afterwards |

