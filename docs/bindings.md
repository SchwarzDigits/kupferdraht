# Language bindings (Kotlin / Swift / Android / Web)

Kupferdraht ships two binding tracks from the same `kupferdraht-core` crate:

- **UniFFI 0.29** generates the Kotlin / Swift surface from the `ffi/`
  wrapper layer (native cdylib).
- **wasm-bindgen** exposes a hand-written browser/TypeScript surface from
  `kupferdraht-core/src/wasm/` — parallel to `ffi/`, not layered on it
  (Steps 44–59; see "The WASM / web surface" below).

This document covers both surfaces and how to build the packaging
artifacts. The generated Kotlin / Swift / TypeScript code is the
authoritative reference for exact signatures — this is the map, not the
territory.

## Architecture: the `ffi` wrapper layer (approach B)

The FFI surface lives entirely in `kupferdraht-core/src/ffi/`, separate from the
native matrix-rust-sdk-shaped API. Stateful core types (`Client`, `Room`,
`Timeline`, the manager, …) are wrapped by FFI objects that return owned,
FFI-shaped values and delegate to the inner native type; the core stays
idiomatic Rust. Value types split into two cases (see the
*Public API ↔ FFI parity* rule in `CLAUDE.md`):

- **FFI-friendly** plain-data types and owned-arg callback traits carry
  their `uniffi` derives / exports directly on the core type.
- **Shape-divergent** types (a field UniFFI can't marshal, generics,
  streams) are mirrored in `ffi/` (a `Record` / `Enum` with `From<&core>`,
  or a foreign-callback twin trait + Rust adapter).

A Rust consumer (e.g. `kupferdraht-cli`) uses the native API directly; foreign
consumers use the `ffi` surface below.

## The surface

**Entry points (free functions / constructors)**
- `Kupferdraht.open(base_dir, keystore) -> Kupferdraht` — the multi-account manager.
- `parse_user_id(id) -> QualifiedUserId`, `kupferdraht_version() -> String`.
- `initLogging(level, listener)` (Step 117) — Kupferdraht's log events at
  `level` (`LogLevel.ERROR` … `TRACE`) or above go to `listener`, a
  `LogListener` with one method `onLog(record)` (`LogRecord { level,
  target, message }`). Kupferdraht writes nowhere itself: without a listener it
  stays silent; the app decides — logcat, `os_log`, its own pipeline. A
  later call replaces level and listener (a debug switch); only Kupferdraht's
  own events are forwarded, not its dependencies'. `onLog` runs on the
  thread that logged — return fast. Throws `InvalidInput` when the app
  installed a `tracing` subscriber of its own (Rust embedders).

  ```kotlin
  initLogging(LogLevel.WARN, object : LogListener {
      override fun onLog(record: LogRecord) {
          Log.println(if (record.level == LogLevel.ERROR) Log.ERROR else Log.WARN, "Kupferdraht", "${record.target}: ${record.message}")
      }
  })
  ```

**Objects** (opaque, passed by reference; ephemeral — compare by `id`, not
object identity)
- `Kupferdraht` — `home_id()` / `cookie_label()` (Step 93: the home's id from
  `base_dir/home.id` and its Wire login label `kupferdraht-<home-id>` — one
  per home, so two homes of one user on one keystore keep both
  sessions), `accounts()`, `active_account()`, `login(server, email,
  password, second_factor_code?) -> PendingLogin`, `resume(account_id) ->
  Client`, `register(server, name, email, password) -> PendingClient`,
  `initiate_sso(server, code, success_redirect, error_redirect) -> String`
  (Step 17β — validates the SSO code and returns the interactive initiate URL
  to open in a browser; wasm: `initiateSso`),
  `finalize_sso(server, sso_cookie, device_label?) -> PendingLogin` (Step 17β —
  completes login from the `zuid` cookie captured off the redirect; drive
  `register_device` as with password login; wasm: `finalizeSso`),
  `initiate_sso_by_email(server, email, success_redirect, error_redirect) -> String`
  (Step 17β — resolves the backend's default SSO code via `GET /sso/settings`;
  wasm: `initiateSsoByEmail`).
- `PendingClient` — `user_id()`, `email()`, `activate(code) -> Client`
  (once-only on success; a failed activation can be retried),
  `resend_activation_email()`. Activation registers the account in the
  device registry and persists its session — it is immediately listable
  via `Kupferdraht.accounts` and resumable via `Kupferdraht.resume`.
- `PendingLogin` — `user_id()`, `email()`, `register_device() -> Client`
  (phase 2; single-use on success, retryable on `TooManyDevices`),
  `devices() -> [DeviceInfo]`, `delete_device(device_id)`. Phase-1 login
  handle: `register_device` completes login; on the 7-device cap, free a
  slot (`devices` / `delete_device`) and call `register_device` again on
  the same handle — no second login.
- `Client` — identity getters (`user_id() -> QualifiedUserId?`,
  `device_id`, `account() -> Account`, backend URLs),
  `storage_engine() -> StorageEngineInfo?` (Step 90 — the whole-DB
  encryption engine, cipher scheme and header mode read off the open
  store; `null`/`nil` when the backend reports none);
  `own_device_thumbprint()` and `subscribe_device_changes(listener:
  RoomDevicesListener) -> Subscription` (Step 95d — this device's MLS
  thumbprint; one callback per room whose device set changed);
  rooms (`rooms`, `room`, `create_dm`, `dm_with`, `self_room`, `join_room`,
  `leave_room`, `delete_room`, `rename_room`, `join_via_link`,
  `find_message_room`, `find_message_by_id`, `resync_room`,
  `resync_existing_rooms`, …); admin (`logout`,
  `set_display_name`, `set_handle`, `update_email`, `change_password`, `devices`,
  `feature_configs`, `file_sharing`, `receipt_mode`, …); sync +
  observers (below).
- `Room` — getters (`id`, `name`, `room_type`, `counterpart`, `members`,
  …), membership (`join`, `leave`, `delete` — team-admin, `invite`, `kick`,
  `set_name`), access /
  guest links (`access`, `set_access`, `create_join_link`, …),
  `timeline() -> Timeline`, `messages(limit?, before?)` (`limit`
  `null`/`nil` = everything), `failed_sends()` (the room's `Failed`
  rows, oldest first — Step 79), `devices() -> [RoomDevice]` (Step 95d —
  one per MLS leaf with identity and MLS thumbprint).
- `Timeline` — `room_id()`, `items()`, send (`send_text`,
  `send_text_auto_join`, `send_reply`, `edit`, `redact`,
  `toggle_reaction`, `mark_as_read`, `send_file`, `send_file_stream`,
  `send_text_with_mentions`, `send_knock`, `send_location`,
  `send_ephemeral_text(body, ttl: Duration)` — Step 89; `edit` / `redact`
  refuse another user's message with `InvalidInput`, Step 95b),
  failed-send recovery (`resend(message_id)` — one failed row under the
  same id; `resend_failed() -> ResendSummary` — the room oldest-first,
  stops at the first hard error; Step 79),
  typing (`send_typing_started`,
  `send_typing_stopped` — auto-stop + dedupe), `asset(message_id) -> Asset?`,
  `subscribe(listener) -> Subscription`.
- `Asset` — `metadata() -> AssetMeta`, `download() -> bytes`,
  `save_to(path)`.

**Records / enums** — `QualifiedUserId`, `Account`, `RoomMember`,
`RoomDevice`, `DeviceCredential`, `RoomDevicesChange` (Step 95d; the
foreign-implemented `RoomDevicesListener` sits with the other observers),
`RoomType`, `RoomAccess`, `JoinLink`, `TimelineItem`, `TimelineItemKind`,
`TimelineDiff`, `DeliveryStatus`, `Message`, `MessageBody` (`TimelineItem`
and `Message` carry `expires_at_ms: Option<i64>` since Step 89 — set
only for an ephemeral message), `AssetMeta`,
`MessageHandle`, `Mention`, `ReactionGroup`, `DeviceInfo`, `FeatureConfigEntry`,
`UserPropertyEntry`, `FileSharingPolicy`, `ReceiptMode`, `AccountEntry`,
`ServerConfig`, `SyncSettings`, `SyncTransport` (Step 62 — selects the
receive transport; `WebSocket` also switches `start_sync` to the push
runner. UniFFI records carry no enum defaults, so Kotlin and Swift name
it: pass `WebSocket`, the Rust default on native targets since Step 118
(ADR-0033) — it drains without a socket too; `Polling` is the explicit
opt-out), `SyncSummary`, `TypingEvent`, `JoinSummary`,
`ResyncSummary`, `FailedJoin` (element type of `JoinSummary.failed` /
`ResyncSummary.failed`), `ResendSummary` / `FailedResend` (Step 79 —
`resent`, `skipped`, `stopped_at?`, `remaining`), `StorageEngineInfo`
(Step 90 — `engine`, `version`, `cipher`, `plaintext_header_size`). Errors surface as a thrown `KupferdrahtError` (Kotlin
sealed class / Swift enum) — foreign code can branch on the variant.
`KupferdrahtError.AccountLocked` (Step 85, native) means another Kupferdraht
process is inside the account's critical section for a few
milliseconds — retry the call; nothing was changed on this side.
Since Step 97d a sync call can return it too, when another `Client` or
process on the account held the drain lock for more than 5 s.
`KupferdrahtError.KeyUnavailable` (Step 96, ADR-0029) means a database
exists but the keystore returned no key for it; nothing was written.
If the keystore may still be locked (iOS before the first unlock),
retry once it is available; if the key is gone for good, the database
cannot be read — the payload names the way out (for `global.db` the
home's registry file, for the account databases the account directory).

**Callback interfaces** (the embedder implements these)
- `SecureKeyStore` — the platform key store (iOS Keychain / Android
  Keystore). Called from Rust when a database is keyed. The secret is an
  opaque blob the embedder stores verbatim: **32 bytes** since Step 90
  (the raw ChaCha20 key, ADR-0021; previously 48). A 48-byte blob stored
  by an earlier build is rejected like any other length (`KupferdrahtError`,
  never a crash) — re-provision; there is no transition path (I3). The
  `id` strings are **home-scoped** since Step 93 (ADR-0025):
  `kupferdraht.<home-id>.global-db`, `kupferdraht.<home-id>.account-<user>`,
  `kupferdraht.<home-id>.account-<user>-mls`, where `<home-id>` is 32 hex
  characters read from `base_dir/home.id` — ASCII, at most 89 bytes plus
  the user's domain; store them verbatim, never parse them. An embedder
  that keyed a home under the pre-Step-93 ids (`global-db`,
  `account-<user>`, `account-<user>-mls`) needs no migration: on the
  first open Rust reads the legacy id and writes the same secret under
  the home-scoped id (only for a database that already exists) — the
  legacy entry is left in place and may be deleted by the embedder once
  every home has been opened by a Step-93 build; `kupferdraht.install-id`
  (the retired Step-41 label source) likewise. **Contract (ADR-0029):**
  `get_secret` returns `None` only for an id that was never stored or
  was deleted; a keystore that is locked or not available yet must
  **throw** — the open then fails and a later attempt succeeds. Kupferdraht
  never replaces the key of an existing database; answering `None`
  while locked surfaces as `KeyUnavailable`. Platform advice: iOS items
  with `kSecAttrAccessibleAfterFirstUnlock(ThisDeviceOnly)` (readable
  in the background after the first unlock); Android Keystore keys
  without user-authentication binding. On the web this
  is **not** an embedder callback — the wasm track ships the built-in
  `IndexedDbKeyStore` (IndexedDB + WebCrypto, Worker-capable since
  Step 58), constructed internally by `Kupferdraht.open`.
- `EventHandler` — inbound messages / invites / sync errors / typing
  (`on_typing(TypingEvent)` — the current set of users typing in a room).
- `TimelineListener` — per-room timeline diffs.
- `SyncListener` — per-iteration `SyncSummary` from `start_sync`;
  `on_error(description) -> Bool` decides whether a failed iteration
  retries (`true`) or stops the loop (`false`);
  `on_session_expired()` fires at most once when the loop hits the
  terminal `KupferdrahtError::SessionExpired` — the loop has already stopped;
  surface a re-login gate (Step 39).
- `AssetSource` — foreign-implemented streaming byte source for
  `Timeline::send_file_stream`, the streaming counterpart of `send_file`
  for platforms with no filesystem path (e.g. an Android `content://`
  stream). `read_chunk(max_len) -> bytes`; an empty `Vec` signals EOF.

Sync-driven callbacks (`EventHandler`, `TimelineListener`,
`SyncListener`) fire **synchronously on the sync task** — a blocking
implementation stalls the sync loop. Return fast; dispatch heavy work
(DB writes, UI updates) to your own executor / queue.

**Sync model.** `Client.sync_once(settings)` runs one iteration.
`Client.start_sync(settings, listener) -> TaskHandle` runs a background
loop (the FFI form of the non-marshalable `sync` / `sync_with_callback` /
`sync_stream`). A failed iteration calls `listener.on_error`; returning
`true` retries after a backoff (`poll_interval`, or 2 s when unset;
after `KupferdrahtError.RateLimited` the backend's `Retry-After`, bounded to
1–60 s), `false` stops the loop. Between successful iterations the loop
pauses `poll_interval` — 10 s when unset (Steps 103, 118: one value
without a socket on every target), none for a zero duration. Retry policy beyond that — give-up counters,
exponential growth — belongs to the listener, which sees every failure.
Cancellation is **cooperative** — `sync_once` always finishes, then the
loop stops at the iteration boundary (also mid-backoff). A panic inside
the loop (a defect — foreign data no longer reaches one, Step 95c) is
reported as `on_error("sync stopped after an internal panic: …")`, and
the loop stops regardless of the return value.

**Reconnect contract (Step 79).** The loop returning on a hard error
*is* the contract: there is no network observer and no background
scheduler in core. Reconnect / backoff is the embedder's loop
(`on_error` → `true` retries; give-up counters and exponential growth
live in the listener), and a send that failed while offline stays a
`Failed` row until the embedder re-drives it — on the first successful
iteration after a restart, call `Room.failed_sends()` and
`Timeline.resend_failed()` per room (or `Timeline.resend(message_id)`
for one row). `docs/library.md` "Reconnect contract" has the full
statement and the Rust shape of the loop.

**Handles.** `TaskHandle.cancel()` and `Subscription.unsubscribe()` stop a
sync loop / listener. Dropping the handle also stops it, but on the JVM
finalization is GC-timed — **call them explicitly** for prompt shutdown.

## The WASM / web surface

Hand-written `#[wasm_bindgen]` bindings in `kupferdraht-core/src/wasm/`
(`mod.rs`, `manager.rs`, `live_sync.rs`) — the UniFFI `ffi/` wrapper
module is cfg'd off on wasm32, so this layer is **parallel to `ffi/`,
not layered on it**. All method names are camelCase on the JS side; payload objects use
camelCase fields (serde-projected view structs).

**Persistence (Step 91, ADR-0022; on sqlite-remote-vfs since Step 111,
ADR-0035):** the same whole-DB-encrypted SQLite3MC store as native —
`kupferdraht.db`, `mls.db` (openmls state, persisted continuously) and
`global.db` — opened through `sqlite-remote-vfs`, one registration per
database, SQLite3MC encrypting above it. Step 111a uses the VFS's **local
mode**: every database is an IndexedDB database
`sqlite-remote-vfs-local/<home-id>/kupferdraht/…` (`kupferdraht/global.db`,
`kupferdraht/accounts/<id>/kupferdraht.db`, `…/mls.db`), one Web Lock per database;
the **server mode** (Step 111b, below) keeps an account's databases on a
storage server. The keystore is IndexedDB + WebCrypto (`IndexedDbKeyStore`); it also holds the
home id (`kupferdraht.home-id`). Browser data of builds before Step 111 (OPFS)
is not migrated and left where it is. Three consequences for the embedder:

- **Serve the app cross-origin isolated.** The storage blocks with
  `Atomics.wait` on a `SharedArrayBuffer`, which browsers grant only to a
  cross-origin isolated page — send
  `Cross-Origin-Opener-Policy: same-origin` and
  `Cross-Origin-Embedder-Policy: require-corp` (and allow the VFS's
  connection worker, `worker-src blob:`, in a Content Security Policy).
  Without it `Kupferdraht.open` fails closed with an error naming the headers.
- **Run Kupferdraht in a dedicated Web Worker.** The storage blocks, which the
  main thread may not; `Kupferdraht.open` fails closed there with an error naming
  the requirement (no in-memory fallback — MLS state must never be silently
  volatile).
- **One active instance per origin (Step 92, ADR-0023).** Before any
  database is touched, `Kupferdraht.open` takes the origin-scoped Web Locks lease
  `kupferdraht:instance`. Held by another tab → the open fails closed with
  "already open in another tab" (recoverable — render "open elsewhere, use
  here?"). `Kupferdraht.open({ takeOver: true }, onEvicted)` steals the lease and
  each database's lock: the other tab's instance is **evicted** — its
  `onEvicted("tab")` fires once, its `evicted` getter turns `true`, every
  store operation there fails as `Evicted`; its embedder should terminate
  that worker. Web Locks: Chrome 69 / Firefox 96 / Safari 15.4.
- **Many tabs, one instance (Step 94, ADR-0026).** The lease holder's
  worker is the **leader**; every other tab of the origin is a **thin
  client** that reads and sends through it. Kupferdraht ships the layer as
  `bindings/web/kupferdraht-tabs/` (below) — opt-in; `Kupferdraht.open` directly
  stays the single-tab path.

**The server mode (Step 111b, ADR-0035).**
`Kupferdraht.open({ storage: { mode: "server", url, tokenExchangeUrl,
pinServiceUrl } })` keeps each account's `kupferdraht.db` and `mls.db` on a
storage server (`sqlite-remote-server`); `global.db`, the keystore and the
home id stay in the browser. The addresses come from the app's
configuration — Kupferdraht has none built in: `url` is the storage server's
WebSocket endpoint (`wss://…/v1/ws`), `tokenExchangeUrl` the token
exchange's base URL (`https://…`), `pinServiceUrl` (optional, Step 112) the
PIN service's base URL — recovery data needs it; plain `ws://` / `http://`
only with `permitHttp`.

- **Whose databases.** The server keeps an account's databases under its
  storage-login key — an account key like the database keys, derived from
  the account's seed (`kupferdraht.<home-id>.account-<user>-storage-login`) — so
  the same seed finds the same databases in any browser. The VFS only ever
  gets a signer over the key.
- **Admission.** Before an account's databases open, Kupferdraht buys a storage
  token from the token exchange with the Wire session (`POST /v1/token`,
  audience `storage`, the key's public half) and renews it in the
  background at half its lifetime — with the login's token while a login
  is pending, with the client's session afterwards. `resume` first renews
  the Wire session with the browser's login cookie (the device id sits in
  the remote `kupferdraht.db`), then opens the databases **without creating**
  them.
- **The server serves too.** Every open needs the server; the VFS keeps a
  cache of each database in IndexedDB (only acknowledged pages), so reads
  it can serve cost no round trip. Every commit is a round trip.
- **Taken over elsewhere — only when asked (Step 111c).** Opening an
  account another browser has open fails as `HeldElsewhere`; the embedder
  asks the user, then repeats the call with the takeover:
  `resume(accountId, { takeOver })` or `registerDevice({ newSetup,
  takeOver })` with `takeOver: true`. `Kupferdraht.open({ takeOver })` takes the
  tab lease (and, in the local mode, the databases' locks of this browser)
  — never another browser's databases. A reload, a crash or a leader
  change between tabs takes its own lease back at once (Step 111d): the
  storage server grants it to the instance that held it, and the instance
  id is stable per browser and account — no wait, no takeover. The instance
  that lost them learns at once (Step 119, sqlite-remote-vfs 0.3.0 reports
  the takeover as an event; across storage-server instances at its next
  request): `onEvicted("device")` fires, every store operation fails as
  `Evicted` — an operation that met the takeover too, not as a storage
  error — and its sync loop and push socket stop.

Refused opens have their own names: `HeldElsewhere` (another instance holds
the database), `StorageFull` (the storage quota is exhausted; the commit was
rolled back), `Evicted` (this instance stopped). The server mode adds:

- `NothingStored` — `resume` found no database of the account on the
  storage server (its retention may have removed an unused one): log in
  again; the login creates the databases anew and registers a new device.
- `StorageAccessDenied` — the token exchange does not admit this user, or
  the storage server refused the storage token. A renewal the exchange
  refuses stops the instance: `onEvicted("access")`.
- `ServiceUnavailable` — the storage server or the token exchange cannot
  be reached, or the exchange is overloaded; retry later. The
  exchange's rate limit is `RateLimited`.

**The storage heals itself (Step 113, ADR-0038).** When the storage server
is unreachable, the VFS reconnects in the background and heals an
account's databases in place once the server answers again — no resume, no
reload, nothing for the embedder to do but show the state:

- `client.storageState` — `"online"`, or `"reconnecting"` while the
  server is unreachable; the sixth optional `startSync` callback
  `onStorage(state)` reports each change (the tabs leader broadcasts it as
  `storage`).
- While `"reconnecting"`, every operation of the account — a send too —
  fails at once with `ServiceUnavailable`; nothing is queued, so keep what
  the user typed and offer to send it again. A send that failed was not
  sent: a commit the outage interrupted is rolled back.
- The operation that first meets an outage waits up to 3 s for the
  connection before it fails; after that nothing blocks.

The IndexedDB record store of ADR-0015 Part 2 (per-record AES-GCM,
plaintext index keys) and the MLS snapshot are gone.

**An account's keys come from its seed (Step 110, ADR-0035).** The keys are
stored and loaded exactly as natively; only a *new* key of an account
differs: it is derived from the account's seed (32 random bytes, a keystore
entry of its own) instead of drawn at random. `global.db` and the home id
belong to the browser and stay native. A login in the browser **opens and
stores nothing** of the account (Step 111c, ADR-0035): `registerDevice`
opens the account's databases — with the seed here, or after a first
setup creates it — and without the seed the embedder decides: a first
setup now, or (Step 112) a restore from recovery data. A takeover
therefore never needs a second login. Accounts set up by builds before Step 110 have no seed: they are no
longer listed, and a first setup removes their keys and databases (no
migration — test-only deployment).

**Errors.** Every thrown error is a JS `Error` whose `name` is its
`KupferdrahtError` variant — `SessionExpired`, `TooManyDevices`,
`RecoveryDataRequired`, `InvalidInput`, … — so JavaScript branches on
`error.name` as Kotlin and Swift do on the typed error; `message` is the
text as before. The binding's own failures use the same names (a malformed
options object or an unknown room is `InvalidInput`). The recovery errors
(Step 112) carry their detail as properties, each only when known:
`InvalidRecoveryData` its `reason` (`"notRecoveryData"`, `"unsupported"`,
`"otherAccount"`, since Step 120 `"oldDevice"`), `WrongPin` its
`attemptsRemaining` (the PIN service's
count; absent for a PIN checked locally), `TooManyAttempts` its
`retryAfter` (seconds). `RateLimited` carries `retryAfter` too since
Step 122: the seconds the backend's `Retry-After` asks for.

**The login flow.** The login screen knows nothing of recovery data — a
login may be single sign-on; setting up the browser is a step after it,
and only when needed:

1. App start: `accounts()` lists the accounts whose keys this browser holds
   → `resume(accountId)`. `SessionExpired` → log in again, no recovery data
   needed. `RecoveryDataRequired` → the keys are gone; set up again. In the
   server mode also `NothingStored` → log in again, and `DeviceRetired`
   (Step 120, ADR-0040) → another browser set the account up anew and
   replaced this one; Kupferdraht forgot its keys here — log in again (recovery
   with the new device's data, or a new setup). An empty list means
   "never set up here, or the browser evicted the data".
2. Login (password or SSO) → `PendingLogin`. With `keys` `"present"`,
   `registerDevice()` and into the app — the native path: the recorded
   device is reused, or replaced when the backend no longer has it
   (`client.deviceReplaced`, older messages gone). `HeldElsewhere` (the
   server mode: another browser has the account open) → ask → the same
   call with `{ takeOver: true }`; the same for `resume`.
3. With `keys` `"missing"`: one "set up this browser" screen. With
   `recoveryExpected` (the account's storage slot names a set-up device, so
   recovery data exists somewhere — Step 120) the main action is restoring
   it (Step 112) and a new setup is the secondary, warned-about choice: it
   **replaces** that device — its databases on the storage server are
   deleted, its Wire client too, older messages do not come back; without
   it the main action is the first setup: `registerDevice({ newSetup: true
   })`. Back:
   `cancel()`. `registerDevice()` without `newSetup` throws
   `RecoveryDataRequired` here — nothing is set up by accident.
4. `TooManyDevices` on a first setup: `devices()` → `deleteDevice(id)` →
   the same `registerDevice({ newSetup: true })` again; the seed is kept.
   `DeviceRetired` (Step 121): this browser holds the keys of a device
   another setup replaced, so the account's databases will not open here.
   Kupferdraht forgets those keys as it fails — as on `resume` — so a login
   straight after lands at step 3 with `keys` `"missing"` and the keyfile is
   the way back in. Report it rather than the raw error: before Step 121 the
   keys stayed and every further login met the same refusal, with no call an
   embedder could make to clear the browser.
5. Restoring (Step 112): the user picks the recovery data file and enters
   the PIN → `recover(data, pin, { takeOver })` → the client (with
   `deviceReplaced` when the recorded device was gone). `WrongPin` → ask
   again, showing `attemptsRemaining`; `TooManyAttempts` → try again after
   `retryAfter` seconds; `InvalidRecoveryData` → another file (`reason`
   `"otherAccount"`: the data of another account; `"oldDevice"`: the data
   of a device another setup replaced — no PIN attempt spent). `HeldElsewhere` → ask →
   `registerDevice({ takeOver: true })` — the keys were recovered, no second
   PIN attempt. `TooManyDevices` → free a slot → `registerDevice()`.
6. After a first setup, "secure your access": a PIN (`validateNewPin`,
   entered twice) → `client.recovery().enable(pin)` → bytes the app offers
   as a download.

**Recovery data (Step 112, ADR-0035).** The account's seed sealed with a
PIN — opaque, versioned bytes (about 130) the embedder keeps and never
parses: a download, a file, a password manager. The seed never leaves
Kupferdraht. Only the server mode with a `pinServiceUrl` has recovery data; in
the local mode `recovery().state()` is disabled and the other recovery
calls throw `InvalidInput`.

- **The PIN.** `validateNewPin(pin)` → `"ok"`, `"tooShort"` or
  `"tooCommon"`: at least 6 characters, counted as characters after Unicode
  NFC, not as bytes; digits only is fine, and longer PINs with letters,
  digits and other characters are too — there is no upper limit a user
  would reach. `"tooCommon"` refuses a short list of trivial PINs such as
  `123456` or `000000`. So offer a text field that takes any characters,
  not a keypad for exactly six digits; digits first, with a hint that more
  is allowed.
- **The PIN service** limits the guesses: every seal (`enable`,
  `changePin`) and every `recover` asks it once (5 attempts an hour, 12 a
  day by default); it never sees the PIN, only a blinded value. A stolen
  file alone is therefore not enough — each guess costs an attempt at the
  service, and the file also needs Argon2id work per guess. `verifyPin`
  and the current PIN of `changePin` are checked locally and cost none.
  Since Step 124 a seal and a `recover` with the right PIN give their
  attempt back: Kupferdraht signs a receipt with a key only the seed yields
  (ECDSA P-256), so only wrong PINs stay counted. A receipt that fails (the
  network, or more than 15 minutes later) leaves that attempt counted and
  fails nothing.
- **Whose data.** The data names its account only as a hash, and since
  Step 120 (ADR-0040) the device — its Wire client id — and when it was
  made; since Step 124 (format version 3, 186 bytes) also the seed's
  receipt key, which the PIN service keys its evaluation by. Another account's data is refused as
  `InvalidRecoveryData` (`reason` `"otherAccount"`), the data of a device
  the account's storage slot no longer names as `"oldDevice"` — both before
  the PIN service is asked. Data of versions 1 and 2 is `"unsupported"`
  (the PIN service cannot evaluate for it any more): after the update, make
  new data (`changePin(pin, pin)`) and drop the old file.
- **The state** lives in the account's `kupferdraht.db` — on the storage server
  with the databases, so a browser that restored the account knows its
  recovery data exists.
- **Earlier data** still opens with its own PIN after `changePin`: tell the
  user to replace the copies they kept.
- Rules for the embedder, which Kupferdraht does not enforce: offer no recovery
  data in the local mode; warn before `logout({ purgeLocal: true })` while
  `recovery().state()` is disabled — without recovery data the account's
  history cannot come back.

**Leaving a browser (Step 114, ADR-0035).**
`logout({ purgeDevice, password, purgeLocal })`, all optional:

- **No options** — the native logout: the session ends, the account's keys
  and data stay in this browser, so the next login needs no recovery data.
- **`purgeDevice`** (with the password, as natively) — deletes this device
  at the backend and, in the server mode, the account's databases on the
  storage server (with the VFS's cache). Another browser that holds the
  account's keys shares that device (a restore brings the recorded device
  back), so its session ended with it: its `resume` throws
  `SessionExpired`; log in again — the login registers a new device with
  empty databases. (`NothingStored` stays the answer for databases the
  server's retention removed.)
- **`purgeLocal`** — forgets the account in this browser: its keys, its
  databases in the local mode, its registry entry (`accounts()` no longer
  lists it). The device and its databases on the storage server stay, so
  recovery data still restores it here or elsewhere. In the server mode
  the VFS's page cache of the databases stays in IndexedDB — unreadable
  without the keys.
- **Both together leave a browser for good.**

The backend comes first: a wrong password (`BackendRejected`) or a missing
one (`InvalidInput`) changes nothing here, and the call can be repeated.
With a purge **the client is closed** afterwards — its store refuses with
`SessionExpired`, `resume` no longer hands it out — so drop it. A purge
needs a client: an account whose session expired is logged in again first
(`keys` is `"present"`), then logged out with the purge.

**Free functions** — `kupferdraht_version()`, `kupferdraht_now_ms()` (wall-clock
shim, W8).

**Logging (Step 117).** Kupferdraht writes nowhere itself — no console output
of its own; the product decides where its logs go. `Kupferdraht.open({
logLevel, onLog })`: with `onLog`, Kupferdraht hands its log events at
`logLevel` (`"error"` | `"warn"` | `"info"` | `"debug"` | `"trace"`,
default `"warn"`) or above to it as `{ level, target, message }`; without
`onLog` it stays silent and `logLevel` changes nothing. `onLog` runs in
Kupferdraht's worker; what it throws is dropped. Only Kupferdraht's own events are
forwarded (targets `kupferdraht::…`), not those of its dependencies. To see them
in the browser's console:

```js
const kupferdraht = await Kupferdraht.open({
  storage,
  logLevel: 'warn',
  onLog: ({ level, target, message }) =>
    (level === 'error' ? console.error : level === 'warn' ? console.warn : console.log)(`[kupferdraht] ${target}: ${message}`),
}, onEvicted);
```

**`class Kupferdraht`** — device-level entry point, browser flavor of the
manager:
- `static open(options?, onEvicted?) -> Kupferdraht` (async) — opens the
  IndexedDB keystore, checks the context (dedicated worker, cross-origin
  isolated — see above), takes the origin's instance lease, resolves the
  home id and opens the storage, installs the panic hook. The registry key is stored durably before `global.db` is
  created with it, and so are the account keys on `login` / `resume`
  (Step 104) — a key IndexedDB refuses (quota, private mode) fails the
  call before any database exists, instead of leaving a database whose
  key is gone after a reload. A panic cannot be caught on wasm: it traps the module, and the
  worker has to be restarted (Step 95c removed every panic foreign data
  could trigger; the hook logs the one a defect still causes). `options` is
  `{ permitHttp?: boolean, takeOver?: boolean, storage?: { mode: "local" }
  | { mode: "server", url: string, tokenExchangeUrl: string,
  pinServiceUrl?: string } }`
  (`undefined` = defaults; `storage` defaults to the local mode; the server
  mode is above): `permitHttp` is for dev proxies only, `takeOver` evicts
  another tab's instance — never another browser's databases (Step 111c).
  `onEvicted(reason)` fires once when this instance stops — `"tab"`: a
  later tab took over; `"device"`: another browser took the account's
  databases on the storage server over; `"access"`: the token exchange no
  longer admits the user; terminate the worker then. Getters `evicted`, `homeId` and `cookieLabel` (Step 93; since
  Step 111a the home id is the keystore entry `kupferdraht.home-id` — one origin,
  one home — and the login label is `kupferdraht-<home-id>`).
- `login(apiBaseUrl, websocketUrl, title, email, password,
  secondFactorCode?, deviceLabel?) -> PendingLogin` — phase-1 password
  login.
- `initiateSso(apiBaseUrl, code, successRedirect, errorRedirect) ->
  String`, `initiateSsoByEmail(...)`, `finalizeSso(apiBaseUrl,
  websocketUrl, title, ssoCookie, deviceLabel?) -> PendingLogin`
  (Step 17β — same semantics as the UniFFI methods above).
- `resume(accountId) -> Client` — resume a registered account from the
  browser's storage, no network login. `SessionExpired` → log in again;
  `RecoveryDataRequired` → this browser no longer holds the account's
  keys. In the server mode the browser's login cookie renews the session
  first; `NothingStored` → log in again; `StorageAccessDenied` /
  `ServiceUnavailable` from the storage. `resume(accountId, { takeOver })`
  (Step 111c): `HeldElsewhere` when another browser has the account's
  databases open; `takeOver: true` takes them over (the other browser's
  instance stops with `onEvicted("device")`).
- `accounts() -> {accountId, server, handle, displayName,
  lastActiveMs}[]` — the device registry; since Step 110 only the accounts
  whose seed this browser holds.
- `validateNewPin(pin) -> "ok" | "tooShort" | "tooCommon"` (Step 112) —
  whether `pin` may become a recovery PIN (the rules above).

**`class PendingLogin`** — `userId`, `email` (getters),
`registerDevice(options?) -> Client` (phase 2, persists session + registry row),
`devices() -> DeviceInfo[]` / `deleteDevice(deviceId)` (Step 98a — the
device-limit recovery path: on `TooManyDevices` this handle stays valid, so
free a slot and call `registerDevice` again on it, with no second login).
Since Step 110 (ADR-0035):
- `keys` (getter) — `"present"` (the account's seed is in this browser) or
  `"missing"`.
- `recoveryExpected` (getter) — with `keys` `"missing"`: the account's
  storage slot names a set-up device (Step 120, ADR-0040: the login asks
  the storage server `GET /v1/slot`), so recovery data exists somewhere and
  would bring that device back. `false` with no device, a device still
  being set up, or no PIN service. The storage URL must be the storage
  WebSocket's (`…/v1/ws`); the slot query is beside it (`…/v1/slot`).
- `registerDevice({ newSetup?: boolean, takeOver?: boolean })` — opens the
  account's databases (the login opened none, Step 111c). With `keys`
  `"missing"` only `newSetup: true` proceeds: it removes keys and
  databases the account has here without a seed, creates the seed (stored
  durably first), derives the keys and registers. Without it:
  `RecoveryDataRequired`. `takeOver: true` takes databases another browser
  holds over (`HeldElsewhere` without it) — so the full form is
  `registerDevice({ newSetup, takeOver })`.
- `recover(data, pin, { takeOver })` (Step 112) — with `keys`
  `"missing"`: open the recovery data `data` (a `Uint8Array`) with `pin`
  for this account at the PIN service, keep the account's seed in this
  browser (stored durably first), then register as `registerDevice` does
  — the recorded device, or a new one when the backend removed it. A
  failure before the seed is recovered (`WrongPin`, `TooManyAttempts`,
  `InvalidRecoveryData`, `ServiceUnavailable`) leaves nothing; the login
  stays usable and a first setup works right after it. After it the seed
  stays (`keys` becomes `"present"`): `HeldElsewhere` → `registerDevice({
  takeOver: true })`, no second PIN attempt. With `keys` `"present"` or
  without a `pinServiceUrl`: `InvalidInput`.
- `cancel()` — abandon the login; what a first setup of it created (seed,
  keys, databases) is removed again. A seed it recovered is forgotten in
  this browser; the databases are the account's and stay. Synchronous.

**`class Client`**:
- Getters: `userId`, `displayName`, `email` (Step 122: the account's
  email address, `undefined` when it has none on record), `handle`
  (Step 125: the account's handle, `undefined` when it has none on
  record — as the session knows it from its login, like `displayName`:
  a `setHandle` in this session does not change it, though since Step 126
  the **next** session sees it),
  `deviceReplaced` (Step 110: this login's device replaced the recorded
  one, which the backend no longer had — older messages are gone).
- `storageEngine() -> {engine, version, cipher, plaintextHeaderSize,
  location} | undefined` (async) — what the app-data store's encryption
  engine reports about itself, read off the live connection (the browser
  flavor of `Client::storage_engine`; `SQLite3 Multiple Ciphers` /
  `2.5.1` / `chacha20` / `32` on today's build); `location` (Step 122)
  is `"server"` for databases on the storage server, else `"local"`.
- `pushState() -> "connected" | "polling" | "stopped"` (Step 122) —
  whether sync runs on push: the sync loop has an open socket, runs
  without one (polling every 10 s), or no loop runs (`startSync` not
  called, or `SyncHandle.stop()`). A method, not a getter: it changes,
  and tab proxies snapshot getters.
- Sync: `startSync(onMessage, onTyping?, onExpired?, onSync?,
  onDevicesChanged?, onStorage?) -> SyncHandle` — background `spawn_local` loop that drives
  `sync_once` and pushes decrypted events to the JS callbacks. Since Step 115 it
  waits on a push doorbell (`wasm/push_socket.rs`, the legacy `/await`
  socket): a frame drains within about a second, and receive itself stays the
  REST drain over `GET /v14/notifications` (ADR-0030). Since Step 118
  (`wasm/sync_pace.rs`) it does not poll beside a live socket: the socket's
  close wakes it at once to reconnect and drain, and a socket silent for
  a minute is renewed and drained once (a browser cannot ping, so a silently
  dead socket looks open). Without a socket it polls every 10 s and retries the
  socket, at most once per 10 s. A pass fetches the room list only when it is
  stale or older than 5 minutes (Step 118), so an idle pass is one request.
  A successful own send wakes the loop too (Step 122): the backend sends
  the sender no frame for its own message, so the pass that follows — and
  its `onSync` — is the embedder's cue to read the timeline again (the
  message itself is in `messages()` as soon as `sendText` resolves);
  `onSync` (Step 94) fires after every successful pass with the
  `{roomsUpdated, eventsDispatched}` summary. `syncOnce()` runs one pass and
  returns `{roomsUpdated, eventsDispatched}`.
- Rooms: `rooms() -> {roomId, name}[]`, `createRoom(name)`,
  `createDm(peer)`, `members(roomId)`, `invite(roomId, userIds)`,
  `kick(roomId, userIds)`, `leave(roomId)`, and since Step 98b
  `renameRoom(roomId, name)`, `deleteRoom(roomId)` (team-admin),
  `allowGuests(roomId)` (the **prerequisite** for a guest link — a
  Kupferdraht-created room defaults to `access=["invite"]`, against which Wire
  answers `403 access-denied`), `roomAccess(roomId)` /
  `setRoomAccess(roomId, modes, roles)`,
  `createJoinLink(roomId, password?) -> {url, passwordProtected}`,
  `revokeJoinLink(roomId)`, `joinViaLink(code, key, password?)` (an MLS
  external commit — `code`/`key` come from the link's query, and the link
  names the conversation; Step 122 dropped `domain`, BREAKING).
- Server-administered config (Step 98d): `featureConfigs()`,
  `fileSharing() -> {policy}` (`"disabled" | "enabled"`).
- Devices (Step 95d): `roomDevices(roomId) -> {userId, deviceId,
  credential: "basic"|"x509", thumbprint}[]` (one per MLS leaf;
  `thumbprint` is Wire's "MLS Thumbprint"), `ownDeviceThumbprint()`.
- Timeline: `sendText(roomId, body)` (MLS auto-join), `messages(roomId,
  limit?, before?)` (local decrypted read, oldest-first; `before` is a
  message id, Step 98c), `sendKnock(roomId, hotKnock)` /
  `sendLocation(roomId, lat, lon, name?, zoom?)` (Step 98c), `sendReply`,
  `toggleReaction`, `editMessage`, `redact`, `markAsRead` (`editMessage` /
  `redact` reject another user's message — Step 95b, ADR-0028).
- Typing: `startTyping(roomId)` / `stopTyping(roomId)` — outbound only;
  the browser prototype manages start/stop explicitly (the core 11-s
  auto-stop timer is native-only, `tokio::spawn` is cfg'd out on wasm).
- Assets (W7): `sendFile(roomId, bytes, filename, mimeType)` (in-memory
  encrypted send), `downloadAsset(roomId, itemId) -> bytes` (download +
  decrypt + SHA-256 verify).
- Session admin (Step 98a): `setDisplayName(name)`, `setHandle(handle)`,
  `updateEmail(email)` — the last one means "verification mail sent", not
  "address changed". Since Step 126 the first two also write the persisted
  `self_user` row, so the next `resume` reports the new value; the live
  `displayName` / `handle` getters are the login's snapshot either way.
- Devices (Step 98a): `devices() -> DeviceInfo[]`, `deleteDevice(deviceId,
  password)`. `DeviceInfo` is the core record field for field —
  `{deviceId, class, label, model, registeredAt, lastActive, isSelf}`.
- `recovery() -> Recovery` (Step 112) — the account's recovery data.
- `logout(options?)` — `POST /access/logout`. `options` is
  `{ purgeDevice?: boolean, password?: string, purgeLocal?: boolean }` —
  `logout({ purgeDevice, password, purgeLocal })`, the native shape plus
  `purgeLocal` (Step 114, ADR-0035); see "Leaving a browser" below.

**`class Recovery`** (Step 112, ADR-0035) — from `client.recovery()`:
- `state() -> { state: "disabled" } | { state: "enabled", exportedAt }` —
  `exportedAt`: when the current recovery data was made (milliseconds since
  the epoch).
- `enable(pin) -> Uint8Array` — the first recovery data; only while
  disabled (`InvalidInput` otherwise), and the PIN must pass
  `validateNewPin`.
- `changePin(currentPin, newPin) -> Uint8Array` — new recovery data under
  `newPin`; with the same PIN twice, a new copy of lost data.
  `currentPin` is checked locally: another one throws `WrongPin` without
  `attemptsRemaining`, so a running session alone cannot make recovery data
  under a PIN of someone else's choosing.
- `verifyPin(pin) -> boolean` — whether `pin` is the recovery PIN; local,
  for reminders.

**`class SyncHandle`** — `stop()`, idempotent (ends the loop and the
`onDevicesChanged` / `onStorage` registrations).

**JS callbacks** (plain functions passed to `startSync`):
- `onMessage({roomId, senderUserId, messageId, body, replyToId,
  receivedAtMs, expiresAtMs})` — one call per inbound decrypted
  message (`expiresAtMs` is set only for an ephemeral message, Step 89;
  `messages()` items carry the same field). `senderUserId` is the
  MLS-authenticated signer, not the backend's envelope — the same
  rule on every surface (Step 95a, ADR-0027; `docs/library.md`
  §Identifiers "Sender of an inbound message").
- `onTyping({roomId, typingUsers})` — optional. `conversation.typing` is
  a transient event that arrives only on a push socket; since Steps 115
  and 115b the browser's own socket delivers it (natively: the push
  transport, the default since Step 118).
- `onExpired()` — optional; fires once on the terminal
  `KupferdrahtError::SessionExpired`, then the loop stops (W10). Surface a
  re-login gate.
- `onSync({roomsUpdated, eventsDispatched})` — optional (Step 94); the
  per-pass summary `syncOnce` returns, after every successful pass of
  the loop. The leader tab broadcasts it as `sync`; a thin tab re-pulls
  `rooms()` when `roomsUpdated > 0` (the wasm surface has no room-list
  or timeline-diff events).
- `onDevicesChanged({roomId, added, removed})` — optional (Step 95d);
  one call per room whose MLS device set changed against the last
  persisted set (devices shaped as in `roomDevices`). `SyncHandle.stop()`
  ends it with the loop. The leader tab broadcasts it as `devices`.
- `onStorage(state)` — optional (Step 113, ADR-0038); `"reconnecting"`
  when the account's storage server became unreachable, `"online"` when
  the storage healed. Always `"online"` in the local mode (it never
  fires). The leader tab broadcasts it as `storage`; `client.storageState`
  is the current value.

### Many tabs, one instance — `bindings/web/kupferdraht-tabs/` (Step 94, ADR-0026)

A reference module (plain ES modules + JSDoc, hand-written `index.d.ts`,
no bundler, no runtime dependency) that turns ADR-0023's "one active
instance" into "served to every tab". Two halves:

- **Page side — `createTabHost({ worker })`** in every tab. It keeps one
  candidate dedicated worker alive, follows the leader announcements and
  exposes `host.kupferdraht`, a **`KupferdrahtProxy` with the same method names as
  the wasm `Kupferdraht` / `PendingLogin` / `Client` / `Recovery` / `SyncHandle`
  classes**
  (everything but the leader-only `Kupferdraht.open`; wrapper objects come
  back as proxies whose getters `userId` / `displayName` / `email` are
  snapshots, so they stay synchronous; the `Kupferdraht` getters `homeId` /
  `cookieLabel` ride on the leader's announcement and are `undefined`
  until one arrived). `host.state` is `electing` |
  `leader` | `thin`; `host.takeOver()` is the ADR-0023 override for a
  hung leader; `host.close()` terminates the candidate (a leader releases
  the lease). Events on the host: `leader-changed` `{leaderTabId, epoch,
  isSelf}`, `evicted` `{epoch, reason}` — `"tab"`: this tab led and
  another tab took over (the host terminated that worker and queued a
  fresh candidate); `"device"` / `"access"` (Step 111c): the leader's
  instance stopped for every tab — another browser took the account's
  databases over, or the storage no longer admits the user — and every
  tab hears it once from the leader's notice (a transport message of its
  own kind, `{kind: "evicted", leaderTabId, epoch, reason}`, that moves no
  tab's view of the leader); the next leader serves again, and the proxy's
  re-resume meets `HeldElsewhere` / `StorageAccessDenied`,
  `leader-unresponsive` (missed heartbeats — a hint only; the lock is the
  source of truth), `error`, and every leader broadcast: `message`,
  `typing`, `expired`, `sync`.
- **Worker side — `serveTab({ load, permitHttp, storage, logLevel, onLog })`** in the
  embedder's worker script — the only file that knows where the wasm
  package lives (`storage`, Step 111c, goes to the leader's `Kupferdraht.open`
  as it is — the server mode through the tabs layer):

  ```js
  import init, { Kupferdraht } from './pkg/kupferdraht.js';
  import { serveTab } from '../kupferdraht-tabs/worker.js';
  serveTab({ load: async () => { await init(); return Kupferdraht; }, permitHttp: true });
  ```

  The candidate queues one `navigator.locks.request("kupferdraht:instance")`
  (no `ifAvailable`); the browser grants in FIFO order and releases a
  dead tab's lock and queued request by itself — the queue is the
  election, crash-safe. On the grant the candidate opens Kupferdraht **while
  its JavaScript still holds the lease**, with `Kupferdraht.open({ takeOver:
  true })`: a lock is not reentrant within one agent, so the Rust
  instance *steals* the lease from the candidate's own callback (a steal
  preempts the queue, so no other tab can slip in between). From then on
  the worker dispatches every tab's requests to its instance, runs one
  sync loop per client (each tab's `startSync` is a subscription to it)
  and broadcasts the loop's events; it announces itself on acquire and
  every 2 s.
- **Transport:** `BroadcastChannel` `kupferdraht:tabs` for requests, events
  and announcements, `kupferdraht:tabs/<tabId>` for that tab's responses;
  envelopes `{kind: "request", tabId, requestId, target, method, args}` /
  `{kind: "response", requestId, ok, value | error}` — an `error` keeps
  its `name`, `message` and the recovery detail (`reason`,
  `attemptsRemaining`, `retryAfter`). The leader answers
  each `(tabId, requestId)` once. Payloads are the wasm view objects,
  structured-cloned (asset bytes are copied, not transferred).
- **Failover, honest by construction (L3):** when a new leader announces
  itself, an **idempotent** call in flight (`accounts`, `rooms`,
  `messages`, `members`, `downloadAsset`, `storageEngine`, `resume`,
  `syncOnce`, `initiateSso*`, `validateNewPin`, `recovery`,
  `Recovery.state` / `verifyPin`) is re-issued to it (a `Client` proxy
  re-resumes by `userId` first, a `Recovery` proxy asks its client
  again); a **mutation** in flight (`login`, `finalizeSso`,
  `registerDevice`, `recover`, `Recovery.enable` / `changePin`, every
  send / edit / redact / reaction / read marker / room change / typing /
  `logout`) rejects with
  `LeaderChangedError` (`name === "LeaderChanged"`) and is **never
  retried by the module** — a send that reached the backend before the
  leader died must not go out twice; the embedder offers "try again".
  Subscriptions (`startSync`) are re-established. A call while no leader
  is known waits `leaderWaitMs` (10 s) for an announcement, then rejects
  `NoLeaderError` — no offline queue; show "connecting". The
  classification lives in `protocol.js`'s `METHODS` table. A late
  heartbeat or event of an older leader (a lower epoch) moves no tab's
  view (Step 116) — `BroadcastChannel` orders nothing between senders, so
  an evicted leader's last message can arrive after its successor's
  announcement; such an event is still delivered.
- **Parity as a test:** `kupferdraht-core/tests/step_94_tab_proxy_parity.rs`
  enumerates the `#[wasm_bindgen]` methods and getters and fails on any
  name the table, `proxy.js` or `index.d.ts` does not mirror (and on any
  phantom entry) — the JS twin of the FFI-parity rule: a wasm-surface
  change lands with its proxy + `.d.ts` mirror in the same commit.
- **Proof:** `kupferdraht-core/tests/step_94_leader_tab.rs` drives the module
  in headless Firefox (`scripts/wasm-test.sh`) — RPC round trip with
  dedup and per-tab routing, event fan-out, election on
  `worker.terminate()`, read re-issue, in-flight mutation failing typed,
  `NoLeader`, the hand-off against the real Web Locks API, the takeover
  override, the recovery calls with their error detail and a `Recovery`
  handle across a leader change (Step 112b) — with a stub `Kupferdraht`; `bindings/web/opfs-smoke/tabs.html`
  is the consumer smoke with the real wasm (`?scenario=tabs` plays two
  tabs on one page: `TABS-OK`).
- **Not there:** `SharedWorker` hosting (ADR-0023 Option C), cross-origin
  or private-window sharing, idempotency keys for automatic mutation
  retry (a later step, `open.md`), and the Step-79 resend path on wasm.

Timeline items carry `deliveryStatus` (`"sending" | "submitted" | "received"
| "failed"`) and each reaction carries `isSelf` since Step 98c.

**Not on the wasm surface** (vs. UniFFI): registration
(`PendingClient`),
`TimelineListener`/`subscribe`, `SyncListener`-driven `start_sync`,
`Asset.save_to` (no filesystem), mentions/ephemeral
sends, failed-send recovery (`resend` / `resend_failed` /
`failed_sends`, Step 79). The core `sync_stream` is a stub on wasm (single `Err`) — use
`startSync`. Ephemeral expiry on wasm is **sweep-only** (no task
spawner): inbound ephemeral rows self-redact at the end of the next
`syncOnce` / `startSync` cycle or on resume, not on a live timer. The
sweep itself is the same SQLite sweep native runs (Step 91 closed the
IndexedDB no-op).

**Verification status.** The browser surface is developed against the
W1–W10 prototype; Steps 53/55/56/57 record their live browser tests as
pending (see the step docs in `docs/plan/archive/`). Treat end-to-end
claims accordingly.

## Packaging

Each track has a build script (under `bindings/`) that cross-compiles the
native lib(s), generates the binding source, and assembles the artifact.
The generated source and native libs are build outputs (git-ignored); run
the script before building / testing.

### Swift (iOS + macOS) — XCFramework + SwiftPM
```
./bindings/swift/build-xcframework.sh [debug|release]
cd bindings/swift && swift test
```
Builds `kupferdrahtFFI.xcframework` (macOS arm64, iOS device + simulator arm64
when their `rustup` targets are installed) and the generated `kupferdraht.swift`,
consumed through `Package.swift` (`Kupferdraht` library target).

### Kotlin / JVM (desktop) — JAR
```
./bindings/kotlin/build-jvm.sh [debug|release]
cd bindings/kotlin && ./gradlew test
```
Generates the Kotlin surface and bundles `libkupferdraht.dylib` under the
JNA-resolved resource dir; produces a JAR. UniFFI Kotlin is JNA-backed and
uses kotlinx-coroutines for the `async` exports.

### Android — AAR
```
./bindings/android/build-android.sh [debug|release]   # needs the NDK + cargo-ndk
cd bindings/android && ./gradlew assembleRelease
```
`cargo-ndk` cross-compiles `libkupferdraht.so` (arm64-v8a) into `jniLibs`; the
AGP `com.android.library` module packages it + the generated Kotlin + JNA
(`@aar`) into an AAR.

### Kotlin Multiplatform — bindings/kmp
```
cd bindings/kmp && ./gradlew buildKupferdrahtNatives   # after a Rust change
./gradlew captureKupferdrahtNatives                    # first setup / native-dep bumps
```
A standalone Gradle module (`io.github.schwarzdigits:kupferdraht-bindings`) that generates
the KMP `uniffi.kupferdraht` binding itself — commonMain `expect` + JVM /
Android / iOS `actual`s (targets `jvm`, `androidTarget`, `iosArm64`,
`iosSimulatorArm64`) — via gobley's cargo + uniffi plugins, from Kupferdraht's
**prebuilt** native libraries. gobley's own Cargo build is disabled on an
ordinary run; `buildKupferdrahtNatives` cross-builds every target's native lib
then regenerates the binding, `captureKupferdrahtNatives` re-snapshots the
link metadata into `native/` — only those two invoke Rust, so a
day-to-day build needs no Rust toolchain. natrium-core consumes this
module through a Gradle composite build (`includeBuild`, per the module's
file headers; the consumer-side wiring lives in natrium's repo) instead of
carrying the gobley/cargo wiring itself — extracted out of natrium-core
into this repo (Step 42) so the binding is produced next to the Rust code
it binds.

### Web — wasm-pack
```
export CC_wasm32_unknown_unknown=/opt/homebrew/opt/llvm/bin/clang     # a wasm-capable clang
export AR_wasm32_unknown_unknown=/opt/homebrew/opt/llvm/bin/llvm-ar
wasm-pack build --target web
```
Builds the `wasm32-unknown-unknown` artifact + generated TypeScript
glue from the `wasm/` module. There is no `bindings/wasm/` build script
yet — the invocation above is the whole build (run from the
`kupferdraht-core` crate directory). Since Step 91 the build compiles the
SQLite3MC C for wasm (`sqlite-wasm-rs` over `vendor/sqlite3mc/`), so it
needs a clang with the WebAssembly backend: Apple's clang has none
(`brew install llvm`, paths as above — machine-specific, never
committed); distro clang and the GitHub runners' clang do. The browser
tests run through `scripts/wasm-test.sh` (headless Firefox via
geckodriver; the storage tests in a dedicated worker — the test runner
serves the page cross-origin isolated) — the same script CI runs;
`wasm-pack test` is not the entry point because the crate's unit-test
harness is native-only. `bindings/web/opfs-smoke/` (the name is historical)
is the consumer smoke: a page hosting Kupferdraht in a dedicated worker
(`worker.js`) — serve it with the two isolation headers after a
`wasm-pack build --target web --out-dir ../bindings/web/opfs-smoke/pkg`,
reload to see the store survive a session, or drive the login vertical
against a Wire instance from its form (`?scenario=login` scripted, with a
local proxy serving the Wire API and `/smoke-login.json` same-origin). `tabs.html` next to it is the multi-tab smoke over
`bindings/web/kupferdraht-tabs/` (Step 94): open it in several tabs, close the
leader, watch the next tab take over; `?scenario=tabs` runs the scripted
two-tab check.

## Scope

- **Kotlin Multiplatform**: Kupferdraht's `bindings/kmp` module generates the
  KMP `expect` / `actual` UniFFI wrapping itself (see "Kotlin Multiplatform
  — bindings/kmp" above) — not natrium. Natrium-core consumes the result
  as a Gradle composite-build dependency instead of owning that
  generation step.
- **WASM / web** is a separate hand-written `wasm-bindgen` track — shipped
  since PR #44 (Steps 44–59), documented above. The browser architecture
  is recorded in ADR-0015 (WASM-only delivery, hand-written bindings —
  superseding ADR-0005) and ADR-0022 (persistence: SQLite3MC over OPFS,
  superseding ADR-0015 Part 2).
