# One remote device per user, held as a slot on the storage server

Condensed from `docs/adrs/0040-one-remote-device-per-user-storage-slot.md`.

Accepted 2026-10-02 (Step 120). Supersedes, in part, `rules/0035-…`.

- **Ask the slot before recovery** (`GET /v1/slot`, PIN token): only
  `200 {"slot":null}` is "no device"; any other failure is
  `ServiceUnavailable`. `recoveryExpected` = the slot has a label.
- **A new setup claims the slot, then labels it**: `claim_slot()` → retire
  the replaced device's Wire client (only a `Kupferdraht (Web)` one) → open,
  register → `set_slot_label(client id)`; label again at every resume.
  **Don't** delete a Wire client of another model.
- **Bind recovery data to the device** (version 2: client id in the
  header); refuse another device's data as `oldDevice` before the PIN
  service is asked. Don't read version 1.
- **`SQLITE_PERM` on open is `DeviceRetired`**; whichever way in met it —
  `resume` or a registration (Step 121) — then forgets the account's keys in
  this browser, so the next login offers recovery. **Don't** leave them: no
  call an embedder has drops them (`purge` with `purge_local` needs a
  `Client`), so a browser that keeps them is stuck for good.
- **`purgeDevice` releases the slot** (`delete_slot()`) after the
  databases are deleted.

## Revision History

- 2026-10-05: Widened the `DeviceRetired` bullet to both ways in (Step 121).
  ADR-0040's consequences already promise that a replaced browser "resets
  itself"; only `resume` did.
