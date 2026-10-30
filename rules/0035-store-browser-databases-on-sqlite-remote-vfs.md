# Browser storage on sqlite-remote-vfs, keys from a per-account seed that Kupferdraht seals with a PIN

Condensed from `docs/adrs/0035-store-browser-databases-on-sqlite-remote-vfs.md`.

Supersedes decision item 3 (OPFS `sahpool`) of
`rules/0022-browser-persistence-sqlite3mc-over-opfs.md`; its other items,
item 6 (the keystore holds the database keys) included, stay in force there.

**In part superseded by ADR-0040 (2026-10-02)** — current rule:
`rules/0040-one-remote-device-per-user-storage-slot.md`: one remote device
per user (the storage slot), a new setup replaces the old device and
deletes its databases, `recoveryExpected` from the slot, recovery data
bound to the device. The bullets below marked *(→ 0040)* are read through
it; **don't** go back to a device per seed.

- **The browser differs from native in four things only:** where a new key
  of an account comes from, when a login opens the account's databases,
  where those databases live, and the recovery data. Don't change native
  behaviour or the native keystore for it; the only UniFFI change is new
  `KupferdrahtError` variants for the browser's outcomes. Browser API changes
  are additions and keep native names, defaults and the error model.
- *(→ 0040)* **The unit is the account.** Its seed, its keys and its `kupferdraht.db` /
  `mls.db` are what survives an eviction. `global.db` and the home id are
  device-level and stay exactly native: random key, random id, local only.
  Don't key them from a seed or put them on the server.
- **Storage:** an account's `kupferdraht.db` and `mls.db` open through
  `sqlite-remote-vfs` — server mode (with the VFS's browser cache) or local
  mode (IndexedDB). No OPFS. One VFS per database, SQLite3MC per database,
  rollback journal.
- **Keys are stored and loaded as natively** (same keystore ids, same
  code). Only a *new* key of an account differs: natively random, in the
  browser HKDF-SHA-256 from that account's seed with frozen neutral labels
  (`kupferdraht/v1/db-key/kupferdraht`, `…/mls`, `kupferdraht/v1/storage-login`) — no
  account and no environment in a label. Device-level keys stay random.
  Don't add a second key-loading path and don't change a shipped label.
- **One seed per account**, created only on an explicit first setup after
  authentication or taken from recovery data; never create a new seed in
  place of a lost one. Keep it in `IndexedDbKeyStore` as
  `kupferdraht.<home-id>.account-<account>-seed`, persisted before any database
  of the account is created. `logout()` keeps them, as natively;
  `logout({ purgeLocal: true })` deletes seed, keys, local databases and
  registry entry.
- **A pending login without the account's seed opens and persists nothing
  of the account** until `recover()` or `registerDevice({ newSetup: true })`
  succeeds; without the seed and without `newSetup`, key creation refuses
  with `RecoveryDataRequired`. Register the device only after that. A first
  setup first removes keys and databases the account has without a seed.
- **Seal the seed with a PIN inside Kupferdraht** and hand out only the sealed,
  versioned bytes (`recovery().enable` / `changePin`, `PendingLogin.recover`):
  OPRF (RFC 9497, P256-SHA256) with the PIN service plus Argon2id,
  AES-256-GCM; the header carries a SHA-256 hash of the account id, not the
  id. Don't hand the seed or the account's access token to the embedder, and
  don't make the embedder parse the bytes.
- **New recovery data after the first needs the current PIN**
  (`changePin(current, new)`, checked locally) — never let a running
  session alone mint recovery data under a new PIN.
- **PIN rules:** at least 6 characters after Unicode NFC (characters, not
  bytes); digits only is allowed, longer PINs with any characters too; no
  practical upper limit; refuse a short list of trivial PINs. Document it on
  `validateNewPin` so embedders don't build a six-digit-only input.
- *(→ 0040)* **Check the binding:** refuse recovery data of another account before
  asking the PIN service. `registerDevice` and `recover` then run the native
  login path with its device reuse (recorded device, or a new one replacing
  it with its history wiped — `client.deviceReplaced`); don't add a
  browser-only device check or delete remote databases for it.
- **The VFS gets a signer and a token source, never key material.** In
  server mode, open a store only while holding a storage token (ADR-0036).
- **Open an account's databases where a takeover can be requested**
  (`resume`, `recover`, `registerDevice`), never in
  `login`; take over only on request.
- **Surface outcomes as distinct, named errors:** `RecoveryDataRequired`,
  `HeldElsewhere`, `NothingStored`, `StorageAccessDenied`, `StorageFull`,
  `ServiceUnavailable`, `WrongPin` (attempts left), `TooManyAttempts`
  (retry after), `InvalidRecoveryData` (`notRecoveryData` / `unsupported` /
  `otherAccount`) — each a `KupferdrahtError` variant; every browser error's
  `name` is its variant. A stopped instance via
  `onEvicted("tab" | "device" | "access")`.
- **An account is listed in `accounts()` only while its seed is in the
  browser**; `purgeLocal` removes the registry entry with the seed.
  `resume` throwing `SessionExpired` means log in again (seed present),
  `RecoveryDataRequired` means only recovery data helps.
- *(→ 0040: `recoveryExpected` comes from the slot)* **Recovery is a step after the login, never on the login screen** (a
  login may be single sign-on). `PendingLogin.recoveryExpected` is `true`
  when the account has a registered `Kupferdraht (Web)` device — the browser
  registers its device with that model.
- **A failed `recover` leaves nothing** when the seed was not recovered;
  the pending login stays usable. A `recover` that hits `HeldElsewhere`
  keeps the recovered seed and continues with
  `registerDevice({ takeOver: true })` — never ask the PIN service a second
  time for it.
- **Keep the browser API additive, small and in matrix-rust-sdk's recovery
  vocabulary:** `PendingLogin.keys` is `"present"` or `"missing"`; the
  first setup is `registerDevice({ newSetup: true })`; `Client.recovery()`
  has `state`, `enable`, `changePin`, `verifyPin`; `logout({ purgeDevice,
  password, purgeLocal })` keeps the native shape; local mode has no
  recovery data. "Seed" is an internal word — no public name carries it.
