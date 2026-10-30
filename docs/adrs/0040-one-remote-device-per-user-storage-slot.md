# ADR-0040: Keep one remote device per user, held as a slot on the storage server

**Status:** Accepted
**Date:** 2026-10-02
**Deciders:** Markus Troßbach

## Context

This ADR supersedes, in part, ADR-0035 "Store an account's browser
databases on sqlite-remote-vfs, with keys from a per-account seed that
Kupferdraht seals with a PIN": the account as the unit of the seed, the first
setup through the native login path without deleting remote databases,
`recoveryExpected` from the device list, and recovery data that names only
the account. The rest of ADR-0035 stands. The rationale for changing
course is the device concept agreed with the storage side on 2026-10-02.

Under ADR-0035 every browser that set an account up created a seed of its
own; browsers sharing a seed shared a Wire device, and a new setup added
another device while the old one's databases stayed on the storage server
and its key could still write them. Which browser held "the" account was
nowhere recorded, so `recoveryExpected` guessed from the device list (a
client with the model `Kupferdraht (Web)`), and recovery data of an abandoned
seed opened as well as the current one's.

The storage side (sqlite-remote-server / -vfs 0.4.0) now keeps a **slot**
per user: the one key that may open the user's databases, with a label.
`claim_slot()` takes it — another key's databases are deleted and that key
can no longer open them (`SQLITE_PERM`); `set_slot_label()` labels it (the
holder only); `delete_slot()` gives it up with every database of the key;
`GET /v1/slot` (with the PIN token) says whether there is a device and
which. Kupferdraht registers its Wire client only once the account's databases
are open, so the client id exists after the claim, not before.

## Options Considered

### Option A: Keep the account as the unit (ADR-0035)

**Pros:**
- No change.

**Cons:**
- Several devices per user, none authoritative; a replaced setup's key
  still opens its databases; `recoveryExpected` is a guess; stale recovery
  data still opens.

### Option B: One remote device per user — the slot; the label from the token

The storage token carries the client id; `claim_slot()` takes the label
from it.

**Pros:**
- One call.

**Cons:**
- The client id exists only after the databases are open, and a new
  device may not open them before it holds the slot — a second claim with a
  fresh token and a reconnect would be needed.

### Option C: One remote device per user — claim, then label

`claim_slot()` without a label, the databases open, the Wire client
registered, then `set_slot_label(client id)`.

**Pros:**
- Fits Kupferdraht's order (storage before registration); no second token, no
  reconnect.

**Cons:**
- A short window in which the slot has no label ("being set up").

## Decision

**Option C.** In the browser's server mode:

1. **The slot before recovery.** A login without the account's seed asks
   `GET /v1/slot` (URL beside the storage WebSocket): `null` — no device;
   a label — the device; `""` — a device being set up. Any other answer is
   `ServiceUnavailable`, never "no device". `recoveryExpected` is "the
   slot has a label".
2. **A new setup replaces the device.** New seed → `claim_slot()` (another
   device's databases go) → the replaced device's Wire client is deleted,
   only if its model is `Kupferdraht (Web)` (with an empty replaced label: every
   other `Kupferdraht (Web)` client — the old device stopped during its setup)
   → the databases open, the new client is registered → `set_slot_label`.
   The label is set again at every resume (idempotent), so an interrupted
   setup heals.
3. **Recovery data is bound to the device** (format version 2): the
   header carries the client id and the creation time beside the account
   hash. `recover` compares the id with the slot's label before the PIN
   service is asked (`oldDevice`). Version 1 is unsupported, no migration.
4. **`DeviceRetired`.** An open the slot no longer admits is the new
   error; `resume` forgets the browser's keys of the account.
5. **`purgeDevice` releases the slot** after the databases are deleted.

## Consequences

### Positive

- One authoritative device per user; a replaced key can neither open nor
  write the databases.
- `recoveryExpected` and the recovery check are facts, not guesses; stale
  recovery data costs no PIN attempt.
- A replaced browser learns it (`DeviceRetired`) and resets itself.

### Negative

- A new setup deletes the previous device's history — by design; the app
  must warn.
- Existing recovery data (version 1) no longer opens; users make new data.
- Retiring the old Wire client relies on its model (`Kupferdraht (Web)`); in
  the crash case every other `Kupferdraht (Web)` client of the user is deleted.
- The browser needs storage server and VFS 0.4.0 or later.

### Neutral

- Native targets are unchanged; the local mode has no slot.
- No `clientId` in the token exchange — the label comes from
  `set_slot_label`.

## References

- Supersedes, in part: ADR-0035 (the account as the unit; the first setup
  without deleting remote databases; `recoveryExpected` from the device
  list; recovery data naming only the account).
- Builds on ADR-0036 (storage tokens), ADR-0038 (the VFS's failures),
  Step 119 (the takeover event).
- `docs/plan/120-device-slot.md`.

## Revision History

- 2026-10-02: Created and accepted (Markus — design approved before the
  implementation, Step 120).
