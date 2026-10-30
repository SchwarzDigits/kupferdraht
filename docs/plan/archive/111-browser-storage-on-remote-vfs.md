# Step 111 — The browser's databases on sqlite-remote-vfs

**Status: MERGED 2026-09-30** — commissioned 2026-09-30 (Markus —
ADR-0035; ADR-0036, PR #81, merged as `52916c3`). 111a `534ab91` +
`4e4c75f` (PR #86, `c5d1fd6`), 111b `69afbe5` (PR #87, `620fc12`), 111c
`cc33af4` (PR #88, `bf8f0cb`), 111d `efea6b2` + `038150e` (PR #89,
`60258e2`); CI green for each. Smokes 2026-09-30 on the ODJ test stage
(local mode; server mode; two-browser takeover; reload reclaim); the
two-home takeover and `purgeDevice` followed with Steps 112b and 114.
Retrospectives: `docs/LEARNINGS.md` §5.50–5.52. Archived with this header.

- **111a** (`step/111a-local-vfs-storage`) — the VFS's local mode replaces
  OPFS for all three databases. Testable in CI without a server.
- **111b** (`step/111b-server-vfs-storage`) — server mode for an account's
  databases: the storage-login key from the seed as the VFS's signer, the
  storage token from the token exchange (ADR-0036), `resume` renewing the
  session before it opens anything.

## Problem

The browser keeps its databases in OPFS through the `sahpool` VFS. ADR-0035
moves them to `sqlite-remote-vfs` (v0.1.0): on a `sqlite-remote-server`
(111b), or in IndexedDB in the VFS's local mode (111a) — one storage path,
no OPFS.

## Decisions (2026-09-30, Markus)

- **D1 — two sub-steps** as above; 111a stands on its own: without a
  `storage` option `Kupferdraht.open` uses the local mode, which is what the
  browser does today (the data stays in this browser).
- **D2 — cross-origin isolation is required**, in both modes: the VFS
  blocks with `Atomics.wait` on a `SharedArrayBuffer`, which browsers grant
  only to a cross-origin isolated page (COOP/COEP). `Kupferdraht.open` checks
  `crossOriginIsolated` first and fails with a message naming the headers,
  instead of hanging later.
- **D3 — the home id lives in the keystore**, as the entry
  `kupferdraht.home-id`: it must live exactly as long as the keys whose ids carry
  it, and there is no OPFS file any more. An eviction loses both together;
  the next open creates a new one (and so a new login cookie label).
- **D4 — the local namespace is the home id**: the databases are the
  IndexedDB databases `sqlite-remote-vfs-local/<home-id>/<path>`.
- **D5 — no migration.** OPFS data of earlier builds stays where it is;
  Kupferdraht does not touch it.

## 111a — design

- **Dependency:** `sqlite-remote-vfs` (SchwarzDigits, Apache-2.0, not on
  crates.io) as a wasm-only dependency, pinned to `v0.1.0` (`66d436a`;
  `v0.1.1`, `7a1e1cf`, since 111b);
  `deny.toml` allows that git source. `sqlite-wasm-vfs` (the `sahpool`)
  goes.
- **One VFS per database** (the VFS allows one database per registration and
  one connection per database). `storage::browser` keeps a registry per
  worker: a database is *prepared* — its VFS registered with
  `Config::local(<home-id>)`, takeover as the instance was opened — in an
  async step before the synchronous open, which names that VFS explicitly
  (`encrypted_name()`, SQLite3MC above it). The async paths that open
  databases prepare them: `Kupferdraht.open` (`global.db`), opening an account's
  storage (`kupferdraht.db`, `mls.db`), `resume`.
- **Opens name their VFS.** The three open sites (`storage::global`,
  `storage::sqlite`, the `mls.db` provider) open through one
  `storage::open_connection(path)`: natively `Connection::open`, in the
  browser with the prepared VFS.
- **Existence and deletion** in the browser go through the VFS: a database
  exists when its IndexedDB database does (`indexedDB.databases()`);
  deleting an account's databases (Step 110's first setup and `cancel`)
  uses `RemoteVfs::delete_database` (synchronous on a prepared database, so
  `cancel` stays synchronous; a first setup prepares the databases before it
  removes leftovers).
- **Instance lease unchanged** (ADR-0023): one instance per origin; a
  takeover evicts the other tab. The per-database Web Locks of the local
  mode sit under it; a takeover passes `takeover` to the VFS too.
- **Errors** (new `KupferdrahtError` variants, named in the browser): an open
  refused with `SQLITE_BUSY` → `HeldElsewhere`, `SQLITE_FULL` →
  `StorageFull`, an instance evicted by another tab → `Evicted`.
  `onEvicted` receives the reason — `"tab"` in 111a (`"device"` and
  `"access"` come with the server mode).
- **`Kupferdraht.open({ storage })`** accepts `{ mode: "local" }` (the default);
  `"server"` comes with 111b.

## TDD (red first)

- Browser (`tests/step_111a_local_vfs_storage.rs`, dedicated worker, CI):
  `Kupferdraht.open` without a `storage` option keeps `global.db` in IndexedDB
  under the home id; an account's store written through the local VFS is
  read back after its connection is closed and the database prepared anew;
  the home id is the keystore entry `kupferdraht.home-id` and stable across
  opens; a second instance's open of a database held elsewhere is
  `HeldElsewhere`; a takeover gives `onEvicted` the reason `"tab"`; an
  account database without its key is still `KeyUnavailable` (Step 96)
  over the VFS.
- The OPFS tests (`step_91_opfs_storage`, `step_93_home_id_wasm`,
  `step_96_no_silent_key_replacement_wasm`) become their VFS counterparts.
- Native (unit): the SQLite result codes map to the new variants.
- Native pins: no `sahpool` / OPFS pool left, the isolation check exists,
  `docs/bindings.md` names the headers.

## 111a — as built (2026-09-30)

- `storage/browser.rs` rewritten: the instance lease stays; the OPFS pool,
  its cipher wrap and pool files are gone. `open_browser_storage` checks the
  dedicated worker and `crossOriginIsolated`, takes (or takes over) the
  lease, resolves the home id from the keystore entry `kupferdraht.home-id`
  (durable on creation). A per-worker registry maps each database path to
  its `RemoteVfs` (`Config::local(<home-id>)`, takeover as opened) and
  whether it exists (`indexedDB.databases()` at prepare, updated on open and
  delete). `wait_for_browser_eviction` yields `BrowserEviction::Tab`.
- `storage::open_connection` is the one open of `kupferdraht.db`, `mls.db` and
  `global.db`; `browser_open_error` maps `SQLITE_BUSY` / `SQLITE_FULL`.
  The stores' `lock()` fails as `Evicted`. `home_id.rs` resolves the
  browser's id from the opened storage. `prepare_account_databases` runs
  before an account's keys are ensured (open, resume, first setup).
- `wasm::Kupferdraht::open`: keystore first, then the storage, the base dir
  `kupferdraht`, `storage: { mode: "local" }`, `onEvicted(reason)`.
- Tests: the OPFS pins of Steps 91, 93 and 96 became
  `step_111a_local_vfs_storage` (10) and `step_111a_eviction_reason`; the
  Step 91/92 browser tests and the Step 91 build pins follow the new API.
  Dropped with OPFS: the raw at-rest byte check (the VFS's own tests show
  it stores only ciphertext with SQLite3MC) and the native file opened in
  the browser (no longer a file).
- Gate 2026-09-30: fmt, 0 warnings, clippy (native all targets, wasm lib),
  1076 native tests, 24 browser tests, `cargo deny` — green.
- Smoke 2026-09-30 (schwarz-dev-wire, release wasm, headless Chrome, the
  local proxy now sending COOP/COEP, user 2): (1) fresh browser — first
  setup over the local VFS, external-commit join, send; (2)–(4) the same
  browser in new sessions — `accounts()` 1, keys present, the device
  reused, sync, send, and from (3) on no commit at all (the MLS state
  survives in IndexedDB; (2)'s one commit took pending proposals). User 1
  received every send; the smoke device was deleted again. The scripted
  smoke page gained a `READY` line: a login posted to the module worker
  before its handler was set was lost once.
- Retrospective: `docs/LEARNINGS.md` §5.50.

## 111b — design (2026-09-30)

- **Configuration only.** `Kupferdraht.open({ storage: { mode: "server", url,
  tokenExchangeUrl } })` — the storage server's `wss://…/v1/ws` and the
  token exchange's base URL come from the embedder; nothing is built in
  (other stages have other addresses). `pinServiceUrl` joins with Step 112.
  `global.db` and the home id stay local in the server mode too.
- **The storage-login key** is an account key like the database keys:
  keystore id `kupferdraht.<home-id>.account-<user>-storage-login`, derived from
  the seed with `kupferdraht/v1/storage-login`, an Ed25519 signing key. The VFS
  gets a `Signer` over it (algorithm, public key, sign) — never the key.
  The server keeps an account's databases under that key's subject, so the
  same seed finds the same databases in any browser.
- **The storage token** comes from the token exchange: `POST /v1/token`,
  `Authorization: Bearer <Wire access token>`, `{"audience": "storage",
  "publicKey": <base64url>}` → `{"token", "expiresIn"}`. The VFS asks its
  `TokenSource` synchronously, so Kupferdraht keeps the token cached and renews
  it in the background at half its lifetime; a `403 not_allowed` there
  stops the instance: `onEvicted("access")`.
- **Where the Wire token comes from:** the login's token while the login is
  pending (a first setup or a login with the seed), the client's own
  refreshing token once it is built; `resume` renews the session first —
  `POST /access` with the browser's cookie alone (`client_id` is optional),
  the backend's address from the account's registry entry — because the
  device id it would name sits in the remote `kupferdraht.db`.
- **Opening:** an account's databases register with `Config::server(url,
  signer)`, the token source, the VFS's browser cache and the takeover of
  the call that opens them. `resume` opens without creating: a database
  the server no longer has (retention) is `NothingStored`.
- **Errors** (new `KupferdrahtError` variants): `NothingStored` (open without
  create → `SQLITE_CANTOPEN`), `StorageAccessDenied` (the exchange's
  `403`, a registration or open refused for the token → `SQLITE_AUTH`),
  `ServiceUnavailable` (the storage server or the exchange unreachable,
  the exchange's `503`); the exchange's `429` is `RateLimited`.
- **`onEvicted("device")`** needs the VFS to tell a database another
  browser took over from an unreachable server — both are
  `SQLITE_IOERR_FSYNC`. sqlite-remote-vfs v0.1.1 (`7a1e1cf`, 2026-09-30)
  reports why a database stopped: `RemoteVfs::failure()` (`TakenOver`,
  `Unreachable`, `Denied`, `Rejected`, `Full`). Every `SqliteStore::lock()`
  checks it first: `TakenOver` on a server database stops the instance
  (`onEvicted("device")`; on a local one it is the tab's takeover),
  `Unreachable` is `ServiceUnavailable`, `Denied` `StorageAccessDenied` —
  until the database is opened again (the embedder drops the client and
  resumes; an automatic reopen is a follow-up, `open.md`).
- **Tests:** natively (httpmock) the exchange client and its error mapping,
  the storage-login key (known-answer derivation, a verifiable signature,
  forgotten with the seed), the token cache and its renewal point, the
  Wire session (a refused client token renewed once, a login's token
  not), the open-error mapping. The chain
  — configuration, registration with signer and token, login, first setup,
  resume, `NothingStored` — is proven by the smoke on the ODJ test stage
  (storage server, token exchange) with a Wire login: a browser test would
  need a server CI does not have, and public API only tests would use.

## 111b — smoke (2026-09-30)

On the test stage (storage server, token exchange; addresses from the dev
proxy's configuration), headless Chrome, a fresh profile, the smoke page
with `?scenario=login&storage=server` and then `?scenario=resume&storage=server`:

- **Login:** `Kupferdraht.open` in the server mode, login, `keys` `"missing"` →
  `registerDevice({ newSetup: true })`: the seed, the storage-login key, a
  storage token from the exchange, both databases created on the storage
  server; sync, send (received by the other user), messages.
- **Resume** on the same profile: the Wire session from the browser's
  cookie, a new storage token, both databases opened from the server
  without creating them; sync and messages.
- **Found:** the first run failed as `ServiceUnavailable` — the storage
  server's origin allow-list lacked the page's origin (deployment setting,
  fixed on the service side). A resume seconds after the login run failed
  as `HeldElsewhere`: the killed page's lease holds until it expires
  (30 s); 40 s later it worked (`open.md` §72).
- **Not smoked:** `NothingStored` (needs a database removed from the
  server), `onEvicted("device")` (needs the seed in a second browser —
  Step 112) and `"access"` (needs the exchange to stop admitting the
  user); their mappings are unit-tested or rest on the VFS's tests.

## 111c — takeover per call, the tabs layer in the server mode (2026-09-30)

Commissioned by Markus after 111b. Two deviations of 111b from ADR-0035
surfaced while the tabs layer was examined, and they compound there:

- 111b registered an account's server databases with the takeover of
  `Kupferdraht.open({ takeOver })` — the flag of the *tab* hand-off. ADR-0035
  puts the takeover on the call that opens the account: `resume(id,
  { takeOver })`, `registerDevice({ newSetup, takeOver })` (and Step 112's
  `recover`). The tabs layer always opens with `takeOver: true` (its lease
  hand-off), so every new leader would silently take another browser's
  databases, and two browsers that resume after an eviction would take
  them from each other forever.
- 111b opened the databases at login when the seed is present. ADR-0035:
  "Databases of an account open in `recover`, `registerDevice` and
  `resume`, not in `login`, so a takeover never needs a second login."

Design:

- **A login in a seed home opens nothing.** `keys` says whether the seed
  is here; `registerDevice` opens the account's storage (with its seed) —
  or, without the seed, only a first setup does. Internally "the seed is
  here" and "the storage is open" become two facts (they were one flag).
  Native homes (`Random`) keep opening at login.
- **The takeover is the call's.** `resume(accountId, { takeOver })` and
  `registerDevice({ newSetup, takeOver })`; without it a database another
  instance holds throws `HeldElsewhere`, and the embedder asks, then
  repeats the call with `takeOver: true`. `Kupferdraht.open({ takeOver })` takes
  over the tab lease and, in the local mode, the databases' locks of this
  browser — never another browser's databases. A server registration
  serves an open only for the same storage-login key *and* the same
  takeover; otherwise the database registers anew.
- **The tabs layer in the server mode.** `serveTab({ storage })` passes the
  storage option to the leader's `Kupferdraht.open`. An eviction other than by
  a tab (`"device"`, `"access"`) is announced by the leader to every tab
  on the request channel (a message kind of its own, which does not move
  any tab's view of the leader), so every host fires `evicted` with its
  `reason` — the leader's host from its worker, as before. Hosts re-queue
  as before: the next leader opens Kupferdraht, and the proxy's automatic
  re-resume meets `HeldElsewhere` / `StorageAccessDenied`, which the
  embedder shows. The proxy's `resume` passes the options on.
- **Tests (red first).** Natively: a login with the seed derives and opens
  nothing, `registerDevice` then does. In the browser (the Step-94 stub
  harness): `serveTab` passes `storage` and `takeOver: true` to
  `Kupferdraht.open`; `resume(id, { takeOver })` reaches the leader with the
  option; a `"device"` eviction reaches the leader's and a thin tab's host
  once each, and a leader serves again afterwards; a tab takeover is
  reported as `"tab"` to the evicted tab only. Pins: the binding's and the
  `.d.ts`'s option shapes, the embedder guide.
- **Smoke:** two browser profiles with the same account need the seed in
  both — possible only with Step 112's recovery data (or a copied
  keystore). The server-mode takeover smoke waits for 112 unless a
  profile copy works; the login → register → resume smoke is repeated.

## 111c — smoke (2026-09-30)

On the test stage, headless Chrome, the 111b profile (seed and account
present), the smoke page in the server mode:

- **Login with the seed:** `keys` `"present"`; the login opened nothing,
  `registerDevice()` opened the account's databases on the storage server
  and reused the recorded device (`deviceReplaced` false); sync, send.
- **Two browsers, one account** (browser B a copy of A's profile, so both
  hold the seed): A resumes and keeps working (`?scenario=hold`); B logs in
  (`?scenario=takeover`) — `registerDevice()` throws `HeldElsewhere`,
  `registerDevice({ takeOver: true })` takes the databases over and reuses
  the device; A reports `onEvicted("device")` at its next commit.
- **Found:** Chrome writes a rotated login cookie to disk late — a profile
  closed right after a login kept the previous, superseded cookie, and its
  `resume` was `SessionExpired`. The smoke now waits before closing; for an
  embedder nothing changes (a running browser holds the cookie).
- **Not smoked:** the tabs layer on a real storage server (its
  behaviour is covered by the stub scenarios of `step_94_leader_tab`).

## 111d — a stable instance id: a reload takes its own lease back (2026-09-30)

Commissioned by Markus after 111c. The storage server grants a database's
lease without a takeover to the instance id that holds it (the other
instance is fenced) — live on the test stage since 2026-09-30
(sqlite-remote-server `75190e6`; no crate change, the pin stays v0.1.1).
With a random id per registration, a reload, a crash or a leader change
between tabs meets its own predecessor's lease for up to 30 s
(`HeldElsewhere`, found in the 111b smoke, `open.md` §72).

- **The id:** 16 bytes, stable per browser home and account —
  `SHA-256(label ‖ home id ‖ account id)` (each part length-prefixed, label
  `kupferdraht/v1/storage-instance`), truncated. Not secret (it opens nothing
  without the storage-login key); derived, not stored — the home id is
  already a keystore entry. Both databases of an account use it.
- **Why it is safe:** two instances with the same id must never run at the
  same time. One browser home runs one instance (the origin's Web Lock
  lease, ADR-0023; the tabs layer serves every tab through one leader); a
  tab takeover fences the old leader's connection, which is being
  terminated anyway. Another browser has another home id and still needs
  the explicit takeover (111c). A copied browser profile shares the id —
  it also shares the Wire device and the MLS state, so it is unsupported
  anyway.
- **Tests (red first):** natively the derivation (a known-answer value
  computed independently, stable, different per home and per account); a
  pin that the server registration sets it. The chain — a reload that
  resumes at once — is the smoke.
- **Smoke:** a profile resumes; the page is closed and reopened at once;
  the second resume succeeds without waiting and without a takeover.

## 111d — smoke (2026-09-30)

On the test stage, headless Chrome, a profile with the account (server
mode), `?scenario=resume&storage=server&reload=1`: the page resumes, reloads
itself at once — the first page's lease still active — and resumes again:
`RESUME-OK` twice, no `HeldElsewhere`, no takeover. In the 111b smoke the
same situation was `HeldElsewhere` until the lease expired (30 s).

Consequence for the 111c takeover smoke: its browser B was a copy of A's
profile, so it shares the home id and now the instance id — it would take
the lease back silently instead of meeting `HeldElsewhere`. A takeover
smoke needs two browsers with their own home ids, which Step 112's
recovery data makes possible.
