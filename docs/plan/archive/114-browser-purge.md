# Step 114 — Leaving a browser: `logout({ purgeDevice, password, purgeLocal })`

**Status: MERGED 2026-09-30** — commissioned 2026-09-30 (Markus —
ADR-0035's remaining `logout` options). `29df4ab`. Merged to `main` as PR
#99 (replacing #95, same content with corrected commit authors;
`e7c1523`); CI green. Smoke 2026-09-30 (purge-device, purge-local).
Retrospective: `docs/LEARNINGS.md` §5.56. Archived with this header.

## What ADR-0035 asks

`logout` keeps the native shape and default — `logout({ purgeDevice,
password, purgeLocal })`, all optional:

- without options the session ends and everything local stays (today);
- `purgeDevice` (with the password, as natively) deletes the device at the
  backend and, in the server mode, its remote databases;
- `purgeLocal` deletes the account's seed, keys, local databases and
  registry entry in this browser (the device and its remote databases
  stay, so recovery data still restores it);
- both together leave a browser for good.

## Decisions (2026-09-30, Markus)

- **Close, then delete at once** (option 1). The VFS deletes a database
  only while it is closed, and a live client holds `kupferdraht.db` and
  `mls.db` open (the sync loop and the tabs layer's handles hold it too).
  `logout` with a purge therefore closes the account's two databases
  itself: `SqliteStore` and the MLS provider each swap their connection for
  an empty in-memory one, which closes the database; the store refuses
  every later operation with `SessionExpired`. No deferred deletion.
- **The server mode's page cache stays** with `purgeLocal` alone: the VFS
  has no call to delete only the cache, and with the keys gone it is
  unreadable. With `purgeDevice` the VFS deletes the cache with the
  server's database.

## Design

The order — the backend first, because a refused password must leave
everything as it was:

1. The backend, as the native `logout(purge_device, password)` does:
   `purgeDevice` deletes the device (password checked there; the session
   dies with it), otherwise the session is revoked. An error stops here —
   nothing local has changed, and the call can be repeated.
2. With a purge: close the account's two databases.
3. Delete the databases where they are the device's: in the server mode
   with `purgeDevice` (on the server, with the cache — over the VFS's
   connection, which is still admitted: the device deletion ended the Wire
   session, not the storage token); in the local mode with `purgeLocal`.
4. `purgeLocal`: forget the account's seed and keys, remove its registry
   entry (and the active account, when it was this one).
5. The manager forgets the closed client, so `resume` never hands it out
   again.

- **Core:** `Kupferdraht::logout_client(client, PurgeOptions { purge_device,
  purge_local }, password)` (manager; compiled for the browser and for
  tests); `Store::close` with a default body (additive); `SqliteStore` and
  `MlsClient` close their connection.
- **Binding:** the wasm `Client.logout(options)` takes `purgeLocal`; the
  wasm `Client` / `PendingLogin` hold the manager (for the purge and the
  recovery context) instead of the recovery context alone.
- **Tabs layer:** `logout` keeps its name and kind (mutation); the `.d.ts`
  declares `purgeLocal`.
- Rules for the embedder (ADR-0035): warn before `purgeLocal` while
  `recovery().state()` is disabled; after a purge the client is closed —
  drop it.

## Tests (red first)

- Natively, a seed home with the login mocks (the local mode's semantics):
  - a plain `logout` revokes the session and keeps seed, databases and
    registry entry;
  - `purgeLocal` revokes the session and removes seed, keys, databases,
    registry entry and the active account;
  - a purged client's store refuses with `SessionExpired`;
  - `purgeDevice` without a password is `InvalidInput`, with nothing
    deleted;
  - a refused device deletion purges nothing, also with `purgeLocal`;
  - `purgeDevice` deletes the device and — in the local mode — keeps the
    databases here; the client is closed;
  - both together leave nothing;
  - the manager forgets a purged client.
- Pins: the binding's `purge_local`, the tabs `.d.ts` declaration, the
  embedder guide.
- Smoke (test stage, server mode): `purgeDevice` in browser A → a resume in
  browser B (the same account's seed) meets `NothingStored` (the smoke
  111b left open); `purgeLocal` → `accounts()` is empty, and a login shows
  `keys: "missing"`.

## As built (2026-09-30)

- The purge is `Kupferdraht::logout_client` (`manager/purge.rs`): the native
  `Client::logout` first, then — with a purge — the client leaves the
  manager's cache, `Client::close_storage` closes both databases,
  `delete_account_databases` deletes them where they are the device's, and
  `purgeLocal` forgets seed and keys (flushed to the keystore before the
  registry entry goes) and clears the active account.
- Closing swaps the connection for an empty in-memory one
  (`std::mem::replace`); the browser's SQLite (`sqlite-wasm-rs`) installs
  an in-memory VFS as its default, so the stand-in opens there too.
  `SqliteStore` refuses afterwards with `SessionExpired` (one flag);
  `MlsClient` keeps an empty provider, so MLS operations find no group.
- The wasm `Client` / `PendingLogin` hold the manager (`Arc`) instead of
  the recovery context; the wasm `Kupferdraht` keeps the manager in an `Arc`.
- **The smoke corrected the plan's `NothingStored` expectation.** Browsers
  that share an account's seed share its device (a restore brings the
  recorded device back), so `purgeDevice` in one ends the others' sessions
  too: their `resume` is `SessionExpired`, not `NothingStored`.
  `NothingStored` stays the retention's answer; the deletion shows instead
  in the next login's empty databases.
- **Smoke (2026-09-30, test stage):** browser B (holding the account after
  112b's takeover) `purge-device` → `PURGE-DEVICE-OK`; browser A `resume`
  → `SessionExpired` (the shared device is gone); A `purge-local` → login
  `keys=present`, `registerDevice` finds empty databases (`recovery=
  disabled`, it was `enabled` before the deletion), `logout({ purgeLocal:
  true })` → `accounts=0`, a new login → `keys=missing
  recoveryExpected=true`.

