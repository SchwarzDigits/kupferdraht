# ADR-0035: Store an account's browser databases on sqlite-remote-vfs, with keys from a per-account seed that Kupferdraht seals with a PIN

**Status:** Accepted (the account as the unit, the first setup's device handling, `recoveryExpected` and the recovery data's binding superseded by ADR-0040)
**Date:** 2026-09-29
**Deciders:** Markus Troßbach

## Context

This ADR supersedes item 3 of ADR-0022 "Persist browser data in SQLite3MC over OPFS instead of IndexedDB per-record
encryption": the OPFS `sahpool` VFS (its dedicated-worker requirement stands). Its other items — SQLite3MC through
`sqlite-wasm-rs`, the same stores and migrations as native, the rollback journal, the IndexedDB keystore holding the
database keys — stand.

Browsers evict an origin's storage. OPFS and IndexedDB go together, and with them Kupferdraht's databases and the keys that
open them: MLS state, device identity and history are gone from that browser, and the user has to register a new
device. For a browser client that is meant to be a primary client, that is not acceptable. Storing the data elsewhere
only helps if the keys survive as well, and what brings the keys back must be safe to keep outside the browser.

`sqlite-remote-vfs` (Apache-2.0) keeps a database file on a `sqlite-remote-server`. SQLite runs unchanged in the
worker; a commit returns only after the server stored it; one lease per database fences an older client; the login
signs a server challenge with an Ed25519 key; a token source lets the server admit only clients with a valid access
token. The same VFS offers a local mode that keeps the file in the browser's IndexedDB, with a Web Lock per database.
With SQLite3MC above the VFS the server stores ciphertext only. The Step 108 spike showed that it links with Kupferdraht's
wasm SQLite, runs Kupferdraht's stores unchanged and makes a takeover detectable; a commit costs about one round trip plus
5–10 ms.

A short PIN can protect such keys only if every guess needs a server that counts: a PIN service evaluates an
oblivious pseudorandom function (RFC 9497, P-256) with a key of its own per user, limits the evaluations per user, and
sees neither the PIN nor the result. Both that service and the storage server admit a user only with a token of a
token exchange, which the account's access token buys.

Natively every database key is drawn at random when its database is first created, stored in the platform keystore
under a home-scoped id and loaded from there afterwards (ADR-0025, ADR-0029); the device-level `global.db` holds the
account registry.

Constraints from prior decisions: ADR-0021 (SQLite3MC engine), ADR-0023 and ADR-0026 (one active instance per origin,
leader tab), ADR-0025 (keystore ids and the login cookie label are scoped by a home id), ADR-0029 (a key for an
existing database is never replaced).

## Options Considered

### Option A: Keep OPFS and the IndexedDB keystore (status quo)

**Pros:**
- No server, no network round trip per commit.

**Cons:**
- An eviction loses the account in that browser.

### Option B: Remote VFS, random database keys in the IndexedDB keystore

**Pros:**
- The data survives an eviction on the server.

**Cons:**
- The keys do not: after an eviction the remote databases are unreadable and the VFS login key is lost.

### Option C: Remote VFS, keys from a seed; the embedder supplies seed and account id

**Pros:**
- Kupferdraht stays free of recovery.

**Cons:**
- The embedder has to learn and keep the account id, which Kupferdraht knows first-hand, and has to implement the PIN
  protection itself; the seed crosses into JavaScript.

### Option D: Remote VFS, keys from a seed; Kupferdraht hands out an opaque recovery secret the embedder protects

**Pros:**
- Kupferdraht owns the format and the binding; the embedder can protect the secret any way it likes.

**Cons:**
- The secret crosses into JavaScript as a string that cannot be wiped.
- The embedder needs the account's access token for its PIN protection, so the browser API has to hand it out.
- Every embedder implements the PIN protocol (OPRF, Argon2id, encryption) again.

### Option E: Remote VFS; one seed per account, sealed by Kupferdraht with a PIN; keys stored as natively, derived from the seed when created

Kupferdraht creates a seed per account, seals it under a key derived from a PIN and the PIN service, and hands the embedder
the sealed bytes. The browser stores keys exactly as natively; only where a new key of the account comes from differs.

**Pros:**
- The seed and the access token never leave Kupferdraht; only sealed bytes do.
- One implementation of the format and the PIN protocol, in Rust, for every embedder; Argon2id runs as WebAssembly in
  the worker, much faster than in JavaScript.
- The embedder keeps what is its own: PIN entry, reminders, where the bytes are kept, wording.
- Key ids, key loading, durability, the account registry, the stores and the manager stay the native code.
- Several accounts per browser, each with its own seed and recovery data.

**Cons:**
- Kupferdraht owns a data format and depends on the PIN service's protocol.

### Option F: Like E, one seed per browser origin, every key derived on each open, all three databases remote

The first draft of this ADR: the keystore holds only the seed; a derived keystore computes the keys of `global.db`,
`kupferdraht.db` and `mls.db` and the home id on demand.

**Pros:**
- One secret in the keystore.

**Cons:**
- A second way of loading keys beside the native one.
- One account per browser origin, and a seed of another account must be cleared before a login can continue.
- `global.db` is device-level, not an account's: keyed from an account's seed it cannot hold more than that account,
  and on a server it needs a storage token before the account is known.
- A home id derived from an account's seed is the same in every browser of that account, so two browsers share one
  login cookie label and revoke each other's cookie.

### Option G: Like E, but the recovery data seals the account's stored keys instead of a seed

**Pros:**
- No seed; the browser draws keys at random exactly as natively.

**Cons:**
- A key added by a later version is not in recovery data exported before; every such change needs a new export.

### Option H: Like E, but the seed is kept only in the recovery data, not in the browser

**Pros:**
- The browser holds only the keys, not the secret that yields them.

**Cons:**
- Recovery data can be made only while the seed is at hand — at the first setup; once lost, no new copy can be made
  from a running browser, and a key added later cannot be derived.

### Option I: Keys derived from the account password

**Pros:**
- Nothing new for the user to keep.

**Cons:**
- A password change re-keys everything; single-sign-on accounts have no password.

## Decision

**Option E.** Native storage, the native keystore and the shared API (`ClientBuilder`, the UniFFI surface) are
unchanged, except that `KupferdrahtError` gains the variants for the browser's outcomes (native never raises them). The browser differs from native in four things only: where a new key of an account comes from, when a
login opens the account's databases, where those databases live, and the recovery data.

1. **The unit is the account.** What must survive an eviction belongs to an account: its seed, the keys derived from
   it and its databases `kupferdraht.db` and `mls.db`. What belongs to the browser — `global.db` (the account registry) and
   the home id — is kept exactly as natively: a random key in the keystore, a random home id, both local. An eviction
   loses them; the next login writes the account's registry entry again.
2. **Storage.** An account's `kupferdraht.db` and `mls.db` open through `sqlite-remote-vfs`, in one of two modes the
   embedder chooses: *server* (a `sqlite-remote-server`, with the VFS's local copy in the browser as a cache that holds
   only acknowledged ciphertext) or *local* (IndexedDB only; an eviction still loses data in this mode, and there is
   no recovery data). `global.db` and the home id always stay local. OPFS is no longer used. One VFS registration per
   database; each database keyed with SQLite3MC; rollback journal.
3. **The seed.** One 32-byte seed per account, created in the worker on a first setup the embedder requests
   explicitly, once the account is authenticated, or taken from recovery data. The browser keystore keeps it beside
   the account's keys, written durably before any database of the account is created. Several accounts may each have
   a seed in one browser. Logging out keeps them, as natively; the embedder can ask to delete them.
4. **Keys are stored as natively; only their source differs.** An account's database keys — and in server mode its
   storage-login key — are stored in the keystore under the native ids and loaded by the native code. Where natively a
   missing key is drawn at random, the browser derives a key of an account from that account's seed (HKDF-SHA-256,
   neutral labels): one SQLite3MC key per database and the Ed25519 storage-login key. Device-level keys stay random. A derived key is deterministic — deriving it again for an
   existing database yields the same key — so ADR-0029's refusal applies in the browser only when the seed is missing.
5. **Nothing before the seed.** A login that finds no seed for the account opens and persists nothing of the account
   — no token, no database — until the embedder restores recovery data or requests a first setup; the device is
   registered only after that. Kupferdraht never creates a new seed in place of a lost one, and cancelling the pending
   login leaves nothing behind.
6. **Recovery data.** On request Kupferdraht seals the account's seed with a PIN and returns opaque, versioned bytes bound
   to the account; the embedder decides where they go and never interprets them. The key that seals them combines
   the PIN service's evaluation of the blinded PIN with Argon2id of the PIN, so every guess needs the service and a
   leaked service key still leaves Argon2id. Because the browser keeps the seed, the data can be made again and the
   PIN changed as long as the browser holds the account — but only with the current PIN, so a running session alone
   cannot mint recovery data under a PIN of someone else's choosing. After an eviction the embedder passes the bytes and the
   PIN back; Kupferdraht opens them and restores. Kupferdraht gets the PIN service's token from the token exchange itself. The
   seed and the account's access token never leave Kupferdraht.
7. **Binding to account and device.** The recovery data carries a hash of the account id, so Kupferdraht refuses data of
   another account before it asks the PIN service. Through the databases it keys, a seed belongs to exactly one
   device: the same recovery data in a second browser is the same device, which takes over. After opening restored
   databases the native device reuse applies, as on every login: the recorded device while the backend still has it,
   else a new device that replaces it with the superseded MLS state and history wiped. The seed stays the same, so
   the keys, the storage namespace and the recovery data stay valid, and no remote database is orphaned.
8. **Layering.** The VFS receives a signer (algorithm, public key, sign function) and a token source, never key
   material.
9. **Access.** In server mode the storage server admits only connections with a token bound to the storage-login key,
   which Kupferdraht obtains from the token exchange with the account's access token (ADR-0036). A store opens only once a
   token is held. Local mode has no gate.
10. **Outcomes the embedder sees.** A database held elsewhere is taken over only on an explicit request, made on the
    call that opens it; the fenced client learns it through `onEvicted`, never silently — and the same callback tells
    a running instance that its storage access was withdrawn. A resume that finds no database on the server reports
    "nothing stored"; a restore then simply registers a new device. A refused token reports "no storage access", an unreachable service "unavailable". A
    wrong PIN reports the attempts left, a reached limit the time to wait. Each is a distinct error the embedder can
    branch on.
11. **Retention.** The storage server deletes a database that nobody used for a configurable time, 180 days by
    default, so that storage identities nobody can recover any longer do not stay forever.

## Consequences

### Positive

- An eviction no longer loses the account: the recovery data, the PIN and a login bring back the same databases and
  the same device.
- The server stores ciphertext only; the seed, the keys and the account's access token never leave Kupferdraht, and what
  leaves is sealed.
- A stolen copy of the recovery data is useless without the PIN, and every guess at the PIN needs the user's account
  and the PIN service's limit.
- Key ids, key loading, their durability (Step 104), ADR-0029, the account registry, the stores, the migrations and the
  manager stay one code path on every target; the browser adds a key source, a later database open, a storage location
  and the recovery data.
- Several accounts can live in one browser, each with its own seed and recovery data.
- The native and shared API does not change; the browser API grows only by additions.

### Negative

- Every commit is a server round trip (about one RTT plus 5–10 ms, about nine pages per single-event commit); a long
  catch-up is slow unless commits are grouped (ADR-0037).
- Opening and committing need the server; offline, only what the local copy covers is readable.
- Kupferdraht owns a data format and a PIN scheme, and depends on the PIN service's protocol; the WebAssembly module grows
  by an OPRF, Argon2id and AES-GCM.
- Sealing and opening take a second or more (Argon2id with 64 MiB) and need the PIN service; without it, recovery data
  cannot be made or opened.
- Recovery data and PIN together are a portable device identity: whoever holds both and can log in to the account
  can read its stored history. The PIN itself passes through JavaScript as the user types it.
- The seed sits in the browser keystore beside the keys it yields: whoever can read the keystore holds the device
  identity, as with the keys themselves.
- When an account has no seed in the browser, the embedder must choose between restoring and a first setup; a first
  setup by mistake registers a new device and leaves the old remote databases to retention.
- After an eviction the account registry is empty until the next login.
- The app must be cross-origin isolated (COOP/COEP, `worker-src blob:`), and a storage server, a token exchange and a
  PIN service must be operated.
- The VFS is at version 0.x.
- Local mode keeps the eviction risk.

### Neutral

- Forward secrecy of MLS state now also depends on the server's copies (ciphertext under seed-derived keys) — known
  and accepted.
- ADR-0023's per-origin lease stays for tabs; terminating the evicted worker is no longer needed to release OPFS
  handles.
- After an eviction the browser gets a new home id and with it a new login cookie label; the old cookie expires at the
  backend.

## Implementation Notes

- Shared core, native behaviour unchanged:
  - The one place a new database key is generated tells device-level ids (`global.db`: random on every target) from
    an account's ids (random natively; in the browser derived from the account's seed, and refused with
    `RecoveryDataRequired` when there is none).
  - The login splits into authenticating (HTTP only: credentials, the self user) and assembling (keys, databases,
    the pending device registration); natively both run back to back as today.
  - The browser registers its device with the model `Kupferdraht (Web)`, so the account's device list shows which devices
    a browser holds.
- Browser entry point, additive to today's surface and as close to the native API as the browser allows: native
  names, native defaults and the native error model; what only the browser needs follows matrix-rust-sdk's recovery
  vocabulary (ADR-0009). "Seed" stays an internal word and appears in no public name.
  - Every thrown error carries a `name`: the `KupferdrahtError` variant, as Kotlin and Swift get it typed
    (`SessionExpired`, `TooManyDevices`, `KeyUnavailable`, …). The browser's new outcomes are new `KupferdrahtError`
    variants — additive; native never raises them, but an exhaustive match in Kotlin or Swift must add them.
  - One behaviour of today's browser surface changes: on a browser without the account's seed, `registerDevice()`
    alone throws `RecoveryDataRequired`; a first setup must say so with `newSetup: true` (or restore with `recover`).
    Every other change is an addition.
  - `Kupferdraht.open(options, onEvicted)`: `options` gains `storage` (`{ mode: "server", url, tokenExchangeUrl,
    pinServiceUrl }` or `{ mode: "local" }`); `onEvicted` receives the reason: `"tab"` (another tab took over),
    `"device"` (another browser took over a database) or `"access"` (the storage token was refused while running).
  - `Kupferdraht.validateNewPin(pin)` → `"ok"`, `"tooShort"` or `"tooCommon"`. A PIN has at least 6 characters, counted
    as characters after Unicode NFC, not as bytes. It may consist of digits only, and it may be longer and contain
    letters, digits and other characters; there is no upper limit a user would reach. `"tooCommon"` refuses a short
    list of trivial PINs such as `123456` or `000000`. So an embedder shows a text field that accepts any characters,
    not a keypad for exactly six digits; it may offer digits first and explain that more is allowed. This goes into
    the doc comment of `validateNewPin`.
  - `kupferdraht.resume(accountId, { takeOver })` gains the takeover option and throws `NothingStored` when the server
    deleted the databases (retention). `SessionExpired` means a new login is enough (the keys are still here);
    `RecoveryDataRequired` means the account's keys are gone and only recovery data brings them back. `resume` takes
    the backend's address from the account's registry entry, so it can renew the session and fetch a storage token
    before any database of the account is open.
  - An account is listed in `accounts()` only while its seed is in the browser: `logout({ purgeLocal: true })`, which
    deletes the seed, also removes the registry entry. Since the registry and the keystore are evicted together, an empty list before a
    login means "evicted or never set up here", a listed account whose `resume` throws `SessionExpired` means "log in
    again, no recovery data needed".
  - `PendingLogin` gains `keys` (`"present"` or `"missing"`: whether this browser holds the account's seed),
    `recoveryExpected` (`true` when the account has a registered device with the model `Kupferdraht (Web)`: recovery data
    exists somewhere and brings that device back, while a first setup would add another device — a hint, not a
    guarantee; filled whenever `keys` is `"missing"`), `recover(data, pin, { takeOver })` and `cancel()`;
    `registerDevice()` keeps its native shape and gains the options `{ newSetup, takeOver }`.
  - `registerDevice` with `keys` `"present"` is the native call: it opens the account's databases and applies the
    native device reuse — the recorded device while the backend still has it, else a new device that replaces it
    (the superseded MLS state and history wiped), else, with no databases, a plain first registration. With
    `"missing"` it needs `newSetup: true` — then it first removes keys and databases the account has in this browser
    without a seed (left by builds before this ADR; they cannot be backed by recovery data), creates the seed,
    persists it and registers — and otherwise throws `RecoveryDataRequired`. A retry after `TooManyDevices` is the
    same call; the seed is kept, not made again.
  - `recover` checks the data, recovers and persists the seed, then runs the same native path, so it brings back the
    recorded device, or a new device when the backend removed it or retention deleted the databases; the recovery
    data stays valid either way. `client.deviceReplaced` tells the embedder that a new device replaced the recorded
    one (older messages are gone). A `recover` that fails before the seed is recovered (`WrongPin`,
    `TooManyAttempts`, `InvalidRecoveryData`) leaves nothing behind; the pending login stays usable, and a first
    setup works right after it. A `recover` that recovered the seed but finds a database held elsewhere keeps the
    seed (`keys` becomes `"present"`) and throws `HeldElsewhere`; the takeover continues with
    `registerDevice({ takeOver: true })`, which does not ask the PIN service again. `cancel()` leaves nothing behind,
    also after a first setup that failed.
  - `Client` gains `recovery()` with `state()` (`{ state: "disabled" }` or `{ state: "enabled", exportedAt }`),
    `enable(pin)` (the first recovery data, a `Uint8Array`; only while disabled), `changePin(currentPin, newPin)`
    (new recovery data under `newPin` — also, with the same PIN twice, a new copy of lost data; the current PIN is
    checked locally, a wrong one throws `WrongPin`, so a running session alone cannot mint recovery data under a PIN
    of someone else's choosing) and `verifyPin(pin)` (local, for reminders). In local mode there is no recovery data:
    the recovery calls refuse with `InvalidInput`.
  - `logout` keeps the native shape and default — `logout({ purgeDevice, password, purgeLocal })`, all optional:
    without options the session ends and everything local stays, so the next login needs no recovery data;
    `purgeDevice` (with the password, as natively) deletes the device at the backend and, in server mode, its remote
    databases; `purgeLocal` deletes the account's seed, keys, local databases and registry entry in this browser
    (the device and its remote databases stay, so recovery data still restores it). Both together leave a browser
    for good. `purgeLocal` is named so that the native local purge (Step 78) can adopt it.
  - `login`, `finalizeSso` and `accounts()` keep their shape and their native behaviour. Databases
    of an account open in `recover`, `registerDevice` and `resume`, not in `login`, so a takeover never needs a second
    login.
  - The new `KupferdrahtError` variants: `RecoveryDataRequired`, `InvalidRecoveryData` (with `reason`:
    `notRecoveryData`, `unsupported` — an unknown format version or a key version the PIN service retired — or
    `otherAccount`), `WrongPin` (with `attemptsRemaining` when the PIN service counted it), `TooManyAttempts` (with
    `retryAfter`), `NothingStored` (from `resume`), `HeldElsewhere`, `StorageAccessDenied`, `StorageFull`,
    `ServiceUnavailable` (storage server, token exchange or PIN service unreachable; try again later), `Evicted`.
  - Rules for the embedder, which Kupferdraht does not enforce: warn before `logout({ purgeLocal: true })` while
    `recovery().state()` is disabled; offer no recovery data in local mode.
- The user flow the API is shaped for. The login screen knows nothing of recovery data, because a login may be
  single sign-on; recovery is a step of its own after it, and only when needed:
  1. App start: a listed account resumes straight into the app; `SessionExpired` leads to the login screen with
     "session expired"; an empty list leads to the login screen.
  2. Login (password or single sign-on) → `PendingLogin`. With `keys` `"present"`, `registerDevice()` and into the
     app — the user sees nothing more.
  3. With `"missing"`, one screen "set up this browser": with `recoveryExpected` the main action is "choose recovery
     data" and "I have none" is a secondary link with a warning (older messages do not come back, a new device is
     added); without it the main action is "set up" and "I have recovery data" is the secondary link. Back calls
     `cancel()`.
  4. Restore: PIN entry → `recover` → into the app (with `deviceReplaced`: "older messages could not be restored");
     `WrongPin`, `TooManyAttempts`, `InvalidRecoveryData` and `HeldElsewhere` (ask, then
     `registerDevice({ takeOver: true })`) each get their own message.
  5. First setup: `registerDevice({ newSetup: true })` (on `TooManyDevices` the device list to free a slot, then the
     same call), then "secure your access": choose a PIN (`validateNewPin`, entered twice) → `recovery().enable(pin)`
     → the embedder offers the bytes as a download. Skipping leaves the recovery state disabled, for reminders.
  6. Local mode has no recovery data: with `"missing"` the embedder sets up at once, without steps 3–5.
- The call chains per case (`→` a call, `⇒` an outcome, `!` a thrown error):
  - Known account, session valid: `open` → `accounts()` → `resume(id)` ⇒ client.
  - Session expired, or after a plain `logout()`: `resume(id)` ! `SessionExpired` → login screen →
    `login(…)` or `initiateSso` + `finalizeSso` ⇒ `keys "present"` → `registerDevice()` ⇒ client.
  - Held elsewhere: `resume(id)` or `registerDevice()` ! `HeldElsewhere` → ask → the same call with
    `{ takeOver: true }`.
  - Empty browser, recovery data at hand: `accounts()` ⇒ `[]` → `login(…)` ⇒ `keys "missing"`,
    `recoveryExpected true` → `recover(data, pin)` ⇒ client (`deviceReplaced` when the recorded device was gone); on
    `WrongPin` again with another PIN; on `HeldElsewhere` → `registerDevice({ takeOver: true })`.
  - Empty browser, first setup: `login(…)` ⇒ `keys "missing"` → `registerDevice({ newSetup: true })` ⇒ client (on
    `TooManyDevices` → `devices()` → `deleteDevice(id)` → the same call) → `validateNewPin(pin)` →
    `client.recovery().enable(pin)` ⇒ bytes the embedder offers as a download.
  - Back out of setting up: `cancel()` — nothing stays.
  - Change the PIN, or replace lost recovery data: `client.recovery().changePin(current, new)` ⇒ bytes.
  - Log out: `logout()` keeps this browser, as natively; `logout({ purgeLocal: true })` forgets it (recovery data
    still restores the device); `logout({ purgeDevice: true, password, purgeLocal: true })` leaves it for good.
  - Local mode: `login(…)` → `registerDevice({ newSetup: true })` if `"missing"`, else `registerDevice()`.
- Recovery data: a plaintext header (format version, the PIN service's key version, the Argon2id parameters, salt,
  nonce, a SHA-256 hash of the account id) and the seed sealed with AES-256-GCM, the header as associated data. The
  hash is enough to refuse data of another account before the PIN service is asked, without showing a finder whose
  data it is (the ids of a known set of accounts can still be tested against it). File key =
  HKDF-SHA-256(OPRF output ‖ Argon2id(PIN, salt, 64 MiB, t = 3, p = 1), info `kupferdraht/v1/recovery-data`). A wrong PIN
  shows as a failed decryption; the attempt has been counted.
- PIN service: RFC 9497 OPRF mode, suite P256-SHA256, client side in Rust; the service's fixed interop values are
  checked in Kupferdraht's tests. Tokens for it come from the token exchange with the audience `pin`.
- The seed lives in the existing `IndexedDbKeyStore` (IndexedDB, wrapped with a non-extractable AES-256-GCM WebCrypto
  key) under the account's home-scoped id `kupferdraht.<home-id>.account-<account>-seed`; `registerDevice({ newSetup })`
  and `recover()` await its persist before they open a database.
- HKDF labels (frozen once shipped — a changed label orphans every database or makes recovery data unreadable):
  `kupferdraht/v1/db-key/kupferdraht`, `kupferdraht/v1/db-key/mls`, `kupferdraht/v1/storage-login`, `kupferdraht/v1/recovery-data`. The seed
  is the account's, and an account belongs to one backend, so the labels name neither an account nor an environment.
- VFS errors: `SQLITE_BUSY` at open means held elsewhere, `SQLITE_CANTOPEN` without create means nothing stored,
  `SQLITE_AUTH` means the token was refused, `SQLITE_FULL` in local mode means the quota is full.
- Database names follow the home layout; on a server the storage-login key's subject separates accounts, locally the
  namespace is the home id.
- The spike's route (make each VFS's SQLite3MC wrapper the default before a store opens) becomes an explicit VFS name
  per store open.

## References

- Supersedes ADR-0022 item 3 (OPFS VFS) — partial.
- Builds on ADR-0021, ADR-0022 item 6 (the keystore holds the database keys), ADR-0023, ADR-0025, ADR-0026.
- Constrained by ADR-0029.
- Related: ADR-0036 (storage access gate), ADR-0037 (commit a drained page).
- `docs/plan/108-remote-vfs-spike.md`.

## Revision History

- 2026-09-29: Created (Accepted — Markus), after the Step 108 spike. Numbers 0031–0034 are held by open pull
  requests.
- 2026-09-29: Revised before merge (Markus): one seed per account instead of per origin; keys stored as natively and
  derived from the seed only when created (Option F, the first draft, rejected); `global.db` and the home id stay
  device-level and local as natively; ADR-0022 item 6 stands; `seedState` loses `"otherAccount"` and `kupferdraht.reset()`
  is dropped.
- 2026-09-29: Implementation notes extended (Markus): the user flow (recovery as a step after the login), the
  `accounts()` invariant and the meaning of `resume`'s errors, `recoveryExpected` and the device model
  `Kupferdraht (Web)`; the call chains per case; `registerDevice` with a removed device throws `DeviceRemoved`; what a
  failed restore leaves behind, and the takeover after a restore continuing with `registerDevice`.
- 2026-09-30: Revised before merge (Markus): the storage-login key and `Kupferdraht.open` lose the environment name; the
  first setup is `registerDevice({ newSetup: true })` (`createSeed()` and `SeedAlreadyPresent` dropped); new recovery
  data after the first needs the current PIN (`recovery().changePin`), no longer an embedder rule; the header carries a
  hash of the account id instead of the id; the browser names follow matrix-rust-sdk's recovery vocabulary (`keys`,
  `recover`, `recovery()` with `state`, `enable`, `changePin`, `verifyPin`; `logout({ keepKeys })`).
- 2026-09-30: Aligned with the native API (Markus): `registerDevice` and `recover` use the native device reuse
  (`DeviceRemoved` dropped, `NothingStored` only from `resume`, `client.deviceReplaced` added); `logout` keeps the
  native shape and default (`purgeDevice` with the password, plus `purgeLocal`); every browser error is named after its
  `KupferdrahtError` variant, and the new outcomes are new variants; a first setup removes keys and databases the account
  has without a seed.
- 2026-10-02: In part superseded by ADR-0040 "Keep one remote device per
  user, held as a slot on the storage server": one remote device per user
  (the storage slot) instead of the account as the unit; a new setup
  replaces the old device and deletes its databases; `recoveryExpected`
  from the slot; recovery data bound to the device. The rest stands.

- 2026-10-07: Same decision, the labels renamed once (ADR-0041, the
  clean-cut rename to Kupferdraht, before any release): the frozen HKDF
  labels now read `kupferdraht/v1/…`, the databases `kupferdraht.db` and
  `mls.db`. The freeze resumes from here; the body above keeps the names
  it was decided with.
