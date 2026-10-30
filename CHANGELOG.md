# Changelog

All notable changes to Kupferdraht, in
[Keep a Changelog](https://keepachangelog.com) style. Entries accumulate
under **[Unreleased]** as steps land — the Step Procedure's cleanup phase
(CLAUDE.md) adds them, written consumer-facing. Cutting a release stamps them
into a version (see "Release process" below). Versioning is SemVer —
pre-1.0, so a breaking change bumps the minor (`0.1` → `0.2`), everything
else the patch.

## Release process

A release is cut occasionally (after several steps), not per step. There
is no skill for it — it is this short procedure:

1. **Derive the next version** from the `[Unreleased]` categories and the
   current `version` in `Cargo.toml` (`[workspace.package]`). Pre-1.0
   (`0.y.z`): any **BREAKING** entry, or a source-incompatible
   `Changed` / `Removed`, bumps the **minor** (`0.1.z` → `0.2.0`); only
   compatible `Added` / `Fixed` / `Deprecated` / `Security` bumps the
   **patch** (`0.1.0` → `0.1.1`). Confirm the version before changing
   anything; never release an empty `[Unreleased]`.
2. **Stamp the changelog** — rename `## [Unreleased]` to
   `## [x.y.z] - YYYY-MM-DD` (`date -I`), dropping the empty category
   subsections, and insert a fresh empty `## [Unreleased]` (with the six
   category headings) above it.
3. **Bump the version** — set `version = "x.y.z"` in `Cargo.toml`
   `[workspace.package]`; the five-check gate (CLAUDE.md "Warnings are
   Errors") must stay green afterwards (`Cargo.lock` updates too).
4. **Commit + tag — offer, don't push.** On confirmation:
   `git add CHANGELOG.md Cargo.toml Cargo.lock`,
   `git commit -m "chore(release): vX.Y.Z"`, `git tag vX.Y.Z`. Leave the
   tag local — the user pushes.

## [Unreleased]

### Fixed

- **An asset upload response without `domain` falls back to the user's
  own backend domain** — not to Wire's hosted domain, which the fallback
  had hard-coded. Current v14 servers always send the field, so the path
  was dead in practice; on a self-hosted deployment that ever omitted it,
  peers would have been told to download from the wrong backend.

- **Compliance analyzer, second round** — the stand-in and the JVM
  binding build are excluded from ORT's analyzer (the Cargo plugin fails
  on the stand-in's wasm32-only dependencies, the Gradle Inspector
  resolves no plugin in the container); the ORT scanner is disabled
  because the central workflow names a scanner ORT 96 no longer ships.
  Both upstream issues are open.md §79.

- **The compliance workflow's license analysis runs through** — the
  vendored `libsqlite3-sys` stand-in is excluded from the Cargo workspace
  so `cargo metadata` resolves it on its own; a pre-analyzer script pins
  Gradle to the runner's JDK 17 inside the ORT container; the Swift
  manifest (a local binary target only) and the Android and Multiplatform
  builds (Android SDK and cargo at configuration time, neither available
  in the container) are excluded from the analyzer, and
  THIRD-PARTY-NOTICES lists the components the Kotlin bindings' consumers
  resolve from Maven Central.

- **The third-party license report lists the vendored engine and the git
  dependency** — `docs/THIRD_PARTY_LICENSES.html` gains a hand-kept section
  for SQLite3 Multiple Ciphers (MIT, `vendor/sqlite3mc/`) and
  `sqlite-remote-vfs` (Apache-2.0, git-pinned, `publish = false`), which
  cargo-about skips as private. The section lives in `about.hbs`, so a
  regeneration keeps it.

- **A profile change is remembered, not only sent** — Step 126. `set_handle`
  and `set_display_name` now write the persisted `self_user` row after the
  backend accepts the change, so a `resume` reports the new handle or name
  instead of the one this device last logged in with. Before, the change was
  invisible to the SDK until the next login: `Account` is immutable and a
  resume rebuilds it from that row, which only the post-login persistence
  wrote. The live `Account` of the running session is unchanged — a consumer
  that just made the call knows the value.

### Added

- **`.gitleaks.toml`** — the compliance workflow's secret scan flagged
  twelve known-answer values over the git history (the PIN-service interop
  vectors, the planted fake token of the no-secrets test, a cipher name in
  the threat model, the once-committed Swagger); the allowlist names them
  line by line and leaves every other rule in force.

- **Compliance workflow** — `.github/workflows/compliance.yml` runs the
  Schwarz Digits checks (Gitleaks over the git history, Trivy, ORT license
  analysis and SBOM) on pull requests, pushes to `main` and weekly;
  `.ort.yml` repeats the central excludes and adds the three files that
  reproduce third-party license texts, so the scanner does not attribute
  them to Kupferdraht.

- **`NOTICE`** — copyright and license of Kupferdraht, the Wire schema
  portion under GPL-3.0-or-later, the pointer to THIRD-PARTY-NOTICES and
  the trademark note, after the natrium precedent. The trademark note in
  README, NOTICE and THIRD-PARTY-NOTICES links the Schwarz Digits trademark
  policy for "Kupferdraht".

- **`CONTRIBUTING.md`** — points to the Schwarz Digits open-source
  guidelines (Code of Conduct, Contributor License Agreement, pull-request
  workflow, security reporting) and lists what this project adds: the
  charter, the independence rule, the gate, branch and commit conventions.

- **`THIRD-PARTY-NOTICES`** — the attribution file at the repository root:
  every component Kupferdraht links or compiles, grouped by license with
  authors, repository and target scope (browser-only, native-only),
  generated from the Cargo graph for all targets; plus the components
  outside that graph (Wire's schema, SQLite3MC, SQLite, the libsqlite3-sys
  stand-in, sqlite-remote-vfs, the protoc binary, the Gradle wrapper,
  matrix-rust-sdk as the conceptual model) and a trademark note. The HTML
  report keeps the full license texts.

- **SPDX notice on every authored source file** — Rust, Kotlin, Swift,
  JavaScript/TypeScript, HTML, shell and the CI workflow open with
  `SPDX-FileCopyrightText: 2026 Schwarz Digits KG` and
  `SPDX-License-Identifier: GPL-3.0-only`; `tests/license_headers.rs`
  names any file that lacks them. Not tagged: vendored third-party code,
  the Gradle wrappers, the SHA-pinned SQL migrations and the Protobuf
  schema, which carries its own notice (CLAUDE.md "License Headers").

- **Browser: the account's handle** — Step 125, asked for by an embedder of
  the browser surface. The getter `client.handle` is the account's handle, `undefined`
  when it has none on record — beside `displayName` and `email`, as the
  session knows it from its login (a `setHandle` in the same session does
  not change it, as for `displayName`). A consumer mirroring
  a session's profile had to fetch this one string over the REST API
  itself. Mirrored in `kupferdraht-tabs`, so tab proxies answer it from the
  snapshot the leader hands out.

- **Browser: push state, email, storage location, a rate limit's wait** —
  Step 122 (the natrium web SDK's gap list). `client.pushState()` answers
  `"connected"` (the sync loop has an open socket), `"polling"` (it runs
  without one) or `"stopped"` (no loop) — asked each time, so tab
  proxies ask the leader. The getter `client.email` is the account's email
  address (`undefined` when it has none on record). `storageEngine()`
  gains `location`: `"server"` for databases on the storage server, else
  `"local"`. A `RateLimited` error carries `retryAfter` (seconds) when
  the backend sent `Retry-After`, as `TooManyAttempts` does.

- **Kupferdraht's logs reach the app** — Step 117 (gap G11 of the natrium web
  SDK). `init_logging(level, listener)` (Kotlin/Swift `initLogging`, a
  `LogListener` with `onLog(record)`; browser `Kupferdraht.open({ logLevel,
  onLog })`, kupferdraht-tabs `serveTab({ logLevel, onLog })`) hands Kupferdraht's
  own log events at the level or above to the app as `{ level, target,
  message }`. Kupferdraht writes to no console or stream itself — without a
  listener it stays silent, as before; the app decides where logs go. A
  later call switches level and listener at runtime.

- **Browser: leaving a browser** — Step 114 (ADR-0035).
  `client.logout({ purgeDevice, password, purgeLocal })`: `purgeDevice`
  now also deletes the account's databases on the storage server (another
  browser of the account shares the device, so its session ends with it:
  it logs in again and starts with empty databases); the new `purgeLocal`
  forgets the account in this browser — its keys, its local databases and
  its registry entry (recovery data still restores it). Both together
  leave a browser for good. The backend comes first: a wrong password
  changes nothing. After a purge the client is closed. `Store` gains
  `close` with a default body (additive).
- **Browser: recovery data** — Step 112b (ADR-0035). The account's seed
  sealed with a PIN, as opaque bytes the app offers as a download.
  `Kupferdraht.open({ storage: { mode: "server", …, pinServiceUrl } })` names the
  PIN service (from the app's configuration). `client.recovery()` has
  `state()` (`disabled`, or `enabled` with `exportedAt`), `enable(pin)` (the
  first recovery data), `changePin(currentPin, newPin)` (new data under a
  new PIN; the current PIN checked locally) and `verifyPin(pin)` (local).
  `pendingLogin.recover(data, pin, { takeOver })` restores the account in a
  browser without its keys and registers the device; a wrong PIN leaves
  nothing and reports the attempts left. `Kupferdraht.validateNewPin(pin)` →
  `"ok"`, `"tooShort"` or `"tooCommon"` (at least 6 characters, any
  characters). The recovery errors carry their detail as properties
  (`reason`, `attemptsRemaining`, `retryAfter`), also through kupferdraht-tabs.
  `cancel()` after a recovery forgets the keys in this browser and keeps
  the account's databases. The PIN service limits the guesses; the local
  mode has no recovery data.
- **BREAKING: `Store` keeps the recovery state** — Step 112b.
  `save_recovery_state` / `load_recovery_state` with
  `RecoveryStateRecord { exported_at_ms, pin_verifier }` (migration V30);
  a custom `Store` must implement them.

- **Browser: an account's databases on a storage server** — Step 111b
  (ADR-0035). `Kupferdraht.open({ storage: { mode: "server", url,
  tokenExchangeUrl } })` keeps each account's `kupferdraht.db` and `mls.db` on a
  `sqlite-remote-server`; `global.db` and the keystore stay in the browser.
  Both addresses come from the app's configuration. The server keeps an
  account's databases under its storage-login key, derived from the
  account's seed like the database keys, so the same seed finds them in any
  browser; storage tokens come from the token exchange for the Wire session
  and are renewed in the background. `resume` renews the Wire session with
  the browser's login cookie before the databases open, and opens without
  creating them. New error names: `NothingStored` (the server no longer
  has the account's databases — log in again), `StorageAccessDenied`,
  `ServiceUnavailable`; `onEvicted` also reports `"device"` (another browser
  took the account's databases over) and `"access"` (the token exchange no
  longer admits the user).

- **Browser: the databases on sqlite-remote-vfs, local mode** — Step 111a
  (ADR-0035). `global.db`, `kupferdraht.db` and `mls.db` move from OPFS to
  `sqlite-remote-vfs`, one registration per database, SQLite3MC encrypting
  above it; the local mode keeps each as an IndexedDB database under the
  home id. `Kupferdraht.open` takes `storage: { mode: "local" }` (the default;
  the server mode follows in 111b), `onEvicted` receives the reason
  (`"tab"`), and refused opens are named: `HeldElsewhere`, `StorageFull`,
  `Evicted`. The home id is now the keystore entry `kupferdraht.home-id`.

- **Browser: the account's keys come from its seed; a login waits for it**
  — Step 110 (ADR-0035). In the browser a new key of an account is derived
  from the account's seed (HKDF-SHA-256) instead of drawn at random, and
  stored and loaded as natively; `global.db` stays native. A login in a
  browser without the account's seed opens and stores nothing until the
  embedder decides: `PendingLogin.keys` (`"present"` / `"missing"`),
  `recoveryExpected` (the account has a registered browser device, so
  recovery data exists somewhere), `registerDevice({ newSetup: true })` for
  a first setup, `cancel()`. `Client.deviceReplaced` tells that the native
  device reuse registered a new device because the backend no longer had
  the recorded one; `Client.logout({ purgeDevice, password })` takes the
  native options. Browsers register their devices with the model
  `Kupferdraht (Web)`.

- **Browser: push receive** — Step 115. The browser's live-sync loop
  (`startSync`) now waits on a push doorbell instead of its fixed 2.5 s timer,
  so an inbound message lands in about a second. The socket is a
  `web_sys::WebSocket` on the same legacy `/await` route the native push
  transport negotiates, and it obeys ADR-0030 exactly: a frame only triggers the
  REST drain the loop would have run anyway, and nothing but the drain advances
  the cursor.

  Nothing to configure and nothing to call — `startSync` behaves as before, only
  sooner. If the socket cannot be opened or the backend closes it, the loop falls
  back to its 2.5 s timer and reconnects on the next pass, so receive never stops.

  Two differences from the native transport, both forced by the browser. The
  credentials ride the query string, because no page can set the `Authorization`
  header the native socket uses. And the browser sends **no keepalive**: the
  WebSocket API exposes no `ping()`, and a socket that sent nothing at all was
  measured open for 392 s against a live backend, so none is needed.

  Typing arrives through it too (Step 115b): `transient: true` frames are
  live-only — no drain can ever return one — so they are dispatched as data
  through the registered `EventHandler` without touching the cursor, and they
  deliberately do **not** trigger a drain. A consumer that runs its **own**
  `/await` socket must stop doing so when it adopts this, because the backend
  terminates the previous socket of the same client id.

- **Browser: session and device admin** — Step 98a. The wasm surface binds
  what the UniFFI one already had: `Client.setDisplayName(name)`,
  `setHandle(handle)`, `updateEmail(email)`, `devices()` and
  `deleteDevice(deviceId, password)`, plus `PendingLogin.devices()` /
  `deleteDevice(deviceId)`. Devices cross to JS as the core `DeviceInfo`
  record field for field (`{deviceId, class, label, model, registeredAt,
  lastActive, isSelf}`).

  The `PendingLogin` pair is the one that mattered: a browser user who hit
  Wire's seven-device cap during login was stuck with no way back, because the
  documented recovery — list, free a slot, `registerDevice` again on the same
  handle — stopped at the wasm boundary. It no longer does.

  `updateEmail` means "verification mail sent", not "address changed"; Wire
  applies the change when the link in that mail is followed.

- **`kupferdraht-cli email-update --email <addr>`** — Step 98a. The CLI consumer
  for `Client::update_email`, which had none.

- **Browser: room admin and guest links** — Step 98b.
  `Client.renameRoom(roomId, name)`, `deleteRoom(roomId)` (team-admin),
  `createJoinLink(roomId, password?) -> {url, passwordProtected}`,
  `revokeJoinLink(roomId)` and `joinViaLink(code, key, domain, password?)`.
  Redeeming a link is an MLS external commit, so it is the one of these a web
  client had no way to approximate.

  Also `roomAccess(roomId)`, `setRoomAccess(roomId, modes, roles)` and
  `allowGuests(roomId)` — a Kupferdraht-created conversation defaults to
  `access=["invite"]`, so opening it to guests is the prerequisite for a link,
  not an extra. Without it `createJoinLink` fails `403 access-denied` on every
  fresh room.

- **Browser: timeline parity** — Step 98c. `messages()` takes the `before`
  cursor `MessagesOptions` always had, so a browser can page back through a
  conversation instead of seeing only its newest page. Timeline items now
  carry `deliveryStatus` (`"sending" | "submitted" | "received" | "failed"`),
  and each reaction carries `isSelf` — a UI could previously not tell a sent
  message from a failed one, nor which reaction was its own. Adds
  `sendKnock(roomId, hotKnock)` and `sendLocation(roomId, latitude,
  longitude, name?, zoom?)`.

- **Browser: server-administered config** — Step 98d.
  `Client.featureConfigs()` and `fileSharing() -> {policy,
  allowedExtensions}`, so a web client can say *why* a send was refused
  instead of reporting a generic failure.

- **`UserId::from_parts(localpart, domain)`** — Step 95c. The checked
  counterpart of `UserId::new` for parts that come from runtime data:
  `InvalidInput` when a part is empty or contains `@`.

- **Room devices and MLS thumbprints, with a device-change signal** —
  Step 95d. `Room::devices()` lists a room's devices — one per leaf of
  its MLS group, the own device included — with the Wire identity, the
  credential type (Basic / e2ei X.509) and the **MLS thumbprint** of the
  leaf's signature key: the RFC 7638 JWK thumbprint official Wire apps
  show as "MLS Thumbprint", so a user can compare a device across
  clients (`Client::own_device_thumbprint()` for this device).
  `Client::subscribe_device_changes(listener)` reports, for every room,
  each change of its device set — `added` / `removed`, a re-keyed device
  as both — diffed against the set Kupferdraht persisted for the room (new
  table `room_devices`, schema V29), so a change made while no client
  ran is still reported at the room's next epoch change. Under
  `BasicCredential`s the backend distributes the keys: this makes an
  injected device **visible**; official Wire clients give no such signal
  without e2ei. The first observation of a room records the baseline
  silently; `sync_once` records one for every room that has none before
  it drains, so a room's first real change (e.g. right after upgrading)
  is reported, not swallowed. Mirrored everywhere: FFI `Room.devices`,
  `Client.own_device_thumbprint`, `Client.subscribe_device_changes` +
  foreign `RoomDevicesListener`; wasm `roomDevices`,
  `ownDeviceThumbprint`, `startSync`'s optional fifth callback
  `onDevicesChanged`; `kupferdraht-tabs` proxy + `devices` leader event; CLI
  `room-devices` and the `mls_thumbprint` line in `status`. New
  defaulted `Store::load_room_devices` / `replace_room_devices` (custom
  stores keep compiling, and simply report no changes).
- **Many tabs, one Kupferdraht instance in the browser —
  `bindings/web/kupferdraht-tabs/`** — Step 94 (ADR-0026, extends ADR-0023).
  ADR-0023 left a web app with one usable tab and a takeover dialog for
  every other. Now the tab whose dedicated worker holds the origin's
  lease is the **leader** and every other tab is a **thin client** that
  reads and sends through it: on the page, `createTabHost({ worker })`
  exposes `host.kupferdraht`, a `KupferdrahtProxy` with the same method names as
  the wasm `Kupferdraht` / `PendingLogin` / `Client` / `SyncHandle` classes
  (everything but the leader-only `Kupferdraht.open`); in the worker,
  `serveTab({ load })` queues one `navigator.locks.request("kupferdraht:instance")`
  per tab and, on the grant, opens Kupferdraht by the hand-off
  `Kupferdraht.open({ takeOver: true })` inside the grant callback (a lock is
  not reentrant within an agent; a steal preempts the queue). Closing or
  crashing the leader tab promotes the next queued tab through the
  browser's own lock release — no dialog, no heartbeat in the loop (the
  heartbeat is a UI hint). Transport: `BroadcastChannel` (`kupferdraht:tabs`
  for requests, events and the leader announcement, `kupferdraht:tabs/<tabId>`
  for that tab's responses; dedup per `(tabId, requestId)`); the leader
  runs one sync loop per client and broadcasts `message` / `typing` /
  `expired` / `sync`, every tab's `startSync` is a subscription to it.
  **Failover is honest:** an idempotent call in flight (`accounts`,
  `rooms`, `messages`, `members`, `downloadAsset`, …) is re-issued to the
  new leader, a mutation in flight (`sendText`, `login`, `registerDevice`,
  …) rejects with `LeaderChangedError` and is never retried by the
  module (a send that reached the backend must not go out twice), a call
  with no leader rejects `NoLeaderError` after a bounded wait. Plain ES
  modules + JSDoc + hand-written `index.d.ts`, no build step, no
  dependency; opt-in — the single-tab `Kupferdraht.open` path is untouched.
  The proxy-parity test (`tests/step_94_tab_proxy_parity.rs`) is the JS
  twin of the FFI-parity rule: a wasm-surface change lands with its
  proxy + `.d.ts` mirror in the same commit (Step 93's `homeId` /
  `cookieLabel` were the first case). Consumer smoke
  `bindings/web/opfs-smoke/tabs.html`; live-proven 2026-09-25 against a
  Wire instance from two real Firefox tabs through a same-origin dev
  proxy (observations §87 — the CORS, proxy-root and `zuid` `Path` facts
  a web embedder needs).
- **`Client.startSync(onMessage, onTyping?, onExpired?, onSync?)` on
  wasm** — Step 94. The optional fourth callback fires after every
  successful pass with `{ roomsUpdated, eventsDispatched }` (the per-pass
  summary `syncOnce` returns); additive, existing calls unchanged. It is
  the signal a thin tab re-pulls the room list on (the wasm surface has
  no room-list or timeline-diff events).
- **Failed-send recovery: `Timeline::resend`, `Timeline::resend_failed`,
  `Room::failed_sends`** — Step 79. A send that fails offline has always
  persisted as a `Failed` row (Step 25a) but was a dead end — nothing in
  core retried it and there was no API to. Now `Timeline::resend(message_id)`
  re-drives one row from its persisted state under the **same
  `message_id`** (matrix-rust-sdk's `SendHandle::unwedge` shape; receivers
  dedupe if the first attempt reached the backend behind a 5xx proxy):
  text incl. reply / mentions, ephemeral text (re-wrapped with its
  original timer — the chosen TTL is now persisted, see Changed), knock,
  location, and files (the blob already sits on `/v14/assets`, only the
  MLS reference travels again). It re-encrypts at the current epoch,
  uploads once, flips the row to `Submitted`, re-stamps `sent_at_ms` and
  emits `TimelineDiff::UpdateAt`; a transport error leaves the row
  `Failed`. Task / case / marker snapshots are refused with `InvalidInput`
  naming the state-sending API (`update_task` / `set_case` /
  `set_task_list`) — a stale full-state replay could regress every
  receiver's fold. `Timeline::resend_failed() -> ResendSummary { resent,
  skipped, stopped_at, remaining }` re-drives a room oldest-first and stops
  at the first hard error so a stuck message is never overtaken (rows
  behind the stop stay `Failed`, the pass is re-runnable);
  `Room::failed_sends()` lists the rows. No background scheduler — the
  **reconnect contract** is now written down (`docs/library.md`
  "Reconnect contract", mirrored in `docs/bindings.md`): the sync loop
  returning on a hard error *is* the contract, reconnect / backoff and
  resend-on-recovery are the consumer's loop, no network observer in
  core. FFI: `Timeline.resend` / `resend_failed` → `ResendSummary` +
  `FailedResend`, `Room.failed_sends`. CLI: `resend --message <id>`,
  `resend --id <room> --all`, `failed-sends --id <room>`.
- **Self conversation (notes-to-self): establish, join, send** — Step 86.
  Wire's per-user self conversation (`RoomType::SelfRoom`, one MLS group
  of the user's own devices) is usable now. Every send into it used to
  fail with `MlsGroupMissing`: Wire lists the room at epoch 0 with a
  `group_id` but serves no `GroupInfo` (`404 mls-missing-group-info`),
  so an external commit is impossible there (live-verified 2026-09-24).
  `Client::join_room` establishes it from the first device (the
  Step-31 commit — the user's other devices Added, a self-update when
  solo) and external-joins from every later one; `join_all_rooms`,
  `Room::join` and `Timeline::send_text_auto_join` inherit the branch,
  so a fresh device heals the room on its normal bootstrap (it counts
  under `JoinSummary::joined`). A rejected establishing bundle discards
  the local group so the next `join_all_rooms` retries. New
  `Client::self_room() -> Option<Arc<Room>>` (sync local lookup beside
  `dm_room`; FFI `Client.self_room`) and CLI `self-room`. Non-self
  rooms are unchanged. This is the private cross-device channel later
  features ride (personal task list, `LastRead` read position, drafts).
- **Ephemeral (self-deleting) text messages** — Step 89. Wire's
  `Ephemeral` wrapper (`GenericMessage` field 18) leaves the
  reserved list. Send: `Timeline::send_ephemeral_text(body, ttl:
  Duration)` (`send_text` is untouched) wraps a `Text` with
  `expire_after_millis = ttl` (1 s … 365 days, `InvalidInput`
  otherwise) over the usual MLS path; official clients render a
  countdown and delete their copy locally, Kupferdraht's own row
  self-redacts after `ttl` too. Receive: an inbound `Ephemeral(Text)`
  from any client surfaces as an ordinary `Text` item / `Message`
  decorated with the new `expires_at_ms: Option<i64>` (also on
  `TimelineEventRecord`; Rust consumers that build these structs
  literally gain one field — `None` for every ordinary message); the
  wrapper's other inner arms (image/knock/asset/location) are
  tolerated-unknown. Timer start is dispatch/send time (a headless
  client has no viewport — never later than an official client's
  view-triggered start). Expiry tombstones the row like a
  `MessageDelete` (`TimelineItemKind::Redacted`, stamp kept, one
  `TimelineDiff::UpdateAt`) from a per-row native timer, at the end of
  every dispatch cycle, and on resume — and, as receiver, broadcasts
  the delete-for-everyone official senders wait for (V1b; own rows
  stay local; the self-conversation `DeleteForMe` is a follow-up now
  that Step 86 established the self room).
  A failed send keeps its body with no timer. Storage: V27
  `timeline_events.expires_at_ms` + partial index. CLI:
  `send --ephemeral <secs>`; `timeline` prints `⏱ until …` /
  `(expired)`. FFI: `send_ephemeral_text`, `expires_at_ms` on
  `TimelineItem` + `Message`; wasm views carry `expiresAtMs` (expiry
  on wasm is sweep-only, IndexedDB store still no-op). Live-smoked
  2026-09-15/16 (scripted round-trip incl. the receiver's delete
  reaching the sender; webapp eyes-on pending). Driver: one-time-code
  delivery as a use case. Closes the
  open.md §64 ephemeral item. (Step 89)

- **Task-list room marker + late-join re-broadcast (ADR-0020)** —
  Step 75. A room can be marked as a dedicated task list by ANY
  member: `Room::set_task_list(bool)` sends a `TaskListMarker`
  (`GenericMessage` field 262002, bare payload — official Wire
  clients see nothing), `Room::is_task_list()` folds it
  last-writer-wins in event order (redacted rows drop out); marker
  rows surface as `TimelineItemKind::TaskListMarker` /
  `MessageBody::TaskListMarker` (the audit trail of the deliberately
  forgeable flag — never filtered by the library). On invite, after
  the Add-commit merges, the inviter now re-sends the union of the
  room's anchors — case snapshot (existing ADR-0018 MUST) and/or
  marker — plus **every** task's current folded state once (all
  tasks incl. done/cancelled, always without fallback, best-effort
  warn): a late joiner's first sync shows the complete board in both
  room types. Unanchored rooms keep the empty-on-join behavior.
  Existing members absorb the re-sends as fold no-ops (timeline rows
  still appear). Note: the re-broadcast sends the inviter's
  *current* fold — one-shot processes should sync before inviting
  (the CLI does). CLI: `task mark --id <room> [--clear]`;
  `task list` shows the marker. All FFI-mirrored. Live-smoked
  2026-08-26 end-to-end incl. a done task reaching the late joiner
  and the case+marker union room. (Step 75, ADR-0020)

- **BREAKING: `with_fallback` is now `Option<bool>` on all three
  task send paths** — `Timeline::send_task` / `update_task` /
  `move_task` in Core and FFI (Step 75, ADR-0020 decision 4).
  `None` derives the default from the room's task-list marker
  (marked → bare payload, unmarked → text fallback); an explicit
  `Some(...)` always wins. Callers passing `bool` wrap it in
  `Some`. CLI: `--fallback` joins `--no-fallback` (neither = auto).

- **Per-room case state (ADR-0018)** — Step 72
  (a/b/c). A room can carry one typed case: `CaseSnapshot`
  (external_ref (the file reference), title, case_type, state enum incl.
  CANCELLED, state_detail, assignee, due/opened/closed, priority,
  authority_ref, display-only `extensions`), carried as full
  snapshots in `GenericMessage` field 262001 with **no text
  fallback** (plain Wire clients discard the payload; pinned at the
  proto builder). Reads: `Room::case()` — fold-on-read, admin core
  LWW by `revision` with receive-time authorization captured per
  row, any-member wholesale `extensions`, redacted rows drop out;
  case rows surface as `TimelineItemKind::Case` /
  `MessageBody::Case`. Writes: `Room::set_case(CasePatch)` (set-only
  patch, revision bump, client-side conversation-admin gate), and
  `Client::invite_members` re-broadcasts the current snapshot right
  after the Add-commit merges — a fresh member sees the case on
  their first sync (closes the MLS no-history late-join gap for
  case state). Authorization anchors on Wire conversation roles:
  `RoomMember.role: RoomMemberRole` (decoded from
  `conversation_role`, persisted via V26, merged — not reset —
  by `sync_members`). CLI: `case-set` / `case-show`;
  `room-members` shows `[admin]`. All FFI-mirrored. Live-smoked
  2026-08-21 end-to-end incl. the invite re-broadcast. (Step 72,
  ADR-0018)

- **Unread counts per room** — `Room::unread_count()` and the
  room-list bulk `Client::unread_counts()` (one grouped query, no
  n+1; zero-unread rooms absent from the map), both FFI-mirrored.
  Unread = inbound messages, not redacted, newer than the new
  device-local read marker (`room_read_markers`, migration V25 —
  cross-device sync of the position rides the self-conversation in a
  later step); no marker → everything inbound counts.
  `Timeline::mark_as_read` now advances the marker — also when
  `ReceiptMode::Off` suppresses the outbound confirmation (the gate
  governs the wire signal, not the local read position), and the
  marker only moves forward. CLI: `rooms` shows `unread: N`.
  (Step 67)

- **Live push receive over `GET /v14/events`** — the
  ConsumableNotifications WebSocket returns as an opt-in receive
  transport on native targets. `SyncSettings.transport =
  SyncTransport::WebSocket` switches the loop family (`sync`,
  `sync_with_callback`, `sync_stream`, FFI `start_sync`) from the
  polling drain to a long-lived push socket: per-frame ack strictly
  **after** the cursor persist (an MLS `Commit`/`Welcome` is never
  consumed server-side before it is durable locally), redelivery
  dedup, reconnect with capped exponential backoff (base =
  `poll_interval`), fresh `sync_marker` per connection, and a
  conversations-metadata refresh per connection. Transient events
  (`conversation.typing`) now arrive live on native. Polling stays
  the default everywhere and the only wasm path. New public API:
  `SyncTransport` (uniffi enum) + the `SyncSettings.transport` field,
  and `Client::server_supports_push_events()` (62c) — a typed
  projection of the server's `consumableNotifications` feature config
  so consumers can pick the transport (absent entry → `false`; the
  CLI warns when the websocket is requested against a server that
  reports the feature disabled).
  CLI: `sync-stream --transport websocket|polling`. Reverses the
  Step-5d WebSocket drop (finding §30) against the frozen
  backend-spec §11.2–§11.5 contract — clean rebuild, no Step-5d code
  revival; NG-14 narrows to the browser target. (Step 62)
  **Availability caveat (2026-08-18):** wire-server hard-codes
  `consumableNotifications: false` in its chart templates since 5.30
  (PR #5116 hard-codes the flag; no public timeline), so on every current
  deployment the socket connects, marks and acks but **receives no
  events** — `Client::server_supports_push_events()` reports `false`
  and the CLI warns. Keep `SyncTransport::Polling` (the default) until
  Wire re-enables the feature or the legacy `/await` transport lands
  (observations §68).

- **Key-package pool replenishment** — the MLS key-package pool
  (100 uploaded at device registration, one claimed per peer invite)
  now self-heals instead of silently depleting: the sync surface
  checks the server-side count at most once per 24 h and tops the
  pool back up to 100 whenever it falls below 50; a failing check
  warns and never breaks sync. New API:
  `Client::key_package_count()` and
  `Client::replenish_key_packages()` (both FFI-mirrored). CLI:
  `key-packages [--replenish]`. Storage: V24 adds the throttle stamp
  to `sync_state`. Closes the long-standing gap-B.7 ("pool depletes →
  nobody can invite this device anymore" — the failure used to
  surface weeks later at the peer, not at the affected device).
  (Step 63)

- **Tasks/todos over MLS** (ADR-0014) — E2EE, event-sourced task
  management inside conversations; the server learns nothing. Every
  task message carries the full state of one task (`TaskPayload` in
  `GenericMessage` field 262000, outside the content oneof); receivers
  fold per `task_id`, last writer wins, unknown ids upsert. Two
  encoding modes: normal chats also send a human-readable `Text`
  fallback official Wire clients render (incl. assignee mention);
  dedicated task-list rooms send the bare payload official clients
  ignore entirely (verified: no render, no unread badge, no push).
  New API: `TaskDraft` / `TaskItem` / `TaskPatch` / `FoldedTask` /
  `RoomTask`, `Timeline::send_task` / `update_task` / `move_task`
  (fractional-index manual ordering — a move touches exactly one
  task), `Room::tasks()`, and the cross-room views
  `Client::my_tasks()` / `Client::delegated_tasks()`, backed by a
  materialized `tasks` table (schema V23, fold-on-write, index-backed
  reads) sized for large histories. FFI-mirrored for Kotlin / Swift /
  WASM (IndexedDB task-view storage lands with the WASM rework). CLI:
  `task add` / `list [--mine|--delegated]` / `done` / `assign` /
  `edit` / `move`. (Step 60, ADR-0014)

- `Client::find_message_by_id(message_id) -> Option<TimelineItem>` —
  a local-only, by-id lookup of a single timeline item (matches a
  stable `message_id` or an edit-event id), for resolving a reply's
  quoted parent without scanning the whole room. The returned item
  carries no overlay data (reactions / read receipts); use
  `Room::messages` for a fully hydrated read. (Step 34, PR #28)

- `Timeline::send_file_stream(source, filename, mime_type)` — send a
  file from a caller-supplied byte stream (`AssetSource` callback)
  instead of a filesystem path, so platforms without a real path
  (e.g. Android's `content://` picker URIs) can send without a
  temp-file copy of the plaintext. Also fixes the existing
  path-based `send_file`'s encrypted-staging directory to use the
  client's own data directory instead of `dirs::cache_dir()`, which
  returns `None` on Android and made every Android file-send fail.
  (Step 36, PR #31)

- `kupferdraht-cli login` / `register` / `activate` take `--api-url`
  (plus optional `--ws-url`) to target any self-hosted
  Wire-compatible backend; the `--backend staging|production` presets
  that existed alongside were removed before release (see Removed).
  The WebSocket URL is
  derived from the API URL by Wire's `nginz-https` → `nginz-ssl`
  naming convention when not given explicitly. (Step 43)

- Browser (`wasm32`) target bring-up: `kupferdraht-core` now builds and
  links for `wasm32-unknown-unknown`, backed by a real
  `IndexedDB`-based `Store`/`GlobalStore`, a persistent
  `WebCrypto`+`IndexedDB` keystore, and `IndexedDB`-snapshotted MLS
  (openmls) state — so a browser session's app data, keys, and MLS
  groups all survive a page reload. (Steps 44, 45, 46, 47, PR #44)

- Browser login: browser HTTP transport is backed by the Fetch API
  (`reqwest`'s wasm backend) with a wall-clock shim for `wasm32`
  (`SystemTime::now()` panics there); a hand-written
  `wasm-bindgen` binding layer (`kupferdraht-core/src/wasm/`) exposes
  `Kupferdraht.open` / `login` / `resume` / `accounts`,
  `PendingLogin.registerDevice`, and `Client`; and the Wire `zuid`
  refresh cookie is carried through the browser's own cookie jar
  (`fetch(credentials: 'include')`) rather than read/written by
  Kupferdraht directly, since JS/wasm cannot access `Set-Cookie`.
  (Steps 48, 49, 50, PR #44)

- Browser send/receive/sync: the `wasm-bindgen` `Client` gains
  `syncOnce`, room reads, text send/reply/reactions, edit/redact/
  mark-as-read, room create/DM/members, and a
  `startSync(onMessage, onTyping?, onExpired?)` loop that polls
  `sync_once` on a backoff timer and dispatches decrypted messages
  and typing events to JS callbacks (a polling loop over the
  existing notifications drain, not a WebSocket push). MLS state is
  flushed to the `IndexedDB` snapshot after every mutating call, so
  a reload keeps joined groups and device identity; `onExpired`
  fires once when the sync loop hits a terminal `SessionExpired`.
  (Steps 51, 52, 53, 54, 55, 57, PR #44)

- Browser assets: an in-memory, buffer-based asset pipeline for the
  browser — `Client.sendFile` / `downloadAsset` encrypt/decrypt and
  upload/download a file wholly in memory (no filesystem, no
  streaming), using the same wire format as the native asset
  pipeline; also adds the remaining membership/session bindings
  (`invite`, `kick`, `leave`, `logout`). (Step 56, PR #44)

- Worker-capable browser keystore: the browser keystore now resolves
  `WebCrypto` via `js_sys::global()` instead of `window()`, so it
  also works inside a Web Worker (not just the main thread) — the
  last blocker to running the wasm engine off the main thread.
  (Step 58, PR #44)

- Interactive, SP-driven SSO login (option β): Kupferdraht drives Wire's
  `/sso/initiate-login` and `/sso/finalize-login` endpoints
  directly. `ClientBuilder::initiate_sso` /
  `::initiate_sso_by_email` / `::finalize_sso` (and the matching
  `Kupferdraht` manager methods) return the identity-provider redirect
  URL and complete login from the returned `zuid` cookie;
  `kupferdraht-cli sso initiate` / `sso finalize` exercise the same flow.
  (Step 17, PR #45)

### Changed

- **BREAKING: `FileSharingPolicy` is `Disabled | Enabled`** — the
  `EnabledSome(Vec<String>)` variant and the `config.allowedTypes` field
  it parsed are gone: Wire's `fileSharing` feature carries only a
  `status`, so the variant could never occur against a real backend. The
  wasm `fileSharing()` view is `{policy: "disabled" | "enabled"}` without
  `allowedExtensions`, the Kotlin and Swift enum loses a case, and the
  CLI `send-file` gate is two-way (ADR-0012 revision 1.3).

- **CLI: `sso initiate` redirect defaults** are `kupferdraht://sso/done`
  and `kupferdraht://sso/error` (were `…/sso-login/success` and
  `…/sso-login/failure`); pass `--success-redirect` / `--error-redirect`
  to keep the old values.

- **Provenance documentation** — `docs/wire/kalium-readings.md` indexes
  every reading of Wire's Kalium source with file and permalink, what it
  informed and what was later replaced; `docs/wire/provenance-audit-2026-10.md`
  records the structural comparison against Kalium made before
  publication; README, CLAUDE.md and NOTICE state the reading rule the
  same way, and the Protobuf schema header dates its modifications
  (GPL-3.0 §5(a)). Crate-internal names that happened to coincide with
  Kalium's were changed (`TokenRefresh`, `AssetMultipart`,
  `NewClientRequest` in the Swagger's field order); no public API is
  affected by those.

- **README and NOTICE state how Wire's work is attributed** — the
  provenance rule describes the method (written from the Swagger spec,
  RFC 9420 and live observation; Wire's source read for protocol
  observation only, each reading recorded) instead of asserting a
  result, and both files carry the attribution policy: where Wire's
  work is incorporated its notice is kept, and a missing notice is
  added on report to opensource@digits.schwarz.

- **BREAKING: `Client::wire_receipt_mode()` is now `Client::receipt_mode()`**
  — the one consumer-facing name that carried Wire terminology, against
  ADR-0009's rule for the public API; Kotlin and Swift see `receiptMode()`.
  The server property it projects, `WIRE_RECEIPT_MODE`, and the returned
  `ReceiptMode` are unchanged. ADR-0012 revision 2026-10-09.

- **BREAKING: the Kotlin bindings' coordinates move to
  `io.github.schwarzdigits`** — the Multiplatform module publishes as
  `io.github.schwarzdigits:kupferdraht-bindings` (was
  `org.kupferdraht:kupferdraht-bindings`), its Android namespace is
  `io.github.schwarzdigits.kupferdraht.bindings`, and the Android library's
  namespace is `io.github.schwarzdigits.kupferdraht` (was `org.kupferdraht`).
  Why: a reverse-DNS group asserts control of that domain, and Maven
  Central verifies it; the GitHub-backed namespace is the one Schwarz
  Digits can prove. Consumers update the dependency coordinate and any
  reference to the generated `R` class; the Kotlin package of the binding,
  `uniffi.kupferdraht`, is unchanged.

- **SECURITY.md follows the Schwarz Digits security policy** — reports go
  to opensource@digits.schwarz with coordinated disclosure within 90 days,
  as the program's policy says; the project file keeps its scope, the
  threat model as the yardstick and the note that findings in Wire's
  backend belong to Wire. **Inbound contributions are governed by the
  Schwarz Digits CLA** — open.md §65 is resolved; README, ADR-0016 and
  `rules/0016` say so instead of deferring external pull requests.

- **English throughout** — the findings of April 2026 in CHANGELOG §8–13
  and observations §3e, §7, §14 and §15 were written in German; they are
  translated, content and dates unchanged (CLAUDE.md "Language").

- **User-facing strings name the backend, not Wire** — the CLI's `about`
  text, nine help texts, two password prompts and one status line say
  "account", "backend" or "network" where they said "Wire"; the browser
  smoke pages label the field "API base URL". README gains a Trademarks
  paragraph in the License section that covers "Wire" and "Kupferdraht".

- **README's Terms-of-Use note states what the repository can vouch for**
  — Wire's published terms reserve the hosted service for Wire's own
  apps, the reader checks the current terms, Kupferdraht's supported
  target is a self-hosted wire-server; the paragraph no longer cites
  section numbers or sanctions it cannot source.

- **ConsumableNotifications' status is stated from the public record** —
  ADR-0019, observations §68, LEARNINGS, open.md and the archived Step-62
  plan say that wire-server PR #5116 hard-codes the flag to `false`, so
  clients fall back to the legacy `/await` socket; a re-enabling is
  phrased as a deployment's choice.

- **The reference docs describe a live Wire v14 backend, not Wire's hosted
  service** — api-notes names the nginz host convention instead of
  `wire.com` hosts, and backend-spec, mls-deviations, library.md, cli.md,
  LEARNINGS and the validation plans of ADR-0008/0010/0011/0012 say
  "against a live Wire v14 backend" where they said "against Wire
  production" or "staging". The archived plans, the CHANGELOG and the
  observations log keep their dated history.

- **README states the scope as a limit, not a parity claim** — "limited
  to Wire's messaging feature set"; the target is a self-hosted
  deployment of wire-server, Wire's open-source backend (API v14), in the
  README, `docs/cli.md` and the charter's smoke phase; the feature list
  names Wire's official clients as such and lost its step numbers.

- **The SQLite3MC template project is no longer named** — ADR-0021,
  ADR-0022, `rules/0021`, observations and three archived plans say "the
  sister project"; its repository, file paths and note ids were internal.
  Decisions and findings are untouched; both ADRs carry a revision line.

- **`docs/wire/asset-protocol.md` is an observation record** — same
  findings, same 33 permalinks as observation points, no Kalium code
  fragments; the page no longer presents itself as a survey answer.

- **CLI: the SSO redirect defaults use Kupferdraht's own scheme** —
  `sso initiate` defaults `--success-redirect` / `--error-redirect` to
  `kupferdraht://sso-login/success` and `…/failure` instead of the
  `wire://` scheme Wire's own clients register, so a machine with the
  official app installed no longer hands the `zuid` cookie to that app.
  Any URL still works through the flags. Not yet re-smoked against an
  IdP-enabled backend.

- **Prekey module described by the wire format** — release preparation.
  The comments of `encryption/prekeys.rs` and the text of `rules/0010`,
  ARCHITECTURE and THREAT_MODEL no longer name proteus internals, a deleted
  module or primitives the module never used (Curve25519, ciborium); the
  CBOR layout is described in RFC 8949 terms, the provenance pointer to
  `docs/wire/api-notes.md` stays. `build_prekey_bundle` returns the
  bundle directly, the encoder cannot fail. No behaviour change.

- **The Protobuf schema carries Wire's notice** — release preparation.
  `kupferdraht-core/proto/generic_message.proto` is a modified subset of
  Wire's `GenericMessage` schema (GPL-3.0-or-later); its header now keeps
  Wire's copyright and license notice and states the modifications, as
  GPL-3.0 §§ 4–5 require, instead of claiming an independent layout. The
  declarations are byte-identical to before; only comments changed. In
  the same pass the comments of the shipped core lost their dates and
  process narrative (review dates, "pre-merge", "this commit", internal
  instance names) and the rule went into CLAUDE.md ("Comments in Shipped
  Code"). The "V1" wordings followed in a second pass; the `6-II-x`
  codes are the sub-step names of the archived Step-6-II plan and stay
  as pointers. No behaviour change.

- **Prepared for publication** — the repository goes public as a single
  snapshot of its tree. Examples in the README and the CLI reference log
  in against a self-hosted Wire v14 backend, and the CLI has no default
  backend (the presets for Wire's hosted service went afterwards, see
  Removed). `docs/THIRD_PARTY_LICENSES.html` is regenerated
  and covers the shipped dependency graph only. Wire's OpenAPI document is
  fetched by `scripts/fetch-wire-swagger.sh` instead of being committed,
  and `reference/` holds no submodules any more: Kalium and
  matrix-rust-sdk are consulted online at pinned revisions, natrium is
  private. Test fixtures and example outputs carry synthetic identities;
  `scripts/smoke-baseline.sh` requires `KUPFERDRAHT_BASELINE_ROOM`
  instead of defaulting to a room. The threat model is in English,
  `SECURITY.md` says how to report, and the build script hands `protoc`
  to prost-build without `unsafe`. The charter states the independence
  claim as practised: no Wire source code in the tree, Wire's
  repositories consulted for protocol observation only, every observation
  recorded with its source.

- **BREAKING: Sodium is now Kupferdraht** — a clean cut, no migration.
  The old name was taken: the library built as `libsodium.{so,dylib,a}`,
  the file name of the NaCl library that Homebrew and many apps already
  ship, so a consumer bundling both would load one for the other. Every
  surface follows the new name — the crates `kupferdraht-core` /
  `kupferdraht-cli` / `kupferdraht-uniffi-bindgen`, the library
  `libkupferdraht.*`, the Kotlin package `uniffi.kupferdraht`, the Swift
  module `Kupferdraht` over `kupferdrahtFFI.xcframework`, the wasm-pack
  output `kupferdraht.js` with `bindings/web/kupferdraht-tabs`, the types
  `Kupferdraht` / `KupferdrahtBuilder` / `KupferdrahtError` (Kotlin
  `KupferdrahtException`), the CLI binary with its `KUPFERDRAHT_*`
  environment variables, the log target prefix `kupferdraht::` and the
  Wire device labels `Kupferdraht (<platform>)`. The storage format is
  reset with it, as no release had shipped: the HKDF labels
  (`kupferdraht/v1/…`), the file names `kupferdraht.db` / `kupferdraht.lock`,
  the home directory `~/.kupferdraht`, the keystore id prefix and keyring
  service `kupferdraht`, the browser's OPFS base directory, Web Locks lease
  and home-id entry, and the cookie label. A home, browser storage or
  keystore from before is not readable: wipe it and log in afresh (the
  device registers anew). The repository moved to `reklov/kupferdraht`
  (the old URL redirects) and is published as `SchwarzDigits/kupferdraht`.
- **BREAKING: browser — a successful export or restore costs no PIN
  attempt; recovery data version 3** — Step 124 (with natrium-pin-service
  0.2.0). The PIN service counted every evaluation, the right PIN too, so a
  user who restored a few times locked themselves out for the day. Now a
  seal (`enable`, `changePin`) and a `recover` with the right PIN give
  their attempt back with a receipt signed by a key only the seed yields
  (ECDSA P-256 — HKDF of the seed, then the OPRF's `DeriveKeyPair`); a
  wrong PIN stays counted, a failed receipt leaves the attempt counted and
  fails nothing. Recovery data carries that receipt key in its header
  (version 3, 186 bytes) — the service now keys its evaluation by it, so
  data of versions 1 and 2 is `InvalidRecoveryData` (`"unsupported"`):
  make new data (`changePin(pin, pin)`) after the update. Needs the PIN
  service 0.2.0, which in turn refuses older clients.
- **Browser: storage over sqlite-remote-vfs 0.5.0** — Step 123. When the
  storage server answers a commit with an internal error, the VFS sends the
  same commit again instead of failing the database for good; if the
  server stays unavailable, the storage reports `reconnecting` and heals
  (`ServiceUnavailable` meanwhile), as for an unreachable server. The
  storage server 0.5.0 also retries a request on a fresh database
  connection. No API change.

- **BREAKING: `join_via_link` takes no domain** — Step 122. A guest link
  carries no domain (it is the operator's account page with `key` and
  `code` in the query), so `Client::join_via_link(code, key, password)`
  (Kotlin/Swift the same, browser `joinViaLink(code, key, password?)`,
  `kupferdraht-cli join-via-link --code --key`) finds the room by the
  conversation the link names. A link the own backend does not know —
  another operator's, or a revoked one — fails with `BackendRejected(404
  no-conversation-code)` before anything is redeemed; a room missing after
  a successful redeem is `Generic`, no longer `InvalidInput`.
- **Browser: an own send wakes the sync loop** — Step 122. Since Step 118
  a pass follows a frame, and the backend sends the sender no frame for
  its own message: the message was in `messages()` at once, but no
  `onSync` told the embedder to read again until the next pass, up to a
  minute later. A successful send now wakes the loop like a frame; its
  pass's `onSync` follows within a second, in every tab. The delivery
  receipts a pass sends itself wake nothing.

- **BREAKING: browser — one remote device per user** — Step 120 (ADR-0040,
  sqlite-remote-vfs / -server 0.4.0). The storage server keeps one slot per
  user: the device whose key may open the account's databases, labelled
  with its Wire client id.
  - A new setup (`registerDevice({ newSetup: true })`) claims the slot and
    so **replaces** the account's previous device: its databases on the
    storage server are deleted and its Wire client (a `Kupferdraht (Web)` one)
    too.
  - A browser whose device was replaced gets the new error
    `DeviceRetired` from `resume`; Kupferdraht forgets its keys of the account
    there — log in again (`BREAKING` for exhaustive matching in Kotlin and
    Swift).
  - `recoveryExpected` now comes from the slot (`GET /v1/slot` beside the
    storage WebSocket, whose URL must end in `/v1/ws`).
  - Recovery data is bound to the device (format version 2, 153 bytes): the
    data of a replaced device is `InvalidRecoveryData` with the reason
    `oldDevice`, before any PIN attempt. **Recovery data of version 1 is
    `unsupported`** — make new data with `changePin(pin, pin)`.
  - `logout({ purgeDevice })` releases the slot too.

- **BREAKING: push is the native default receive transport** — Step 118
  (ADR-0033, ADR-0039). `SyncTransport::default()` — and so `SyncSettings::default()`
  — is `WebSocket` on native targets (`Polling` in the browser build, whose
  live sync has its own socket); `kupferdraht-cli sync-stream` defaults to
  `--transport websocket`, and its `--idle-backoff` is optional (unset: the
  library's default). Rust consumers that relied on the polling default set
  `transport: SyncTransport::Polling`. Kotlin and Swift name the transport
  anyway (UniFFI records carry no enum defaults) — pass `WebSocket`.

  One pause without a socket on every target: an unset `poll_interval` is
  10 s (was 5 s, Step 103) — for `Polling` (also FFI `start_sync`) and for a
  push loop whose socket is down, as in the browser. A `429` without
  `Retry-After` waits that pause too (still bounded to 1–60 s).

  The push loop no longer depends on its socket: a connect that fails
  drains anyway (before, a network that blocked WebSockets received
  nothing), the loop connects at most once per pause — `poll_interval`,
  10 s when unset; Step 103's backoff doubling from 5 s to 30 s and its 1 s
  gap after a productive connection are gone — and a legacy socket without a
  frame for a minute is renewed (a ping only fails on a dead write; a socket
  whose frames a proxy holds back answers it).

- **Sync fetches only what changed** — Step 118 (ADR-0039). A sync pass
  fetches the room list (`list-ids`, `list`, `list-users`) only when it may
  be out of date: at start, after an event Kupferdraht does not process itself (a room
  created or renamed, a member joined or left, a profile changed) or a
  Welcome, and when the queue was drained elsewhere (another client or
  process on the account) or the backend lost the cursor — then in the same
  pass, after the drain. As a safety net a list older than 5 minutes is
  refreshed too. An idle pass is one request instead of four, on every
  target and both transports; the native push loop no longer fetches the
  room list per reconnect, and sees a room created during a long connection
  at once. `SyncSummary` and the API are unchanged.

  The browser's `startSync` no longer polls beside a live push socket: it
  waits for the socket, whose close now wakes it at once to reconnect and
  drain. A socket silent for a minute is renewed and drained once — a
  browser cannot ping, so a socket that died silently (a sleeping laptop, a
  dropped NAT mapping, a proxy holding frames back) would look open
  forever. Without a socket the loop polls every 10 s (was 2.5 s) and
  retries the socket, at most once per 10 s. An idle tab makes about 160
  requests an hour instead of about 5,800.
  This closes the last of `open.md` NG-14 (browser event delivery by REST
  polling; push and typing arrived with Step 115).

- **BREAKING: new `KupferdrahtError` variants for recovery data** — Step 112a
  (ADR-0035). `WrongPin { attempts_remaining }`, `TooManyAttempts {
  retry_after_secs }` and `InvalidRecoveryData { reason }` (breaking for
  exhaustive matches in Rust, Kotlin and Swift; native never raises them).
  The recovery data itself — the account's seed sealed with a PIN through
  the PIN service — reaches the browser surface with 112b.

- **Browser: the storage heals itself after a server outage** — Step 113
  (ADR-0038). While the storage server is unreachable every operation of an
  account fails at once with `ServiceUnavailable` — a send too; nothing is
  queued — and once the server answers again the storage heals in place:
  no resume, no reload. `client.storageState` (`"online"` /
  `"reconnecting"`) and a sixth optional `startSync` callback
  `onStorage(state)` report it; the tabs layer broadcasts it as `storage`.
  A send the outage interrupted was not sent (its commit is rolled back).
  sqlite-remote-vfs moves to v0.2.0, which heals in place — and no
  longer aborts the browser module when an outage outlasts one reconnect
  attempt.

- **Browser: a reload takes its own storage lease back at once** — Step
  111d (ADR-0035). An account's databases on the storage server open with
  a VFS instance id that is stable per browser and account, so a reload, a
  crash or a leader change between tabs reopens them at once instead of
  failing as `HeldElsewhere` until the old lease expires (30 s). Another
  browser still needs the explicit takeover.

- **Browser: taking over another browser's databases is the opening call's;
  the login opens nothing; the tabs layer runs the server mode** — Step
  111c (ADR-0035). `resume(accountId, { takeOver })` and
  `registerDevice({ newSetup, takeOver })`: a database another browser
  holds throws `HeldElsewhere` until the call asks for the takeover;
  `Kupferdraht.open({ takeOver })` takes only the tab lease (and, in the local
  mode, this browser's database locks). A login in the browser opens no
  database of the account — `registerDevice` does — so a takeover never
  needs a second login. The tabs layer: `serveTab({ storage })` passes the
  storage option on, the host's `evicted` event carries the `reason`, and
  `"device"` / `"access"` reach every tab.

- **BREAKING: new `KupferdrahtError` variants for the browser's server mode** —
  Step 111b (ADR-0035). `NothingStored(String)`,
  `StorageAccessDenied(String)` and `ServiceUnavailable(String)` (breaking
  for exhaustive matches in Rust, Kotlin and Swift; native never raises
  them); the wasm-only `BrowserEviction` gains `Device` and `Access`.
  sqlite-remote-vfs moves to v0.1.1.

- **BREAKING (browser): a cross-origin isolated page is required; OPFS is
  gone** — Step 111a (ADR-0035). The storage blocks on a
  `SharedArrayBuffer`, so the app must send
  `Cross-Origin-Opener-Policy: same-origin` and
  `Cross-Origin-Embedder-Policy: require-corp`; `Kupferdraht.open` fails with a
  message naming them otherwise. Browser data of earlier builds (OPFS) is
  not migrated and left where it is. The wasm-only Rust items
  `install_browser_storage`, `take_over_browser_storage`,
  `wait_for_browser_storage_eviction` and the `BROWSER_STORAGE_*` constants
  are replaced by `open_browser_storage`, `prepare_browser_database`,
  `browser_database_vfs`, `wait_for_browser_eviction` and
  `BROWSER_HOME_ID_ENTRY`. `KupferdrahtError` gains `HeldElsewhere`,
  `StorageFull` and `Evicted` (breaking for exhaustive matches; native
  never raises them).

- **BREAKING (browser): a first setup must be asked for; errors are named**
  — Step 110 (ADR-0035). On a browser without the account's keys,
  `registerDevice()` alone now throws `RecoveryDataRequired`; a first setup
  passes `{ newSetup: true }`. Every thrown JS error's `name` is its
  `KupferdrahtError` variant (`SessionExpired`, `TooManyDevices`, …) instead of
  `"Error"`; the message is unchanged. `accounts()` lists only accounts
  whose keys the browser holds; accounts set up by earlier builds are not
  migrated — a first setup removes their keys and databases.
  `KupferdrahtError::RecoveryDataRequired(String)` is a new variant (breaking
  for exhaustive matches in Rust, Kotlin and Swift; native never raises
  it).
- **A drained page commits as a whole** — Step 109 (ADR-0037). The sync
  commits a fetched page of up to 100 notifications in one go instead of
  notification by notification: three store commits per page instead of three
  per notification. Locally that is fewer fsyncs; on remote storage, where
  every commit is a round trip, a catch-up is about forty times faster.
  - What your `EventHandler` (`on_message`, `on_room_invited`), timeline
    listeners and device listeners hear about a page now arrives **after the
    page is committed** — never for data that is rolled back afterwards, never
    twice.
  - A notification that fails to store keeps the ones before it; the sync
    call ends with the error and the next one starts at the failed
    notification.
  - A send, or any other store write, made while a page is being processed
    waits for the page: milliseconds locally, a few hundred milliseconds on
    remote storage.
  - Custom `Store` implementations: new `begin_page` / `savepoint` /
    `rollback_to_savepoint` / `commit_page` / `rollback_page` with defaults
    that group nothing, so existing stores stay correct.

- **BREAKING: the sync loops pause by default and wait out rate limits**
  — Step 103 (review finding H8). `SyncSettings::poll_interval: None`
  used to mean "no pause", so every consumer that left it unset polled
  the backend in a tight loop; it now means **5 s**
  (`Some(Duration::ZERO)` still means no pause). A `429` is now
  `KupferdrahtError::RateLimited { retry_after_secs }` (new variant — breaking
  for exhaustive matches in Rust, Kotlin and Swift; `KupferdrahtError` is not
  `#[non_exhaustive]`) instead of `BackendRejected`, the token refresh's
  `429` included:
  `sync`, `sync_with_callback`, `sync_stream`, FFI `start_sync`, the push
  loop and the browser live sync wait the backend's `Retry-After` (else
  the poll interval, bounded to 1–60 s) and carry on instead of stopping;
  `sync_once` and every other call return it. The push loop waits at
  least 1 s (or the lower poll interval) before reconnecting after a
  connection that processed frames, and backs off from 5 s after failed
  ones when the interval is unset. `sync_with_callback` / `sync_stream`
  pause through the browser-safe timer.

- **BREAKING: sends are persisted before the upload — new
  `DeliveryStatus::Sending`** — Step 97a (review blocker B7). A send
  that creates its own row wrote that row only after the upload:
  - a call cancelled during the upload (a Kotlin coroutine cancel drops
    the future; a crash) left no row and nothing to resend;
  - a write failing after a successful upload was reported as a
    failure, so the retry went out twice under a new id.

  Now text, ephemeral text, knock, location, file, task, case and
  task-list marker sends write their row as `Sending` first (with its
  `Append`) and settle it to `Submitted` / `Failed` afterwards
  (`UpdateAt`; an ephemeral send's expiry arrives there).
  - A `Sending` row whose send is no longer in flight — cancelled, or
    the process restarted — reads as `Failed` and resends under the
    same message id (`Timeline::resend`, `Room::failed_sends`).
  - An upload that succeeded is `Ok` even if its status write fails.
  - New enum variant: exhaustive `switch` / `when` in Swift and Kotlin
    and an exhaustive Rust `match` must handle it. `Sending` is the
    first variant, so the UniFFI indices of the other three shifted —
    consistent after regenerating the bindings.
    `Store::mark_send_outcome` is new (defaulted).
  - CLI `timeline` marks a send in flight with `…`.

- **BREAKING: a missing key for an existing database is
  `KupferdrahtError::KeyUnavailable`, never a new key** — Step 96
  (ADR-0029, review blocker B5). When the keystore returned no key for
  a database that already existed, Kupferdraht generated and stored a fresh
  one. That key could not open the database and overwrote the real key,
  should it come back, so a keystore that was merely locked (the iOS
  Keychain before the first unlock, a host keystore not up yet) turned
  into permanent loss of the local account. Now `global.db`, `kupferdraht.db`
  and `mls.db` refuse with the new variant and write nothing; a first
  run (no database) is unchanged. New error variant: exhaustive
  `switch` / `when` in Swift and Kotlin must handle it, and so must a
  Rust consumer matching `KupferdrahtError` exhaustively (the enum is not
  `#[non_exhaustive]`). The payload names the database, the keystore id
  and the way out (`global.db` → the home's registry file; `kupferdraht.db` /
  `mls.db` → the account directory).
  - **Host keystores:** `SecureKeyStore::get_secret` must throw while
    the store is locked or unavailable, and return `None` only for an
    id that was never stored (`docs/bindings.md`).
  - **CLI:** `kupferdraht-cli` exits with code 4 on `KeyUnavailable`.
- **`Timeline::edit` / `Timeline::redact` refuse another user's
  message** — Step 95b (ADR-0028, sending side). Editing or
  deleting-for-everyone a message whose local row was sent by another
  user now returns `InvalidInput` and uploads nothing: every receiver
  (official Wire clients and Kupferdraht since 95b) rejects it, and applying
  it locally left the caller's view diverging from everyone else's.
  Any device of the self user counts as own; an id the local store does
  not know goes out as before. Mirrored on every surface (FFI
  `Timeline.edit` / `redact`, wasm `editMessage` / `redact`, CLI `edit`
  / `delete-message`).
- **BREAKING for foreign keystores: keystore ids and the Wire login
  label are now scoped per home** — Step 93 (ADR-0025). Every id
  carries the home id read from `base/home.id` (created once under the
  Step-85 home lock; a raw file in the OPFS pool on wasm):
  `kupferdraht.<home-id>.global-db`, `kupferdraht.<home-id>.account-<user>`,
  `kupferdraht.<home-id>.account-<user>-mls`; the login label becomes
  `kupferdraht-<home-id>` and the keystore-wide `kupferdraht.install-id` retires
  (left in place, never read again). Two homes on one keystore — CLI +
  TUI on one keychain, two smoke homes on one dev-keystore directory —
  no longer share a database key, no longer race the first creation of
  an id, and no longer evict each other's refresh cookie. Existing
  homes adopt their legacy keys once (only when the database already
  exists; legacy entries stay); the label cannot be adopted — the next
  login takes the per-home label. New `Kupferdraht::home_id` /
  `Kupferdraht::cookie_label` (FFI + wasm), `SessionRecord.cookie_label`,
  and `status` prints `home_id` / `cookie_label` / `session_label`.
- **BREAKING: `TimelineEventRecord` gained `ephemeral_ttl_ms: Option<i64>`**
  — Step 79 (V28 migration `timeline_events.ephemeral_ttl_ms`, applied on
  the next open). Struct-literal constructors of the record must set the
  field (`None` for every ordinary row; `..Default::default()` works). Set
  only on own ephemeral sends — Step 89 kept a rejected ephemeral's body
  for a resend but not its timer, so a resend could only have sent the
  text as permanent; inbound rows keep `None` (their `expires_at_ms` is
  derived at dispatch). `Store` gained `mark_message_resent` with a
  default body (`Ok(false)`; the core reports a storage error when the
  row does not flip — never a silent success), so custom stores keep
  compiling. `MessageHandle` now derives `PartialEq` / `Eq`.
- **Docs: THREAT_MODEL 0.4 and charter brought to the shipped state** —
  Step 82. The threat model now describes what is built: private keys
  are software keys in `mls.db` under a keystore-held database key
  (Secure-Enclave signing is an option, not the state), attacker-
  controlled identifiers never become path fragments (Step 80), the
  Dehydrated Device is dropped (no recovery path ships today — the
  mnemonic backup, Step 12, waits on Wire endpoints), and every
  proteus reference reflects ADR-0010 (own GPL-free prekey module).
  CLAUDE.md's goal/stack lines and the Wire transport line were
  corrected the same way (polling default; WebSocket auto-negotiates
  `/v14/events` vs legacy `/await`, ADR-0019). No behaviour change.
- **Docs: the complete `GenericMessage` catalogue** — Step 83.
  `docs/wire/message-types.md` now lists every content variant Wire's
  schema declares (field-number order, including `Multipart = 27` and
  the `unknown_strategy` envelope field) with a per-variant scope
  verdict, replacing the short "reserved" list; `docs/cli.md` gains the
  never-documented `knock` and `location` sections; `observations.md`
  adopts closure markers (nine superseded findings struck through with
  a pointer, text kept); the stale `TODO(ADR-0013)` comment in
  `feature_configs.rs` now names the ciphersuite gap (open.md gap-B.1 —
  closes def-D.1).
- **BREAKING (browser): one active Kupferdraht instance per origin, and
  `Kupferdraht.open` takes an options object** — Step 92, ADR-0023 (supersedes
  ADR-0022 decision item 7). Before the OPFS pool is touched, the worker
  takes the origin-scoped Web Locks lease `kupferdraht:instance`; another
  tab's `Kupferdraht.open` now fails closed with "already open in another
  tab" instead of an OPFS handle error. `Kupferdraht.open({ takeOver: true },
  onEvicted)` steals the lease: the other tab's instance is evicted (its
  `onEvicted` fires, its `evicted` getter turns `true`, every store
  operation there fails closed) and its embedder must terminate that
  worker — the taking-over open waits up to five seconds for the OPFS
  handles. **Breaking for web consumers:** `Kupferdraht.open(permitHttp)`
  became `Kupferdraht.open({ permitHttp?, takeOver? }, onEvicted?)`. Web
  Locks floor (Chrome 69 / Firefox 96 / Safari 15.4) lies below the OPFS
  floor. No read-only second tab in V1. Pinned in a dedicated-worker
  test that plays the other tab with raw `navigator.locks` requests.
  Closes open.md gap-B.31.
- **BREAKING (browser): the wasm build persists through the same SQLite3MC
  store as native, in OPFS — and must run in a dedicated Web Worker** —
  Step 91, ADR-0022 (supersedes ADR-0015 Part 2). The IndexedDB record
  store with per-record AES-256-GCM and plaintext index keys, and the
  IndexedDB MLS snapshot, are gone: `kupferdraht.db`, `mls.db` and `global.db`
  are whole-file-encrypted SQLite3MC files in the origin's private file
  system (the `sahpool` VFS), written and read by the very code the
  native targets run — the same vendored 2.5.1 amalgamation, keying
  batch, key verification, migrations, task cache and Step-89 expiry
  sweep (the documented wasm no-ops close). MLS state is persisted
  continuously (`openmls_sqlite_storage`), not snapshotted. **Breaking
  for web consumers:** `Kupferdraht.open` fails closed outside a dedicated
  worker (OPFS sync access handles exist only there; no in-memory
  fallback); one tab per account (no cross-tab coordination); browser
  floor Chrome 102 / Firefox 111 / Safari 15.2; building the artifact
  needs a wasm-capable clang. **Browser data written before this change
  is not migrated** (test-only deployment) — a fresh origin storage.
  New on the wasm surface: `Client.storageEngine()`; the wasm-only
  `KupferdrahtProvider` re-export is retired. Engine and at-rest layout are
  pinned in the browser (`scripts/wasm-test.sh`, headless Firefox): a
  file written natively opens in the browser and vice versa.
- **BREAKING: whole-DB encryption engine is SQLite3MC (ChaCha20-Poly1305);
  the database key is a raw 32-byte key** — Step 90, ADR-0021 (supersedes
  ADR-0013). Every local database (`kupferdraht.db`, `mls.db`, `global.db`) is
  now encrypted by SQLite3 Multiple Ciphers 2.5.1 in its `chacha20`
  scheme — ChaCha20-Poly1305, a 16-byte Poly1305 tag per page, a raw
  32-byte key as the whole security parameter (no KDF, no salt, no HMAC
  subkey in the trust path), the 32-byte plaintext header for iOS kept.
  SQLCipher (Zetetic) and its vendored OpenSSL leave the tree; the engine
  is compiled from the vendored amalgamation (`vendor/sqlite3mc/`, pinned
  by version + SHA-256 against the release's signed `SHA256SUMS`) through
  a `libsqlite3-sys` stand-in the root `[patch.crates-io]` points at — no
  OpenSSL, system SQLite or pkg-config in any build. A wrong key now fails
  **at open** (the key is verified by a first read) as a recoverable
  `KupferdrahtError::Storage`. **Breaking for consumers:** `DbKey` is the key
  alone — `DbKey::salt()`, `SALT_LEN` are gone, `MATERIAL_LEN` is 32, and
  the blob a `SecureKeyStore` stores shrinks from 48 to 32 bytes, and a
  48-byte blob written by an earlier build is **rejected**
  (`KupferdrahtError::Generic`) — no transition path, consumers re-provision
  (decided 2026-09-24: Kupferdraht is deployed nowhere). **Existing databases
  are not migrated** (test-only deployment; SQLCipher-format files are unreadable
  by design) — start from a fresh home. New: `Store::storage_engine()` /
  `Client::storage_engine()` return a `StorageEngineInfo` (engine,
  version, cipher scheme, header mode) read off the live connection, the
  `kupferdraht-cli status` `storage:` line. Accepted, visible deviation:
  ChaCha20-Poly1305 is not on BSI TR-02102-1's lists (THREAT_MODEL G3);
  SQLite3MC's AES-based `sqlcipher` scheme stays compiled in as the
  documented fallback, not a consumer knob. Regression pins from the
  Step-88 harness live in `tests/step_90_sqlite3mc_engine.rs`; the
  Step-76 `keyed.db` fixture was converted once in place (`PRAGMA rekey`).

- **Whole-DB encryption engine decided: SQLite3MC
  (ChaCha20-Poly1305)** — Step 88, ADR-0021 (supersedes ADR-0013 /
  SQLCipher). Verification step only: byte-level pins for the
  plaintext-header + external-salt pattern, `PRAGMA key` timing,
  cipher review, and a measured 70/70-green integration patch live
  in `spike/sqlite3mc/`; shipped behaviour is unchanged until the
  swap lands (Step 90).

- **`SyncTransport::WebSocket` now means "push, auto-negotiated"
  (ADR-0019)** — Step 73. Each connection asks the server's
  `consumableNotifications` feature: enabled → the `/v14/events`
  ConsumableNotifications socket (unchanged); disabled — the state
  of every current wire-server, which hard-disables the feature
  upstream — → Wire's legacy version-less `/await` socket
  (drain-then-stream, Bearer-header auth, durable frames advance
  the sync cursor, `transient` typing dispatches live without
  touching it, client keepalive pings). Consumers just opt into
  `WebSocket` and get live push either way; `Polling` stays the
  default. First live-verified push on a current wire-server
  (2026-08-21), including transient typing receive — previously
  impossible on polling.

- **BREAKING: license changed from Apache-2.0 to `GPL-3.0-only`**
  (ADR-0016) — Kupferdraht's own code is now copyleft. Resolves the
  license decision ADR-0010 had deferred. The dependency graph is
  unaffected (all permissive; `deny.toml` still blocks any copyleft
  pull into it, so the GPL posture stays a free choice), but
  downstream embedders now inherit copyleft obligations from Kupferdraht
  itself.

- Internal decomposition of `timeline/task.rs` (~1000 code lines
  after Step 60c) into the `timeline/task/` module folder —
  `types` / `fold` / `order` / `views` / `cache` / `encode` / `send`,
  one responsibility per file. No API, FFI, or wire-protocol change.

- `kupferdraht-cli` renders every timestamp as RFC 3339 in the local
  offset (seconds precision) instead of raw Unix milliseconds —
  `timeline` item prefixes, `task list` `due=`/`updated=`, all send
  echoes (`sent_at:`), `mark-read` / reaction / redact / edit stamps,
  and `config`'s `last_synced_at`. `task add` / `task edit` take
  `--due <ISO 8601>` (bare date = end of that local day;
  offset-free datetime = local time); `--due-ms` stays as a raw
  escape hatch. Wire format, storage, and the library API keep
  epoch-ms — presentation change only. (Step 60)

- Internal decomposition of the three largest library files — no
  API, FFI, or wire-protocol change: `client/runtime.rs` (Step 29,
  PR #22), `timeline/send.rs` (Step 30, PR #20), `sync/dispatch.rs`
  (Step 32, PR #27). Recorded late: the steps merged 2026-06-16/17
  but never left `PLAN.md`; this entry lands with the housekeeping
  that archives them.

- Internal decomposition of `kupferdraht-cli/src/main.rs` by command
  domain into a `cli/` folder (Step 33, PR #42) — no CLI behaviour,
  API, or wire-protocol change; `main.rs` keeps only the clap types
  and the dispatch.

- Extracted the gobley/UniFFI Kotlin-Multiplatform binding
  generation out of natrium-core into a standalone Gradle module in
  this repo (`bindings/kmp`), consumed by natrium via a composite
  build — no Rust, FFI, or Wire-protocol change; the generated
  `uniffi.kupferdraht` API is unchanged, only where it is generated
  moves. (Step 42, PR #35)

- Unified the two near-duplicate `mls.db`-open call sites behind one
  crate-internal seam (`open_mls_provider`) — no public API, FFI, or
  Wire-protocol change. The single `cfg` chokepoint the browser
  `IndexedDB` MLS backend later plugs into. (Step 61, renamed from
  43 on the wasm branch; PR #44)

- Narrowed the wasm-target clippy lint relaxations to the one
  permanent case (`arc_with_non_send_sync`, needed for an `Arc` of
  `!Send` JS state on a single-threaded target); the other four
  lints (`dead_code`, `unused_async`, `unnecessary_wraps`,
  `needless_pass_by_value`) moved to targeted per-site allows so the
  wasm build is back at per-lint `deny`. Internal cleanup, no
  behaviour change. (Step 59, PR #44)

- **ADR-0015** records the browser architecture (WASM-only delivery,
  native IndexedDB object stores with per-record AES-256-GCM and
  deliberately plaintext key/index fields, hand-written wasm-bindgen
  surface parallel to `ffi/`) and **supersedes ADR-0005** in full —
  the WASM-only core decision is carried forward unchanged; the
  superseded implementation notes (sql.js blob, UniFFI-TS bindings)
  were never built. Imports natrium's D-storage-1 decision into the
  repo; confirmed Volker/Sven 2026-07-17; resolves the WASM part of
  `open.md` §61. Same-commit rule file:
  `rules/0015-browser-architecture-wasm-indexeddb-bindgen.md`; the
  missing `docs/adrs/_template.md` + index `README.md` were also
  restored during this catch-up.
  (`wrap_dek` / `unwrap_dek`, wrapped blob on disk) to a plain secret
  store (`get_secret` / `set_secret` / `delete_secret`) — the 48-byte
  DB key material lives directly in the platform keystore; no KEK, no
  `dek.bin`. Shipped with the whole-DB SQLCipher move (PR #4);
  recorded late — resolves the former `open.md` §53 proposal, which
  this change implemented verbatim.

### Deprecated

### Removed

- **The CLI's `-b` / `--backend production|staging` presets** — `--api-url`
  is required on `login`, `register`, `activate`, `sso initiate` and
  `sso finalize`. Wire's Terms of Use restrict third-party clients on its
  hosted service, so Kupferdraht no longer offers that service by name;
  `backend.rs` keeps the URL validation and the nginz-convention WebSocket
  derivation. `scripts/fetch-wire-swagger.sh` takes the backend from
  `KUPFERDRAHT_SWAGGER_URL` instead of defaulting to Wire's staging host.
  **BREAKING** for scripts that passed `-b`.

- **The second receipt interop vector** — its signed message named a
  development instance's hostname, and a domain swap would turn the
  interop check into a self-test. `recovery/receipt.rs` keeps the first
  vector until the PIN service issues a second one with a neutral domain
  (open.md §77).

- **The openmls 0.8.1 storage-format pin, its fixtures and the
  `0-8-1-storage-format` feature** — release preparation. The two fixture
  databases under `tests/fixtures/mls-081/` bound a then-live device
  identity of the author to its persisted signature key, and ADR-0041
  left no 0.8.1-format `mls.db` behind that the pin would protect. For
  Kupferdraht's JSON codec both feature states wrote identical bytes
  (observations §73), so a database written with the feature on reads
  unchanged. A test pins that the feature stays off. Resolves open.md §76.

### Fixed

- **Browser: a login that meets a replaced device resets the browser** —
  Step 121 (ADR-0040). Only `resume` forgot the account's keys when the
  storage slot refused the open; a login kept them, so every further login
  met `DeviceRetired` again and nothing an embedder could call cleared the
  browser (`resume` itself failed first with `SessionExpired`, the
  replaced Wire client being gone). `registerDevice` now forgets the keys
  as it reports `DeviceRetired`, and the next login offers recovery (`keys`
  `"missing"`).

- **`join_via_link` joined the wrong room** — Step 122. After the redeem
  it took the first cached room of the given domain: with any other room
  of that domain cached it returned that room (and joined it again by an
  external commit) and left the link's room without an MLS join. It now
  takes the room the link names, and a room whose group is here already
  is not joined again.
- **Browser types: `InvalidRecoveryData`'s `reason` includes
  `"oldDevice"`** (Step 120's reason was missing from kupferdraht-tabs'
  `index.d.ts`).

- **Browser: a takeover by another browser is noticed at once** — Step 119
  (sqlite-remote-vfs 0.3.0). An idle browser whose databases another
  browser of the account took over learned it only at its next write (since
  Step 118 up to minutes), and that write failed as `Storage: … disk I/O
  error`. Now the storage reports the takeover as an event:
  `onEvicted("device")` fires at once, an operation that met the takeover
  fails as `Evicted`, and the evicted instance stops its sync loop and push
  socket — it would otherwise close the new browser's socket (both are the
  account's device). sqlite-remote-vfs 0.3.0 also keeps idle browser
  connections alive and no longer keeps a revoked lease revoked.

- **kupferdraht-tabs: an evicted leader's late message no longer pulls the tabs
  back** — Step 116. After a takeover, the old leader's last heartbeat or
  event could arrive after the new leader's announcement; every tab — the
  new leader's own included (`state` `thin`) — then followed the dead
  leader for up to one heartbeat (2 s), and calls in that window waited or
  failed with `LeaderChanged`. A message of an older leader (lower epoch)
  now moves no view; its event is still delivered.

- **Only the backend's own "cursor not found" resets the sync cursor** —
  Step 102 (review finding H3). The notifications drain dropped its
  cursor and re-read the queue from its head on any `404`, including a
  proxy's or load balancer's error page. It now resets only on the
  backend's `404` with label `not-found` to a request with `since`; any
  other `404` fails the sync with `BackendRejected` and keeps the cursor.
- **An event is written in one piece — no more text without its
  mentions after a failure** — Step 105 (residual of review finding B9).
  A text and its mentions, an asset and its attachment, an edit and its
  new mentions, a task and its materialized state were two separate
  `kupferdraht.db` writes. A failure between them left the first half, and
  the replayed notification skipped the event as already stored: the
  mentions, the attachment (the asset could not be downloaded), the
  `on_message` callback or the task's new state were lost for good. Each
  pair is now one transaction, inbound and outbound. New `Store` methods
  `insert_timeline_event_with_mentions`, `…_with_attachment`,
  `…_with_task` and `apply_message_edit_with_mentions` (default bodies
  keep custom stores compiling — override them to be atomic). Also fixed:
  an edit of an own edit lost its mentions and its timeline update (the
  row's id was resolved after the edit had moved it on).

- **A crash or disk error between decrypting and storing an inbound
  message no longer loses it** — Step 97e (review finding B9,
  ADR-0032). Decrypting deletes the message key; the content reached
  `kupferdraht.db` only afterwards, so a failure in between left a message
  that could never be decrypted again. Each inbound notification now
  runs in one `mls.db` transaction that commits only after its data is
  stored; the cursor advances after that. A replay after a crash is
  processed again or, where MLS already committed, skipped quietly —
  no loss, no duplicate callback. **Behaviour change:** a failure to
  store an inbound message's data now ends the sync call with
  `KupferdrahtError::Storage` (retryable, the notification stays queued)
  instead of `on_sync_error` + skip. A send waits while a notification
  is being processed (milliseconds).
- **A second `Client` or process on the account, or an own commit
  being uploaded, no longer makes Kupferdraht rejoin a room** — Step 97d
  (review blocker B8, ADR-0031). The out-of-order healing (discard the
  group, rejoin by external commit — and lose the message that
  triggered it) fired in three situations without any real gap: two
  `Client`s or processes on one account draining the same queue at once
  (the second one met a commit it had already processed); a message
  from the next epoch arriving while an own commit (invite, kick, room
  or DM setup, removal flush) was still uploading; and an old commit
  delivered again. Now one sync dispatch runs per account at a time —
  pages are fetched without a lock and dispatched under
  `accounts/<id>/sync.lock`, refetched if another `Client` moved the
  cursor; a next-epoch message waits for the own commit's merge (or,
  if another `Client` owns the commit, waits for a later sync; after
  60 s the commit counts as abandoned and the old healing applies);
  and an older-epoch message is skipped. Network work an event triggers
  (healing, 1:1 delivery receipts, config re-fetches) runs after the
  lock. A sync call can now return the retryable `AccountLocked` when
  another holder kept the lock for more than 5 s.
- **Two processes on one account no longer fail a write with
  `database is locked`** — found by the Step 97d smoke. Every writing
  transaction of `kupferdraht.db` started `DEFERRED`; one that reads before
  it writes (e.g. the ephemeral expiry sweep) failed at once, instead of
  waiting out `busy_timeout`, when another process committed while it
  waited. They all start `IMMEDIATE` now.
- **Large files no longer fail after 30 seconds** — Step 98 (review
  finding H1). The 30 s end-to-end timeout for API calls also applied to
  asset up- and downloads, so a transfer failed after 30 s even while
  its bytes kept flowing (a 20 MB file at 5 Mbit/s). Transfers now run
  without an overall deadline: a download fails only after 30 s without
  a byte, an upload only when its connection fails (or the app cancels
  the send). API calls keep 10 s to connect and 30 s end to end.
  Native only; the browser build never had these timeouts.
- **One unknown conversation no longer stops the whole sync** — Step
  99 (review finding H2). The metadata sync — the first phase of every
  sync — decoded the conversation list strictly: a single conversation
  of a kind Kupferdraht does not know (a new `type` or `protocol` on the
  backend) failed the whole list, and with it every sync: no room, no
  message. The user list for display names had the same weakness (one
  profile without a name). Both lists now decode entry by entry; an
  entry that does not decode is logged and skipped.
- **A browser tab that loses the leadership to another tab now reports
  `thin`** — Step 100. When another tab took over, the evicted tab
  could receive the new leader's announcement before its own worker's
  eviction notice; the host then set its state back to `electing` and
  never corrected it (`host.state` stayed `electing` while the tab
  followed the new leader). The race also made a Step-94 browser test
  fail intermittently in CI.
- **Browser: a new database key is stored before its database exists** —
  Step 104 (review finding H7, part 1). The browser keystore persisted
  keys to IndexedDB in the background and dropped the result; the
  database was created with the key at once. A failed write (quota,
  private mode) or a tab closed too early left a database whose key was
  gone after a reload — `KeyUnavailable`, the account's chats, MLS state
  and device lost. Keystore writes are now tracked; `Kupferdraht.open`, login,
  SSO finalize and resume create the missing keys first and wait until
  IndexedDB has them — a refused write fails the call before any database
  exists, and is logged when it fails. A key the keystore could not
  store is forgotten, so a retry stores a fresh one.
- **A resend in flight is not a failed send; the id lookup settles
  abandoned sends** — Step 107 (review of Step 97a). While a resend's
  upload ran, `Room::failed_sends` still listed the row, so a second
  resend could upload it again; now it is left out and a concurrent
  resend of it is refused. `find_message_by_id` returned the raw
  `Sending` of an abandoned send; it now reads `Failed` like every other
  reader. A send whose status the store did not record (a custom store
  without `mark_send_outcome`) now logs a warning. The send-lifecycle
  contract (`library.md`) spells out that "in flight" is per `Client`,
  that a cancelled send emits no settling diff, and that an ephemeral
  timer counts the upload time.

- **A crash or error in the middle of an MLS operation no longer
  leaves a group half-updated** — Step 97c (review blocker B8, plan
  D5). openmls wrote `mls.db` one record per statement (a commit merge
  about a dozen, the pending commit gone after the first): a failure in
  between left tree, context and secrets at different epochs, and a
  Welcome whose key package was already deleted could never be retried.
  Every MLS operation now runs in one SQLite transaction — all its
  writes land or none: a failed merge keeps the old epoch and its
  pending commit, a failed Welcome keeps its key package. Single MLS
  operations are also serialised across processes and `Client`s on one
  account (the write lock, `busy_timeout`), and each costs one commit
  instead of up to 13. No API change.
- **Live push no longer loses messages on current wire-servers** —
  Step 97b (review blocker B6, ADR-0030). On the legacy `/await` socket
  (what `SyncTransport::WebSocket` negotiates on every current
  wire-server) Kupferdraht drained the REST queue before connecting and let
  every socket frame move the sync cursor: an event stored between the
  drain and the socket registration was never delivered, and neither
  was a push the socket missed while connected. The socket is now only
  a doorbell: connect first, then drain; every durable frame triggers a
  drain (frames during a running drain coalesce into one follow-up),
  and only the drain dispatches and advances the cursor. Transient
  frames (typing) still dispatch directly. One `GET /notifications` per
  burst of events; `/v14/events` and polling are unchanged. On `/await`
  the `sync_with_callback` summary now ticks once per drain or
  transient frame (before: once per frame).
- **Accounts in more than 1000 conversations sync again** — Step 95c.
  All conversation ids went into one `POST /conversations/list`, which
  Wire caps at 1000; the metadata sync failed on every cycle. The ids
  are now fetched in chunks.
- **A panic in the background sync no longer stops it silently** —
  Step 95c. `Client.start_sync` (FFI) reports it as
  `on_error("sync stopped after an internal panic: …")` and stops; a
  lock poisoned by the panic recovers instead of failing every later
  MLS operation of the session. On wasm a panic still traps the module.

- **Two processes on one home can no longer kill each other's session
  or orphan a fresh database** — Step 85 (ADR-0024). A per-account
  advisory file lock (`accounts/<id>/kupferdraht.lock`, plus `kupferdraht.lock`
  per home for the global-db key) guards exactly two critical sections:
  token refresh + persist (a concurrent rotation by another process is
  now *adopted* from the re-read session instead of rebound — exactly
  one `/access`) and first-time key creation (`load_or_create_key` is
  atomic under the lock; an existing key keeps the lock-free fast
  path). The lock is fail-fast and never machine-wide: two homes, two
  users, two processes keep working unchanged (pinned by a test). New
  typed, retryable `KupferdrahtError::AccountLocked` (FFI mirrored;
  `kupferdraht-cli` exits 3 naming the holder pid). Every native connection
  now sets `busy_timeout = 5000` and `synchronous = FULL` explicitly.
  Browser: unchanged — ADR-0023's Web Locks lease covers the same
  invariant there.
- **Task-cache backfill read the wrong column count** — found in Step 79.
  The V23 backfill query (`load_task_timeline_rows`, run once by
  `Client::load_rooms_from_store` on a home whose `tasks` table is empty
  but whose timeline holds task rows) had not grown with Step 89's
  `expires_at_ms` column while sharing the positional row reader — every
  such backfill would have failed with an invalid column index. The
  column list now matches the reader and a unit test pins the query.
- **The dev file keystore is loud and crash-safe** — Step 77. The
  `KUPFERDRAHT_DEV_KEYSTORE` escape hatch (plaintext key files instead of
  the OS keyring) now emits one `WARN` naming the base directory on
  every activation — including `kupferdraht-cli`'s own path, which the
  original plan had missed; key files are written atomically
  (`<id>.key.tmp` created 0600 → fsync → rename → directory fsync), so
  a crash mid-write can no longer destroy the key of an encrypted
  database; a stale `.tmp` is removed unread; and the base directory is
  created 0700 as the docs always promised. No API change.
- **The wasm target compiles again** — Step 91 (open.md gap-B.28).
  `cargo check --target wasm32-unknown-unknown` had been red since
  Step 75: the browser binding's timeline projection lacked the
  `TaskListMarker` arm. Nothing in CI built wasm32; it now does
  (compile check + clippy + the browser tests in headless Firefox).
- **A crash during first open / schema upgrade can no longer brick
  the database** — Step 87. Schema migrations ran with migration
  SQL and history bookkeeping in separate commits; a crash between
  them re-applied the migration on the next open, and 25 of 27
  migrations fail on re-apply — a permanently un-openable
  kupferdraht.db. Migrations now run grouped (one transaction per run):
  an interrupted run leaves no partial state and the next open
  retries cleanly. Fault-injection tests pin the behaviour.
  Residual: the openmls-managed `mls.db` keeps the (smaller,
  recoverable) window until upstream `openmls_sqlite_storage`
  groups its runner too.

- **CI advisory job enforces the real gate** — the `cargo audit`
  step ran with a blanket `continue-on-error` (it scanned the raw
  lockfile, flagging crates no target compiles, so it was
  permanently red and muted — and would equally have swallowed a
  genuine advisory
  in shipped code). CI now runs `cargo deny check`, the same tool
  and `deny.toml` policy as the local pre-commit gate, and fails
  the build on a hit.

- **External-removal commits are batched per dispatch cycle** —
  Step 74 (gap-B.25). Receiving a backlog of backend-emitted Remove
  proposals (e.g. after device deletions) no longer produces one
  commit attempt per proposal: the guaranteed
  `mls-commit-missing-references` rejections and their
  `on_sync_error` spam are gone — one commit per room covers the
  whole backlog, a failed flush stays queued and retries on the next
  cycle, and the expected mid-backlog rejection on the push path is
  silent. Live-verified A/B on the same 2-proposal-per-room load:
  3 commits / 0 rejections vs the prior 6 / 4.

- **wasm32 compiles again** (Step 72b side effect, closes gap-B.25):
  `wasm/manager.rs`'s timeline view had missed the `Task` match arm
  since Step 60, breaking `cargo check --target
  wasm32-unknown-unknown` on main; 72b added the `Task` and `Case`
  arms (bare kind markers — richer JS projections stay with the
  in-flight WASM rework).

- **Queued backend external Removes survive user commits** (Step 71,
  closes review gap-B.20): the pre-commit proposal hygiene in the
  invite/kick path is now selective instead of a blanket
  `clear_pending_proposals`. An external-sender Remove targeting a
  peer leaf stays queued and rides the user's Add/Remove commit
  (one bundle, one epoch advance — RFC 9420 §12.4), so the Step-28
  removal obligation can no longer be silently dropped when an
  invite or kick races the sync drain's auto-commit. Removes
  targeting the own leaf are still dropped (`CannotRemoveSelf`
  guard, live-confirmed behavior preserved and pinned on both
  paths), as are non-Remove proposal classes (commit stays atomic
  in the caller-requested set). Also fixes the original
  stale-device case: a Remove of a prior device leaf of the same
  user is a peer Remove and now gets committed. A kick of a leaf an
  external Remove also targets builds fine (openmls dedups per-leaf
  Removes). (Step 71)

- **Welcome absorption — the receiver reads the initiator's first
  message** (Step 69, closes gap-B.24): the sync drain now processes
  `conversation.mls-welcome` instead of logging + skipping it. A
  claimed device joins the group at the Welcome's epoch — before the
  initiator's first application message — where it previously joined
  later via heal/`rooms-join-all` and lost every pre-join message to
  MLS forward secrecy. `EventHandler::on_room_invited` (declared in
  Step 5c, never fired until now) fires after the group is joined
  and persisted, so consumers can refresh their room list. A group
  the device already joined (external commit won the race) and a
  redelivered Welcome (one-time key-package secret consumed) are
  skips, not errors; a malformed Welcome warns, continues the drain,
  and cannot trigger room-recovery churn. Live-verified: the
  pre-sync first message is read. (Step 69)

- **`Client::create_dm` establishes the MLS 1:1** (Step 68): Wire's
  one2one fetch only *reserves* the conversation — the server keeps
  it out of `list-ids` until a member uploads the establishing
  commit, so the previous mirror-locally-only flow dead-ended in
  "not in cache after sync" and the DM never materialized for either
  side. `create_dm` now branches on the fetched state: local group
  present → resume (idempotent); `epoch == 0` → establish (claim the
  peer's key packages, Add-commit, bundle upload — any failure
  discards the local group so a retry starts clean); `epoch > 0` →
  external-commit join. Live-verified: the 1:1 appears in `list-ids`
  immediately after establishment and messages round-trip both
  directions. Known residual (gap-B.24): a Kupferdraht *receiver* still
  joins via heal/`rooms-join-all` instead of absorbing the Welcome,
  losing the initiator's pre-join messages to MLS forward secrecy.
  (Step 68)

- **Join consistency** (Step 66, review triage S2/S16 — `open.md`
  §66): a failed commit-bundle upload during an external-commit join
  now discards the just-merged local group, so `join_all_rooms`
  retries the room instead of skipping a group that sits one epoch
  ahead of the server behind `already_joined` (openmls has no staged
  external join — the upstream wish is tracked as gap-B.22; the
  residual crash window between finalize and discard is covered by
  Step 64's tolerant `resync_room`). The post-join epoch is now read
  from the openmls group (`MlsClient::group_epoch`) instead of
  `room.epoch() + 1` from a possibly stale rooms cache, keeping
  stale-detection reliable without a prior `sync_metadata`.

- **Timeline correctness** (Step 65, review triage S4/S5/S6/S10/S12 —
  `open.md` §66): `send_reply` now hard-fails with `InvalidInput`
  when the quoted parent is not stored locally (previously it
  silently sent plain text that only Kupferdraht rendered as a reply);
  live `TimelineDiff`s no longer degrade assets to Text (the
  asset-kind reconstruction is shared by `Client::messages`, the
  Append path, and `emit_message_update_at`); the Append diff carries
  the message's mentions (subscribers used to see them only after a
  reload); `on_message` fires only for freshly-inserted rows
  (duplicate binding callbacks on redeliveries are gone); and the
  Wire echo of an own message finally upgrades `Submitted` →
  `Received` (`Store::mark_delivery_received`, new trait method with
  a no-op default) and emits an `UpdateAt` — group sends no longer
  show "pending" forever.

- **A clearing `zuid` Set-Cookie no longer destroys the session**
  (kupferdraht-tui bug report, 2026-08-13): `extract_zuid` used to capture
  a deletion cookie (`zuid=; Expires=1970…`) as an empty string and
  overwrite the valid persisted refresh token with it; the next token
  refresh then tripped a debug assert in consumer debug builds.
  Deletion cookies are now filtered (the old token is kept; a genuine
  revocation surfaces as the clean 403 → `SessionExpired` on the next
  call), and an empty persisted refresh token returns
  `SessionExpired` — the documented re-login signal — instead of
  hitting an assert: persisted state is consumer-reachable input, not
  a test-harness invariant.

- **Sync robustness** (Step 64, from the 2026-08-13 review triage —
  `open.md` §66 S1/S7/S8/S13): the notifications drain now skips a
  redelivered cursor frame instead of re-dispatching it (mirroring
  the push loop's dedup); a stale-cursor `404` resets the cursor and
  refetches marker-less within the same drain (backend-spec §11.4)
  instead of wedging sync until `sync_state` was deleted manually;
  and an `mls-message-add` that fails with *any* error class now
  triggers the same room recovery as `MlsOutOfOrder` (previously a
  commit lost to a transient error healed only when the next message
  in the room broke). Room recovery itself now tolerates a missing
  local group (nothing to discard → straight to rejoin). Docs: the
  api-mapping `MlsOutOfOrder` row and the error display no longer
  promise "hold the cursor and retry" — the drain heals, it does not
  retry.

- `Client::logout(purge_device = true)` no longer fails after a
  successful device deletion: deleting the device invalidates the
  session server-side, so the trailing `/access/logout` is rejected
  with "Invalid zauth token" — that rejection is now logged and
  ignored (the session died with the device), and `kupferdraht-cli logout
  --purge-device` proceeds to wipe local state instead of erroring
  out. Without `purge_device` a failing logout still surfaces.

- Asset messages (files) read back from the timeline as plain
  `Text` on every re-read (app restart, conversation reopen, or a
  poll-driven consumer re-reading `messages()`) instead of `Asset`,
  because the read path never loaded the stored attachment.
  `Client::messages` / `Timeline::items` now reconstruct the
  `Asset` kind from the room's attachments, matching what the live
  inbound path already produced. (Step 35, PR #29)

- A message send that raced a stale MLS epoch (`409
  mls-stale-message` — another member's commit not yet processed)
  failed outright instead of recovering. Sends now resync the room
  to the current epoch and retry once before giving up; a second
  consecutive stale still propagates as an error. (Step 37, PR #30)

- An inbound message whose epoch was ahead of the local MLS group
  (its advancing commit was missed) was silently dropped and the
  notification cursor advanced past it, losing the message for
  good. The receive path now applies in-order commits inline and
  heals the room (resync) on a genuine gap instead of discarding
  the event. (Step 38, PR #32)

- A resumed session could enter a repeating `403
  invalid-credentials` refresh loop when two `Client` instances
  existed for one account, each racing on Wire's single-slot `zuid`
  refresh cookie — `resume` / `login` now cache one live `Client`
  per account. A `403 invalid-credentials` refresh failure is now
  surfaced as a distinct, terminal `KupferdrahtError::SessionExpired`
  (with a matching `SyncListener::on_session_expired` callback)
  instead of being retried indefinitely or failing silently.
  (Step 39, PR #33)

- Re-login after a session expiry registered a brand-new device
  every time instead of reusing the existing one, because the reuse
  lookup keyed on the bare user id while sessions are stored under
  the qualified `<uuid>@<domain>` id. Fixed to look up by the
  qualified id, so a re-login reuses the existing device and MLS
  state; when the device genuinely no longer exists on the server,
  its stale local MLS/timeline state is now cleared instead of
  carried forward onto the new device. (Step 40, PR #34)

- Two devices logged into the same account could knock each other's
  session offline, because the Wire refresh-cookie label was not
  unique per device (login used a constant label; device
  registration used a per-*user*, not per-device, label) — each
  install now generates and persists its own cookie label, so
  devices no longer collide. Device registration also gains a
  caller-supplied `device_label` and derives a platform-appropriate
  device class/model, so devices show distinct, meaningful names
  instead of identical ones. (Step 41, PR #35)

### Security

- **Four dependency updates for RustSec advisories of 2026-10-23/26, two
  accepted without a fix** — `tokio` 1.52.1 → 1.54.1 (RUSTSEC-2026-0386,
  `AsyncFd` constructors violating I/O safety), `rustls-webpki` 0.103.15 →
  0.103.16 (RUSTSEC-2026-0382, distrusted intermediates accepted in path
  building), `webpki-roots` 1.0.7 → 1.0.11 (RUSTSEC-2026-0381, the
  distrusted emSign root removed; the 0.26 compatibility crate re-exports
  this one) and `ctutils` 0.4.2 → 0.4.7 (RUSTSEC-2026-0385, non-constant-
  time mask generation). Two advisories have no fix our graph can take and
  are listed in `deny.toml` with their reasons: `basic-toml` is unmaintained
  (RUSTSEC-2026-0374; a build-time proc-macro of the UniFFI generator, not
  in the shipped code) and `crypto-bigint` 0.5 has timing variability in
  operations meant to be constant-time (RUSTSEC-2026-0387; the fix is on
  the RustCrypto 0.14 line, which openmls' crypto crates and `voprf` do not
  take yet — open.md §81 tracks the upgrade). The third-party reports list
  the new versions.

- **rustls 0.23.45 → 0.23.46** — picks up the fixes for RUSTSEC-2026-0346 and
  RUSTSEC-2026-0347 (derived `Debug` output of client session values could expose TLS
  secrets to custom session stores; buffered application data after an
  ECH rejection). Kupferdraht uses neither a custom session store nor
  ECH; the bump keeps `cargo deny check advisories` green.

- **Rooms heal only on a confirmed gap; only the backend's Remove drops a
  group** — Step 106 (review finding H4, ADR-0034). Three triggers
  discarded a room's MLS group and rejoined it on weak evidence: a
  message claiming a later epoch (its epoch is unauthenticated when it
  cannot be decrypted), a transient storage error, and a Remove proposal
  for our own device from any member. Now a later-epoch message heals
  the room only when the backend's conversation record shows the local
  group fell behind; a storage error retries the notification instead of
  healing; and a member's Remove proposal for our device no longer drops
  the group — only the external sender's (the backend's) does. A real
  gap still heals; one extra request is made when a gap is suspected.

- **No session cookie, token, password or asset key in logs** — Step
  101 (review finding H6). The login logged the full `Set-Cookie`
  headers — the refresh cookie is a session on its own — at debug, in a
  warning and in the error text an app receives when the cookie is
  missing, plus the cookie's first characters. It now logs and names
  only the cookie names. And every type that carries a secret prints
  `<redacted>` (key bytes: `<n bytes>`) in those fields of its `Debug`
  output: `SessionRecord`, `AttachmentRecord`, `AssetSummary`, the FFI
  `AssetMeta`, the send-side `EncryptedAsset`, the login / rebind
  results, and the password- and code-carrying request types. Field
  values and the public API are unchanged.
- **Foreign data can no longer crash the client** — Step 95c. A room
  member, the backend or a damaged database used to be able to hit an
  `assert!` / `debug_assert!`. The worst cases panicked in release
  builds and killed the sync task: a task without `task_id`, a case
  without `case_ref`, a Welcome with an empty group id (which also
  poisoned the MLS lock for the session) and an empty device id in the
  push loop. Every such value is now a typed error or is skipped:
  - **Peer payloads.** Invalid task, case and mention data is ignored or
    dropped. Self-deletion timers are capped at 1 year. A case at the
    maximum revision refuses further updates.
  - **Backend responses.** Empty tokens, client ids, removal keys and
    `/self` ids are `Encoding`. Conversations without an id are skipped,
    and an empty `group_id` or team means none.
  - **Stored state.** A session cleared after `SessionExpired` resumes
    to `SessionExpired` instead of asserting (debug builds).
  - **App arguments.** An empty credential or id, or a malformed room or
    user id, is `InvalidInput`.
- **Foreign strings can no longer steer authenticated requests** —
  Step 95c. An asset's `asset_domain` / `asset_id` (chosen by the
  sender) and a `user.properties-set` key (backend) went into the URL
  path of a bearer-authenticated GET unchecked; `../self` or `%2e%2e`
  reached another endpoint. They must now be a single URL path segment,
  and `FileKeyStore` refuses a keystore id that would escape its
  directory.

- **Inbound edits and deletes are bound to the original sender** —
  Step 95b (ADR-0028). An inbound `MessageEdit` / `MessageDelete` used
  to apply by message id alone, so any member of a room — through an
  honest backend — could rewrite or delete anyone's message, task
  carrier or case snapshot. Now, checked against the MLS-authenticated
  sender (Step 95a) and the target resolved in the same room: an edit
  applies only when it comes from the target's original **user** (any
  of their devices); a delete applies when it comes from the original
  user, or targets an **ephemeral** message and comes from the self
  user (another own device syncing an expiry) or targets the self
  user's own message (receivers enforce the sender's timer — Step 89
  keeps working). A self-sent delete of another member's ordinary
  message is ignored, as the official webapp does. A third party's delete of
  someone else's ephemeral message is rejected (each receiver runs its
  own timer). No admin override — Wire's rule. A rejected edit / delete
  is dropped with a `warn!`. New `Store::load_timeline_event_in_room`
  (room-scoped target lookup; default body, so custom stores keep
  compiling).
- **Inbound messages are attributed to the MLS-authenticated sender,
  not the backend's envelope** — Step 95a (ADR-0027). The
  `sender_user_id` / `sender_device_id` of every inbound MLS message —
  `Message`, `TimelineItem`, `AssetSummary`, a task's
  `creator_user_id`, and the ADR-0018 case gate — used to come from the
  Wire event envelope (`qualified_from`), which the backend writes and
  no signature covers: a compromised backend could re-attribute any
  genuine message to any other member (and pass a member's case
  snapshot off as an admin's). They now come from the credential of
  the MLS leaf that signed the message (Wire's
  `{uuid}:{client_id}@{domain}`); the envelope is a cross-check only —
  a mismatch is dispatched under the MLS sender and logged as a
  `warn!`. No signature change, same fields: the values are now
  authenticated, and `sender_device_id` is now actually set for
  inbound MLS messages (Wire's envelope never carried the device, so
  it used to be `None` / `""` — observations §88). Two edges: a signer
  with an e2ei X.509 credential keeps the envelope attribution until
  X.509 identity parsing is verified (such senders stay
  unauthenticated), and a message whose Basic credential is not in
  Wire's shape is dropped with a `warn!` instead of dispatched. This
  authenticates *which device leaf* sent a message, not *who owns* it —
  under BasicCredentials the backend still distributes the keys
  (THREAT_MODEL 0.5). Live-smoked 2026-09-26 (Kupferdraht → Kupferdraht and
  official webapp → Kupferdraht).
- **`async-std` left the dependency tree** — Step 81. The
  discontinued runtime reached the workspace only through the
  `httpmock 0.7` dev-dependency (never in shipped binaries); the
  bump to httpmock 0.8 drops it, and the RUSTSEC-2025-0052
  suppression is removed from `deny.toml` — leaving exactly one
  tracked suppression (RUSTSEC-2026-0173, an unmaintained
  build-time proc-macro). Test-only change: ~21 test files migrated
  across the 0.8 API renames; the full suite is the regression
  proof.

- **Inbound ids can no longer reach the filesystem as path
  fragments** — Step 80. A malicious peer could send a message
  whose envelope `message_id` contained path traversal
  (`../../…`); on asset download the client would have written the
  attacker-uploaded blob to an arbitrary `.enc` path (directories
  created along the way). Inbound messages with path-unsafe ids
  (separators, `.`/`..`, control characters, overlength) are now
  discarded at the dispatch boundary like any malformed envelope
  (logged, drain continues, no consumer error); the asset-cache
  name derivation independently refuses such ids on both the
  receive and send paths. The same hardening covers the account
  identity: `UserId::parse` rejects path metacharacters in both
  parts, and the two places that materialize `base/accounts/<id>`
  directories refuse path-unsafe ids outright — a malicious
  backend cannot shape the on-disk layout through the login
  identity. Exploitation required an active asset download; the
  fixed `.enc` suffix already limited targets. (Step 80,
  Kalium-audit follow-up)

- **openmls 0.8.1 → 0.9.0: four suppressed advisories closed for
  real** — Step 76. The libcrux advisory batch of 2026-07-17
  (RUSTSEC-2026-0207/0208, SHAKE bugs in libcrux-sha3;
  RUSTSEC-2026-0212, aarch64 constant-time flaw in libcrux-secrets)
  and the wasm-only `instant` deprecation (RUSTSEC-2024-0384) were
  signed-off deny.toml suppressions, unfixable below the next
  openmls release. The 0.9 bump brings libcrux-sha3 0.0.10 and
  libcrux-secrets 0.0.6 (both fixed) and replaces the `instant`
  chain with `web-time`; all four ignores are removed and their
  absence is test-pinned (one suppression remains:
  RUSTSEC-2026-0173, an unmaintained build-time proc-macro via
  libcrux/hax — not a vulnerability). Companion bumps ride along:
  tls_codec 0.5, rusqlite 0.37, refinery 0.9, openmls_sqlite_storage
  0.3. Existing `mls.db` state is unaffected (for Kupferdraht's JSON
  codec both storage formats are byte-identical — observations §73);
  the 0.8.1 fixture pin and the `0-8-1-storage-format` feature that
  rode along were retired again before release (see Removed).

- **Asset key material is compiler-wiped** (Step 70, closes review
  gap-B.21): the per-asset AES-256 keys (`EncryptedAsset` /
  `EncryptedBytes` and their fresh-key locals) are now
  `Zeroizing<[u8; 32]>` behind borrowing accessors — the `DbKey`
  pattern — and the receive-side key handle is `Zeroizing` too, so
  every in-memory copy inside Kupferdraht's own types is wiped on drop
  (THREAT_MODEL G4). The wipe legitimately ends at two
  serialization boundaries (protobuf `otr_key`, SQLCipher-encrypted
  `attachments` row), documented on the types. The transient
  Ed25519 prekey signing keys were already wiped by
  `ed25519-dalek`'s default `zeroize` feature — the feature is now
  named explicitly in Cargo.toml and pinned by a unit test so a
  future `default-features = false` cannot silently drop it.
  (Step 70)

- The 2026-07-17 libcrux advisory batch — RUSTSEC-2026-0212
  (`libcrux-secrets`, incorrect aarch64 constant-time swap/select),
  RUSTSEC-2026-0207 (`libcrux-sha3`, incremental-SHAKE output bug) and
  RUSTSEC-2026-0208 (`libcrux-sha3`, AVX2 SHAKE-256 panic) — added to
  the `deny.toml` ignore list as a **signed-off policy deviation**:
  all three live in hpke-rs's SHA-3 arm and Kupferdraht's single
  ciphersuite uses no SHA-3/SHAKE; the fix chain (hpke-rs 0.7.0) is
  pinned out below `openmls_rust_crypto` 0.5.1. Removal trigger: the
  next openmls release. Analysis and sign-off recorded in
  `open.md` §62.

- `crossbeam-epoch` bumped 0.9.18 → 0.9.20 (lockfile-only) past
  RUSTSEC-2026-0204 (invalid pointer dereference in `fmt::Pointer`,
  transitive via openmls → rayon). RUSTSEC-2024-0384 (`instant`
  unmaintained, no CVE, wasm-only transitive via openmls's `js`
  feature) added to the `deny.toml` ignore list with a removal
  trigger: openmls `main` already replaced `fluvio-wasm-timer` with
  `web-time`; drop the ignore at the next openmls release.

## [0.1.0-dev] — pre-release history

Development log migrated from the former `findings.md`: resolved changes,
newest first, with the original §N numbers kept as headings.

### Cross-cutting commits (not tied to a step)

Migrated from the former `PLAN.md` §Cross-cutting commits on `main`
(work that landed on `main` but is not part of any single step):

- `859aafb` merge chore/cli-room-resolution-message-commands — make `--id` optional on `mark-read`/`react`/`delete-message`/`edit`/`reply`; CLI auto-resolves the room from `--message`/`--to` against local `timeline_events` (UX feedback after the Step 10 live-smoke)
- `e67ec2d` chore(storage): propagate UTF-8 decoding errors as `KupferdrahtError::Encoding` (chore 4 of AUDIT-2026-04)
- `8ca91cf` test(step_1b): lock in `/access` cookie-auth contract via regression guard (chore 3 of AUDIT-2026-04)
- `2d884a1` docs(findings): append three Step 1b live-smoke findings — §16–§18 (chore 2 of AUDIT-2026-04)
- `345279b` chore(client): decompose login pipeline into phases under 40-line limit (chore 1 of AUDIT-2026-04)
- `297e7d7` chore(api): public API alignment with matrix-rust-sdk + `docs/library.md` + `docs/api-mapping-matrix-rust-sdk.md`
- `b467b73` build: ignore dirty content in `reference/natrium` submodule
- `5b9783c` chore: empty `src/` trees per PLAN.md Starting State
- `6cce21a` docs: PLAN.md restructure + CLAUDE.md conventions + repo layout cleanup
- `543aaf4` docs: PLAN.md v2 — clean-slate rewrite
- `662d919` docs: add LEARNINGS.md
- `e68ca04` docs: ADR-0008 + ADR-0009 for the v14/matrix-rust-sdk pivot

## 55) SQLite3MC → SQLCipher engine swap; the §52 hang was SQLite3MC-specific (2026-06-10)

Three drivers — iOS WAL-mode shared-container header, audit/certification, and
the vendored-patch maintenance cost — prompted revisiting the storage engine.
Volker approved replacing SQLite3MC with real **SQLCipher** (Zetetic) via
rusqlite's `bundled-sqlcipher-vendored-openssl` feature; see ADR-0013. A desktop
spike validated it end-to-end:

- **Builds** with vendored OpenSSL (`openssl-src` 3.6.2); the vendored
  `libsqlite3-sys` patch + `[patch.crates-io]` redirect are removed.
- **The §52 `PRAGMA key` hang is gone** — it was SQLite3MC's software-AES cipher
  in unoptimised debug, not real SQLCipher (OpenSSL AES + raw-key/no-PBKDF2 is
  instant). This supersedes the §52 decision to keep ChaCha20.
- **Full suite + openmls/`mls.db` pass**; `libsqlite3-sys` is unified, so mls.db
  is SQLCipher-encrypted too.
- **The iOS plaintext header works** on SQLCipher — the feature that never
  engaged on SQLite3MC. Sequence: `PRAGMA key` **then**
  `PRAGMA cipher_plaintext_header_size = 32` → readable header; the salt
  round-trips via `PRAGMA cipher_salt` (retrieve on create, supply on reopen).
  Order matters — key first, header pragma second; the reverse leaves the header
  encrypted.

Accepted cost: an OpenSSL crypto backend on every native target (vs. SQLite3MC's
self-contained C amalgamation); cross-compile to Android/iOS must be validated
with vendored OpenSSL; WASM gets harder (deferred). The iOS header itself —
`cipher_plaintext_header_size` + a per-DB salt stored in the keystore — is a
follow-up step, not part of the engine swap.

---

## 56) Per-database separate keys + `DbKey` struct; the V20 migration's "SQLite3MC" comment is frozen (2026-06-10)

Branch-review cleanup of the SQLCipher work (PR #4) made two design refinements
and uncovered one immutable artefact:

- **`DbKey` is now a struct, not a 48-byte blob.** `DbKey { key: Zeroizing<[u8;32]>,
  salt: [u8;16] }` with `key()` / `salt()` accessors and a single keystore
  boundary (`from_keystore_bytes` / `to_keystore_bytes`, the only place the
  48-byte length invariant is enforced). This makes the secret/non-secret split
  (key is `Zeroizing`, salt is not) compiler-enforced and removes index
  arithmetic at the call sites.
- **Each database file gets its own independently generated key.** Previously
  `kupferdraht.db` and `mls.db` shared one per-account key. They now key separately:
  `kupferdraht.db` under `account-<id>`, `mls.db` under `account-<id>-mls`,
  `global.db` under `global-db`. The shared key was *cryptographically* safe for
  SQLCipher 4 (AES-CBC with a random per-page IV + keyed HMAC — no IV/keystream
  reuse), but separate keys are the defensible, audit-clean choice (defence in
  depth; nothing to explain away). The dead `load_account_key(persist=false)`
  branch — which would have keyed `mls.db` with a throwaway key, making it
  permanently unreadable — fell out with this change. A `right_key_wrong_salt`
  test now pins that each DB's salt is load-bearing.
- **Do not edit `migrations/V20__step_2b_drop_per_field_columns.sql`.** Its
  header comment still says "Whole-DB encryption (SQLite3MC, sub-step 2a)". That
  string is **frozen**: refinery checksums shipped migrations (see §50), so the
  comment is SHA-256-pinned and also baked into every existing dev DB. Correcting
  "SQLite3MC" → "SQLCipher" there would break `migration_immutability` and every
  on-disk database. The engine name in a historical migration comment is wrong
  but immutable; the *current* engine is documented in ADR-0013 and everywhere
  else.

---

## 51) SQLite3MC works through rusqlite — the Phase-0 link failure was libsqlite3-sys preferring pkg-config/system SQLite (2026-06-09)

Storage-plan Step 1 swaps the engine from vanilla bundled SQLite to
**SQLite3MC** (whole-DB encryption, SQLCipher cipher = AES-256, raw-key
mode). Phase 0 left one open question: rusqlite linked the **system**
SQLite instead of the SQLite3MC static lib (`sqlite3mc_version` missing
at runtime, on-disk header plaintext). This finding resolves it.

### Root cause

`libsqlite3-sys-0.30.1/build.rs` (`find_sqlite`, the `SQLITE3_LIB_DIR`
branch, ~line 428): when `SQLITE3_LIB_DIR` is set it **first probes
pkg-config** for `sqlite3` and only emits *our* `rustc-link-lib` /
`rustc-link-search` **if that probe fails**. On macOS pkg-config finds
the system `sqlite3.pc`, so our lib dir was silently bypassed and the
system `/usr/lib/libsqlite3.dylib` was linked. `SQLITE3_STATIC=1` alone
did not help — the dynamic system dylib still won.

### Proof (spike, `/tmp/mc-integ`, rusqlite 0.32 / libsqlite3-sys 0.30.1)

Neutralising the probe (`PKG_CONFIG_LIBDIR=<empty>`) plus
`SQLITE3_LIB_DIR=<our .a>` + `SQLITE3_STATIC=1` flips everything green:
`sqlite3mc_version() = Ok("SQLite3 Multiple Ciphers 2.3.5")`, on-disk
header **encrypted** (not plaintext), wrong-key read **errors**,
right-key reads succeed, **FTS5** match works — and `otool -L` shows no
system sqlite dylib (our `.a` statically linked). So the engine,
encryption and FTS all work *through rusqlite*; Phase 0 was a pure
link-routing problem, not an engine problem.

### Self-contained build decision (cross-compile clean)

The env-var/pkg-config trick is **not** viable for the shipped crate:
the lib path is per-target (`OUT_DIR`), a fixed `SQLITE3_LIB_DIR` can't
hold per-arch `.a`s, build-script ordering can't guarantee the `.a`
exists before `libsqlite3-sys` builds, and `PKG_CONFIG_LIBDIR=<empty>`
would break other pkg-config consumers. The clean path — the plan's
"Step 1 build glue" — is to build `libsqlite3-sys` **bundled** against
the SQLite3MC amalgamation: vendor a patched `libsqlite3-sys`
(`sqlite3/sqlite3.c` ← `sqlite3mc_amalgamation.c`, build.rs gains
`-DSQLITE_EXTRA_INIT=sqlite3mc_initialize`) and redirect rusqlite's dep
via `[patch.crates-io]`. That compiles per-target in `OUT_DIR` exactly
like vanilla bundled (Phase 0 already proved bundled cross-compiles to
`aarch64-apple-ios`), so no env vars and no pkg-config at consumer
build time.

- **Critical flag (unchanged from Phase 0):**
  `-DSQLITE_EXTRA_INIT=sqlite3mc_initialize` — without it SQLite3MC
  compiles and runs but **silently does not encrypt**.
- **Version skew is fine:** libsqlite3-sys 0.30.1's pre-generated
  bindings (SQLite 3.46 API) work against SQLite3MC 2.3.5 (SQLite
  3.53.2) — the spike linked exactly that pairing and all calls
  resolved. rusqlite never calls the `sqlite3mc_*` C API directly
  (cipher config goes through `PRAGMA`), so the stock bindings suffice.
- **Cost to record:** vendoring `libsqlite3-sys` means SQLite security
  updates must be pulled manually; `deny.toml` must accept the vendored
  crate + amalgamation licenses (SQLite public-domain, MC MIT).

---

## 52) SQLite3MC SQLCipher cipher hangs on `PRAGMA key` in debug builds; ChaCha20-Poly1305 chosen instead (2026-06-09)

Engine-swap sub-step 2a wires whole-DB encryption: `SqliteStore::unlock`
opens the connection, applies the raw 32-byte keystore key via
`PRAGMA key = "x'<hex>'"`, then migrates. The storage plan specified the
**SQLCipher cipher (AES-256-CBC + HMAC)** for BSI-friendliness, selected
by compiling the vendored amalgamation with
`-DCODEC_TYPE=CODEC_TYPE_SQLCIPHER`.

### Symptom

With the SQLCipher cipher, `store.unlock(key)` **hangs** (>60 s, never
returns). Pinpointed with debug markers to the `PRAGMA key` call itself —
the C codec is entered and does not return. Reproduced with both a
passphrase (`PRAGMA key='…'`) and a raw key (`x'<hex>'`), so it is not
the key format. Per-connection cipher *selection* (`PRAGMA
cipher='sqlcipher'`) is separately rejected by this build ("Cipher
'sqlcipher' unknown"), which is why the compile-time `CODEC_TYPE` default
was used.

### What was ruled out / observed

- **Not pure KDF iteration cost.** `PRAGMA kdf_iter = 1` before the key
  did **not** help — either the pragma does not take effect in this
  SQLite3MC build at that point, or the stall is not the PBKDF2 loop.
  (`SQLCIPHER_KDF_ITER` defaults to 64000; that alone should not be 60 s.)
- **Debug-build specific factor.** The cc-compiled amalgamation follows
  the Rust profile, so debug tests link the cipher at `-O0`. Forcing the
  C to `-O2` was inconclusive: the `-O2` recompile of the 13 MB
  amalgamation itself takes ~5 min, which exceeded the test budget before
  a result appeared — and a 5-min-per-build cipher is impractical for the
  test suite regardless.
- **ChaCha20-Poly1305 is fine.** SQLite3MC's *default* cipher
  (ChaCha20-Poly1305, the sqleet scheme) was already proven fast in the
  Phase-0 spike (`PRAGMA key`, encryption + FTS, no stall), and with it
  all five `whole_db_encryption.rs` lifecycle tests pass in 0.10 s.

### Decision (2026-06-09, with the maintainer)

Use **ChaCha20-Poly1305** (drop the `CODEC_TYPE` flag, leave the
SQLite3MC default). It is a strong modern AEAD (TLS 1.3, WireGuard, age)
and BSI TR-02102-conformant, so it is an acceptable stand-in for the
plan's AES-256/SQLCipher. Cipher choice is **not a format lock-in** here:
pre-v1, fresh-start, no existing databases — switching to AES-256 later
(if the SQLCipher hang is root-caused, or Volker requires AES) is a
default-cipher flip plus a re-encrypt/fresh-start. The raw-key design is
unchanged: the 32-byte keystore key is the security parameter, no KDF in
the trust path. The AES-256/SQLCipher hang remains open to revisit.

---

## 58) Finding §24 (cross-process /access rebind rejected by Wire prod) is live-disproven (2026-06-09)

`resume.rs`'s `assert_access_token_fresh` hard-capped a resumed session
at 14 min, refusing to even *attempt* a refresh once the persisted
access token aged out — on finding §24's assumption that a
cross-process `POST /access` rebind with the persisted `zuid` is
rejected by Wire production. The single-shot CLI therefore demanded a
re-login after every ~15-min gap, which is UX-blocking for a real
client (a restart past the access-token TTL should refresh silently,
not force credential re-entry).

### Live verification

A diagnostic probe (`refresh_stale_access_token`) ran against a real
Wire production account whose persisted session was **882 minutes
(~14.7 h) old**:

```
INFO resume: access token stale — probing cross-process /access rebind (finding §24) age_min=882
INFO resume: cross-process /access rebind SUCCEEDED — §24 does NOT hold; session refreshed in place
```

The `POST /v14/access` rebind with the persisted `zuid` returned 200 +
a fresh access token + a rotated `zuid` in ~144 ms. A cross-process
rebind **works**; §24 does not hold.

### Likely cause of the original §24 observation

Almost certainly a symptom of the cookie-collision bug (observations.md §18, §21): the old path persisted a stale/wrong `zuid` (or reqwest's cookie
store appended a duplicate), so the rebind sent an invalidated cookie
and Wire returned `403 invalid-credentials`. With the rotated `zuid`
now captured from the `/access` `Set-Cookie` and persisted correctly,
the rebind succeeds.

### Resolution

`resume` refreshes a stale access token in place
(`refresh_stale_access_token`): rebind with the persisted `zuid`, swap
the fresh token + rotated `zuid` into the session, and re-persist. A
live `zuid` keeps the session alive across process restarts (no
re-login); a dead `zuid` (its own — much longer — TTL elapsed, or
revoked) surfaces the rebind error so the user re-runs `login`. The
14-min `ACCESS_TOKEN_MAX_AGE_MS` is no longer a hard cap that errors;
it is retained only as the "is it worth refreshing yet" threshold.

Branch `feature/refresh-on-resume`; unit-tested in
`src/client/resume.rs` (`refresh_stale_access_token_tests`).

---

## 50) Editing a shipped migration's comments breaks refinery's checksum on existing installs (2026-06-05)

Surfaced while running the Step-18a live smoke. A `login` against
an **existing** `kupferdraht.db` (created 2026-05-08, last written
2026-05-22) failed at startup:

```
storage error: migration failed: applied migration
V12__step_6_ii_c_edit_chain is different than filesystem one
V12__step_6_ii_c_edit_chain
```

### Root cause

`refinery` validates each already-applied migration by comparing
the checksum stored in `refinery_schema_history` against a hash of
the **entire current migration file content — comments included**,
not just the executable SQL.

A later commit ("chore: drop Kalium-source citations from shipped
artifacts") rewrote **comments only** in two already-shipped
migrations (`V12__step_6_ii_c_edit_chain.sql` and
`V14__step_6_ii_e_reactions.sql`). The SQL statements are
byte-identical (`ALTER TABLE timeline_events ADD COLUMN
current_edit_event_id TEXT;` is unchanged). But the comment edit
changed each file's checksum, so `refinery` now rejects every
database that applied V12/V14 under the old comment text.

### Impact

- **Fresh installs:** unaffected — no prior history to compare.
- **Existing installs (real upgrade path):** hard-fail at the next
  `login`/storage-open. This is a shipped-migration-immutability
  violation: a migration file is an append-only artefact once it
  has been applied anywhere, and that immutability extends to its
  comments because refinery hashes the whole file.

### Resolution (decided 2026-06-05)

The V12/V14 comment rewrites are kept deliberately — the
Kalium-citation removal (ADR-0009) is the more important property,
and pre-v1 there are no external installations: only the
developer's own device was ever affected.

- **DB:** resolved by the fresh-login path already taken. The new
  active `kupferdraht.db` re-applied V12/V14 under the current file
  content, so its stored checksums match — it validates and is the
  go-forward database. The stale pre-migration state was backed up
  to `~/Library/Application Support/kupferdraht.bak-2026-06-05` and is
  **discarded** (not recovered); it may be deleted at will. No
  checksum-repair (`UPDATE refinery_schema_history`) and no revert
  of the comment edits was performed.
- **Process fix → Step 18b (landed 2026-06-05).** The real defect is
  that a doc-comment sweep was able to mutate an already-shipped
  migration at all. Step 18b adds the guard:
  `kupferdraht-core/tests/migration_immutability.rs` pins the SHA-256 of
  every `storage/migrations/VNN__*.sql` and fails
  `cargo test --workspace` if a shipped file changes (or a new one is
  added without an explicit pin). That closes the class of bug, not
  just this instance.

---

## 47) AIKIDO-2025-10330 — `goblin` 0.8.2 PE-parser integer overflow (2026-05-22)

### What was reported

Aikido (Pre-CVE, RustSec has no ID yet) flagged `goblin 0.8.2`
as Medium Risk (66/100): integer overflow in
`pe.tls.tlsdata.parse_with_opts` when parsing Thread-Local-Storage
data in PE binaries. Vulnerable range `0.8.2 – 0.9.3`, fixed in
`0.10.0`. Out-of-bounds memory access could lead to DoS or
information disclosure if the parser processes a maliciously
crafted PE file.

### Where it sits in our graph

```
goblin v0.8.2
└── uniffi_bindgen v0.29.5
    └── uniffi_build v0.29.5
        └── uniffi v0.29.5
            [build-dependencies]            ← build-only, not in
            └── kupferdraht-core v0.1.0             the shipped binary
                └── kupferdraht-cli v0.1.0
```

`goblin` is pulled in by `uniffi_bindgen` to parse the host
binary (PE on Windows, ELF on Linux, Mach-O on macOS) during
UniFFI's binding-generation step. It runs only at compile time,
on Kupferdraht's own build output — never on attacker-supplied input.
The vulnerable code path therefore cannot be triggered in any
realistic Kupferdraht deployment.

### Runtime risk: effectively zero

The exploit requires the parser to consume a maliciously crafted
PE blob. Kupferdraht's build chain never reaches a state where an
attacker could plant such a blob inside the parser's input:

- `cargo build` parses the bytes of the Kupferdraht dylib that Cargo
  just produced — content under our control.
- The shipped binary contains no goblin code at runtime; it is
  excluded from the dependency closure once cargo finishes the
  build step.

For an attacker to land the overflow, they would need to already
have write access to the build host's filesystem at the moment
of compilation. That is a much larger compromise than this CVE
permits and has nothing to do with goblin specifically.

### Why we can't bump

- UniFFI 0.29.5 (and the current latest, 0.31.1) both declare
  `goblin = "0.8"` in their Cargo.toml, which under SemVer
  resolves to `^0.8 = >=0.8.0, <0.9.0`. 0.10.x is outside
  that range.
- `[patch.crates-io] goblin = "=0.10.5"` is rejected by Cargo
  with `patch for goblin points to the same source, but
  patches must point to different sources`. The override
  mechanism is for switching to a fork (git URL or local
  path), not for relaxing the version constraint.
- A git-fork-only patch (point UniFFI's `goblin` dep at a
  vendored 0.10.x) would work mechanically, but the API
  surface UniFFI uses against goblin may have changed
  between 0.8 and 0.10 — probable rework required, all to
  fix a parser bug that cannot reach attacker input.
- `cargo update -p [email protected] --precise 0.10.5` does
  nothing for the same SemVer reason.

### Disposition

**Suppress the Aikido finding** as "won't fix" with a link to
this entry. When UniFFI upstream moves to `goblin = "0.10"`,
the issue resolves on a routine `cargo update`. We do not
intend to fork UniFFI or vendor a patched goblin for a
build-only vulnerability with no exploit path.

`cargo deny check` does not flag this today (it is Pre-CVE —
no RustSec advisory ID). If RustSec later issues an ID and
deny starts complaining, we add the advisory ID to
`deny.toml`'s `[advisories] ignore = [...]` with a comment
pointing back to CHANGELOG.md §47.

### How to verify when UniFFI catches up

```bash
cargo tree -i goblin
```

When the resulting goblin version is `>= 0.10.0`, this finding
is resolved without any code change on our side. Suppression in
Aikido can then be removed too.

### Cascading edits

- `CHANGELOG.md` §47 — this entry.
- No code changes. No `deny.toml` change (Pre-CVE, deny silent).
- Aikido suppression should reference this entry.

---

## 49) Step 15 re-scoped to `&str` ergonomics micropass; soft typed wrappers rejected (2026-05-22)

The original Step 15 design (`docs/plan/archive/15-typed-identifiers.md`,
CHANGELOG.md §43) proposed introducing `kupferdraht::ids::{UserId, RoomId,
DeviceId, MessageId, QualifiedUserId}` as soft typed wrappers
(newtype around `String`, no parse-time format validation) and
migrating every `&str` / `String` identifier parameter on the
public API surface to its typed counterpart.

After a design re-check the plan was rejected. Step 15 landed
instead as a minimal ergonomics pass on branch
`step/15-id-params-ref`.

### Why the soft-wrapper plan was rejected

Three concerns, in order of weight:

1. **Half-measure.** "Soft wrapper without validation" delivers
   compile-time type-tagging only — `Client::room(&RoomId)`
   cannot accidentally be called with a `&UserId`. But the
   wrappers do *not* validate format on construction (`UserId::new("")`
   succeeds). Two distinct properties — type safety and format
   validation — are usually bundled in typed-ID systems; Kupferdraht
   would have shipped only half. The user has recorded a
   preference for invariants visible at the call site (memory
   entry `feedback_explicit_invariants.md`); a no-op constructor
   is the opposite of that.
2. **Public-API surface cost vs. CLAUDE.md.** Five new public
   types + propagating them through every `Client`/`Room`/`Timeline`/`Account`
   method + the CLI flag-parse layer + UDL-exposure question
   (deferred to a workspace pass). CLAUDE.md §"Keep the public
   API minimal / Do not implement anything not required by the
   current Step and the next Step" applies — Steps 16 (account
   registration) and 17 (SAML) don't require typed wrappers.
3. **Forward-compat rationale was speculative.** CHANGELOG.md §43's
   second reason for skipping validation ("Wire might extend
   user_id to non-UUID forms like SCIM/email") was speculation.
   The v14 swagger is UUID-strict. If Wire ever does extend, the
   wrappers can be added then with concrete motivation.

### What landed instead

The actual ergonomic pain point in `v1-api-ergonomics-audit.md`
§4.1/§4.2 was owned `String` parameters forcing callers to write
`.clone()`. After Step 14 only four Client/Timeline methods still
took owned `String` for IDs / message-ids:

- `Client::room(room_id: String) -> Option<Arc<Room>>` →
  `room_id: &str`
- `Client::room_or_load(room_id: String) -> Option<Arc<Room>>` →
  `room_id: &str`
- `MessagesOptions::before(message_id: String) -> Self` →
  `message_id: &str`
- `Timeline::redact(item_id: String) -> Result<..>` →
  `item_id: &str`

Plus the §6.3 error message in
`kupferdraht-core/src/encryption/mls_client.rs:266` upgraded from
internal-detail ("user_id missing @domain: alice") to caller-
actionable text ("user_id must be qualified '<uuid>@<domain>'
format (got: 'alice'); see docs/library.md §Identifiers").

Net diff: ~30 LoC across 4 files (signatures + 1 error message);
~20 LoC of cascading caller updates in `runtime.rs`,
`timeline/send.rs`, `timeline/pipeline.rs`, `kupferdraht-cli/main.rs`,
and 9 integration tests.

### What stays open

- Audit §3.4 (`Account::qualified_user_id() -> Option<String>`
  shape) is unchanged — it was only *partially* closeable by
  Step 15's original plan anyway and is the right target for a
  separate "infallible accessors" pass.
- The `docs/plan/archive/15-typed-identifiers.md` plan-doc is retained
  in-tree as the record of the rejected option; the header now
  marks it superseded and links here.

### Cascading edits

- `CHANGELOG.md` §49 — this entry.
- `PLAN.md` Step-15 row — status `pending` → `merged`, scope
  description re-written, links here.
- `docs/v1-api-ergonomics-audit.md` — §4.1/§4.2/§6.3 entries
  dropped per project convention (commit precedents `de471c4`,
  `1a60704`, `f76c9bc`).
- `docs/plan/archive/15-typed-identifiers.md` — header marked superseded.

---

## 43) Step 15 — soft typed identifier wrappers, no validation (2026-05-21)

Step 15 closes the last documented API-shape debt from ADR-0009:
`kupferdraht::ids::{UserId, RoomId, DeviceId, MessageId,
QualifiedUserId}` introduced as **soft typed wrappers** — newtype
around `String`, no parse-time format validation.

### The two questions wrapped together

Typed identifiers usually bundle two distinct concerns:

| Question | What it provides |
|---|---|
| **A. Type safety** | Compile-time: `Client::room(&RoomId)` cannot be called with a `&UserId`. No runtime cost. |
| **B. Format validation** | Construct-time: `UserId::parse(s) -> Result<…>` rejects malformed identifiers early. |

Step 15 takes (A) and **explicitly skips (B)**. The construct is
infallible: `UserId::new(s)` is `Self`, not `Result<Self, _>`.

### Why no validation — even though Wire's swagger is strict

Verified against `reference/wire.swagger.json`:

```
Qualified_UserId = { id: UUID, domain: Domain }
UUID = { format: "uuid", type: "string" }   // RFC 4122
Domain = { example: "example.com", type: "string" }   // no constraint
```

Wire's user_id IS schema-strict (UUID format). A strict parser
in Kupferdraht would catch malformed inputs as Wire would reject
them anyway.

But three reasons make strict validation undesirable here:

1. **Kupferdraht's existing posture is permissive.** `wire/dto.rs:108`
   validates only `!is_empty()`, not UUID format. Every other ID
   site in the codebase is similarly permissive. Introducing
   strict validation for typed wrappers would be a unilateral
   policy change with no other code paths benefiting.
2. **Forward-compat fragility.** If Wire ever extends user_id
   to non-UUID forms (SCIM identifiers, e-mail-based IDs, custom
   external auth subjects), a strict parser would break Kupferdraht
   on the next Wire schema bump. Soft wrappers absorb the
   change transparently.
3. **Test / fixture friction.** Integration tests in `tests/`
   use synthetic string identifiers (e.g. `"alice"`, `"u1"`)
   for fixture readability. Strict validation would force every
   fixture to use real UUIDs, with no actual test-coverage
   improvement.

Wire still rejects malformed inputs at the HTTP boundary —
Kupferdraht just doesn't pre-empt that rejection at parse time.

### The wrapper shape

```rust
pub struct UserId(String);

impl UserId {
    pub fn new(s: impl Into<String>) -> Self { Self(s.into()) }
    pub fn as_str(&self) -> &str { &self.0 }
}

impl From<String> for UserId { fn from(s: String) -> Self { Self(s) } }
impl From<&str>   for UserId { fn from(s: &str)   -> Self { Self(s.into()) } }
impl Display      for UserId { /* writes the wrapped string verbatim */ }
impl AsRef<str>   for UserId { fn as_ref(&self) -> &str { &self.0 } }
```

No `FromStr` with custom error. No `try_new` returning `Result`.
No regex, no format check, no length limit. The wrapper carries
whatever string Wire serves or the caller hands in.

Identical shape for `RoomId`, `DeviceId`, `MessageId`. All four
are tag types whose only purpose is preventing accidental
identifier mixing at the function-signature level.

### The one exception: `QualifiedUserId::parse`

`QualifiedUserId { id: UserId, domain: String }` is the structured
counterpart for code that needs the two parts split. Its `parse`
constructor IS fallible — splitting `"uuid@domain"` into two
fields can fail (no `@`, empty parts). The error is the only
parse-error variant in the `ids` module:

```rust
pub enum ParseError {
    Unqualified,        // no `@` in input
    EmptyPart,          // empty id or domain after split
}
```

This is genuine validation (the parse can fail in a way the
caller must handle), not format enforcement.

### What types are covered

- `UserId` — Wire's user identifier (qualified form
  `"uuid@domain"` in Kupferdraht's public API, post Step 14a).
- `RoomId` — Wire conversation identifier (qualified form
  `"conv-uuid@conv-domain"`).
- `DeviceId` — Wire client identifier (currently a hex string,
  not UUID).
- `MessageId` — `GenericMessage.message_id` (UUID by Wire
  convention, no schema constraint).
- `QualifiedUserId` — structured `{ id: UserId, domain: String }`
  for explicit two-part shape.

### What this closes in the audit

Per project convention (commits `de471c4`, `1a60704`, the §9.1/§9.2
drop in `f76c9bc`), audit items resolved by a step are dropped
in the same commit. Step 15 closes:

- **`v1-api-ergonomics-audit.md §4.1`** —
  `Client::room(room_id: String)` becomes `Client::room(&RoomId)`.
  Owned-String pain root cause removed.
- **`v1-api-ergonomics-audit.md §4.2`** —
  `MessagesOptions::before(String)` becomes
  `MessagesOptions::before(&MessageId)`. Same root cause.
- **`v1-api-ergonomics-audit.md §6.3`** —
  `"user_id missing @domain: {user_id}"` error variant becomes
  unreachable (no `@`-validation in `UserId`); error site
  removed or re-contextualised.

Partially closes:

- **`v1-api-ergonomics-audit.md §3.4`** —
  `Account::qualified_user_id() -> Option<String>` retypes to
  `Option<&QualifiedUserId>` (owned-String allocation per call
  removed); `Option<…>` shape stays for a separate "infallible
  accessors" pass.

### Cascading edits

- `CHANGELOG.md` §43 — this entry.
- `docs/plan/archive/15-typed-identifiers.md` — new plan doc, sub-steps
  15a–15d, public-API block, implementation notes, TDD starter
  tests, audit-closure section, open questions, smoke test.
- `docs/api-mapping-matrix-rust-sdk.md` — rows 62, 63, 82, 178,
  179 marked "Pending — Step 15"; Open Questions §"Typed
  identifiers" flipped to "→ Step 15" with the soft-wrapper
  rationale; revision-log entry for 2026-05-21.
- `PLAN.md` §Status overview — new row for Step 15.

### Why this is the last API-shape debt

After Step 14a–14d and Step 15 land, the public API has:
- right names (Step 14b verb taxonomy)
- right method placement (Step 14d `Room` methods)
- right parameter shape (Step 14a single qualified peer)
- right types (Step 15 typed wrappers)

The remaining api-mapping "Drift" rows after Step 15 are all
**inherent Wire-vs-Matrix model differences** (e.g.
`join_all_rooms` as Wire-extension because Matrix has no bulk
join, `dm_with` as Wire-extension, the API-version accessor)
— not cosmetic or structural debt.

The `Option<…>`-on-accessors question (audit §3.1/3.2/3.3) and
the CLI-subgroup pass (audit §7.3) remain as their own coherent
future passes, both already documented as open questions in the
api-mapping doc.

---

## 44) Step 14 extended with sub-steps 14e / 14f / 14g (2026-05-21)

After completing the Step 14a–14d / Step 15 plan-locks, a cross-
check pass against `docs/v1-known-simplifications.md` surfaced
three insights that genuinely belong to Step 14's character
(matrix-rust-sdk parity polish) but were not in the original
14a–14d scope. **A colleague is already implementing Step 14**;
the existing sub-steps cannot be modified without disrupting
in-flight work. The three new insights are therefore **appended**
as 14e / 14f / 14g rather than woven into the existing sub-steps.

### Why each one belongs to Step 14

| Sub-step | Source | Why Step 14 (not separate) |
|---|---|---|
| **14e** — mutation methods return `Arc<Room>` | `v1-api-ergonomics-audit §1.5` | matrix-rust-sdk's mutations return the affected room; 14a's `create_dm(peer) -> Result<Arc<Room>>` already set the precedent in Step 14. 14e completes the pattern for the existing mutations (`invite_members` / `kick_members` / `set_room_access` / `rename_room`) so the public API surface is internally consistent. |
| **14f** — `Timeline::subscribe` returns `Subscription` handle | `v1-known-simplifications §2.7` | matrix-rust-sdk's `add_event_handler` returns `EventHandlerHandle`; dropping it unregisters. 14c is the typed-handler / listener territory in Step 14. The Subscription pattern lands alongside 14c — same RAII-clean lifecycle approach. |
| **14g** — `Client::room_or_load` + `Timeline::send_text_auto_join` | `v1-api-ergonomics-audit §1.1` + `§1.2` | 14a's `dm_with(peer)` introduced the lookup-then-create-if-missing convenience for the DM path. 14g generalises the same shape beyond DMs. The audit's §1.2 was the user's original trigger-case ("Timeline::send_text fails when MLS state is absent"). |

### Why appended, not interleaved

Three operational reasons:

1. **The colleague's branches are in flight.** Modifying the 14a
   public-API block or sub-step table after the colleague has
   pulled would cause merge conflicts in the plan doc and might
   suggest scope creep on already-started work. Appending three
   new sub-steps is additive.
2. **The original 14a–14d framing stays coherent.** 14a–14d
   address the original four matrix-rust-sdk audit findings
   (DM convenience, sync surface, typed handlers + filtered
   rooms, Room methods). 14e/f/g address three additional
   insights surfaced in a different sweep (the
   v1-known-simplifications cross-check). Keeping them as a
   second group preserves the audit-trail.
3. **Independent landing order.** 14e (mutation-return retrofit)
   can land any time after 14a. 14f (Subscription handle) lands
   alongside or after 14c (same listener-lifecycle territory).
   14g (auto-hydration) can land independently. None of them
   blocks 14a–14d or vice versa.

### Audit closures from this extension

When 14e / 14f / 14g land, the following entries drop from the
sibling audit docs (per the project convention; same pattern as
§42 and §43 closures):

**From `docs/v1-api-ergonomics-audit.md`:**
- §1.1 — `Client::room(id)` returns `None` when cache is empty
  (closed by 14g's `room_or_load`).
- §1.2 — `Timeline::send_text` fails when MLS state is absent
  (closed by 14g's `send_text_auto_join`).
- §1.5 — Mutation methods don't return the updated `Arc<Room>`
  (closed by 14e).

**From `docs/v1-known-simplifications.md`:**
- §2.7 — `Timeline::subscribe` has no `unsubscribe` (closed
  by 14f's Subscription handle).

§2.1 (UDL surface empty) and §2.2 (REST polling vs WebSocket)
in `v1-known-simplifications.md` get **forward-pointer updates**
— not closures. 14b's `sync_stream` and 15's typed wrappers add
to the UDL backlog (already deferred to the workspace-wide UDL
pass); 14b's `sync_*` family is built on top of the polling
mechanic. Both will be revisited when their respective umbrella
work happens; for now the updates ensure future readers see the
linkage.

### Files touched

- `docs/plan/archive/14-matrix-sdk-parity.md` — sub-steps header,
  sub-step table (three new rows), Public-API code block
  (mutations + Subscription + Timeline + room_or_load),
  matrix-rust-sdk-analog section (three new bullets),
  Implementation notes (three new blocks for 14e/f/g),
  TDD starter tests (three new blocks), CLI section (note
  that 14e/f/g are library-only), Audit closures section
  (new explicit section listing what each sub-step drops),
  Smoke test (notes for 14e/f/g).
- `PLAN.md` row 14 — 14a–14d → 14a–14g; description extended.
- `docs/api-mapping-matrix-rust-sdk.md` — row 141
  (`Timeline::subscribe`) flipped to "Drift (Pending — Step 14f)";
  row 162 (`rename_room`) flipped to "Aligned (Pending — Step 14e)";
  six new pending rows in the §"Pending parity items (Step 14)"
  sub-table (mutation retrofits, Subscription, room_or_load,
  send_text_auto_join); revision-log entry for 2026-05-21.
- `docs/v1-known-simplifications.md` — header note for the
  cross-check pass; §2.1 + §2.2 forward-pointer updates;
  §2.7 marked as planned for Step 14f.
- `CHANGELOG.md` §44 — this entry.

---

## 45) Account registration scope split: Step 16 (register) + Step 17 (SAML) (2026-05-21)

Kupferdraht has so far had no account-creation surface — only login
(`POST /login` against an existing account) and device
registration (`POST /v14/clients` for the authenticated user).
Account creation has been implicit out of scope since project
start, on the assumption that users sign up via Wires UI before
they ever touch a Kupferdraht client.

That assumption no longer holds: **Natrium creates Wire users
programmatically in production** and uses a SAML workflow in
the background. Kupferdraht needs a corresponding surface.

### Why split into two steps instead of one

Initial framing was a single "Step 16 — account creation" that
covered both personal-flow registration and the SSO workflow.
A cross-check against Wires SSO/SAML endpoints in
`reference/wire.swagger.json` revealed three discrete scope
options for SAML alone, with very different Kupferdraht-side cost:

| Option | Where SAML protocol-level work happens | Kupferdraht scope |
|---|---|---|
| **α** | Natrium-backend owns SAML; hands `UserSSOId` to Kupferdraht | ~100 LoC (plumbing only) |
| **β** | Kupferdraht drives Wires `/sso/initiate-login` + `/sso/finalize-login` | 200–400 LoC + UDL surface decisions |
| **γ** | External SCIM IdP provisions users into Wire | none — Kupferdraht picks them up at login |

Bundling either of these with the personal-flow register would
have produced one of two anti-patterns:
- **Bundle with α**: tiny step, mostly already covered by Step
  16's `sso_id` plumbing → no real second concern.
- **Bundle with β**: Step 16 grows by ~400 LoC + SP-flow design,
  which would block the unblocked register flow on a SAML
  decision that isn't ready.
- **Bundle with γ**: nothing to do — non-sensical.

The split: **Step 16** ships the register flow + `sso_id`
plumbing as a fully-decided unit. **Step 17** stays a placeholder
plan doc until Natrium's SAML flow is clarified, then becomes
α / β / γ accordingly.

### Step 16 scope summary

`POST /register` (unversioned, like `/login`) — Wires
`NewUser { name, email?, password?, sso_id?, locale?, label?, ... }`
body. Kupferdraht exposes the personal-flow subset:
`NewUserRequest { name, email, password, sso_id (plumbed),
locale, label }`. Response is HTTP 201 + new user record in
pending-activation status.

`POST /activate` with `{ code }` body converts pending → active.
`POST /activate/send` for resend.

`ClientBuilder::register(name, email, password) -> Arc<PendingClient>`
returns the pending handle; `PendingClient::activate(code) ->
Arc<Client>` completes the loop. Single-shot
`register_and_activate(...)` for non-interactive consumers.

CLI: `kupferdraht-cli register --email <e> --name <n>` +
`kupferdraht-cli activate --code <c>`.

Out of scope for Step 16 (and recorded in the plan doc):
- SAML / SSO flow (Step 17).
- Team creation (`NewUser.team`), invitation-code flow
  (`NewUser.invitation_code`).
- SCIM provisioning (server-side, no Kupferdraht involvement).
- `/provider/register` for bot accounts.
- Account deletion.

### Live-smoke posture

Step 16 tests against **Wires staging** (`staging-nginz-https.zinfra.io`),
not production. Production registers would create real Wire
accounts that count against quotas, send real activation emails,
and pollute the user namespace. CI must hard-block any Step 16
test that points at a `*.wire.com` URL.

The activation step (16b) is fundamentally asynchronous — Wire
sends an email containing the code, the user re-submits it.
Automated CLI / test live-smoke needs either:
- A staging test-mode endpoint that returns the code directly
  (recorded as Step 16's Open Question 1 — to be live-probed),
- Or a way to read a Kupferdraht-team test mailbox.

Either way, the contract is determinate; the test infrastructure
choice is implementation detail.

### Step 17 unblocking criteria

A short clarification conversation with Volker (+ Natrium auth
maintainer if available) covering:

1. Which IdP does Natrium SAML against?
2. Where does the SAML protocol-level work happen — Natrium-
   backend, or Wire's SP endpoints?
3. Are users provisioned via SCIM, `/register` + `sso_id`, or
   both?
4. Does Natrium need Kupferdraht to expose `/sso/initiate-login` for
   future browser-driven flows, or stays it backend-only?

Once answered, Step 17 is rescoped to one of α / β / γ and
gets a full plan doc analogous to Step 16's.

### Cascading edits

- `CHANGELOG.md` §45 — this entry.
- `docs/plan/archive/16-account-registration.md` — full plan: sub-step
  table 16a–d, public API code block, Wire-API details, TDD
  starter tests, CLI commands, Live-smoke posture section,
  Open Questions, Smoke test, Threat-model note.
- `docs/plan/17-saml-sso.md` — placeholder plan: the three
  options α / β / γ as decision framework, what we know so far,
  unblocking questions, cross-references.
- `PLAN.md` §Status overview — new rows for Step 16 (pending)
  and Step 17 (blocked-on-clarification).
- `docs/api-mapping-matrix-rust-sdk.md` — new ClientBuilder
  rows (`register`, `register_and_activate`) and a Wire-extension
  row for `PendingClient::activate`; revision-log entry.

---

## 46) `sync_metadata` stayed as `pub(crate)` helper after Step 14b (2026-05-21)

### What happened

Step 14b shipped `Client::sync_once` as the matrix-rust-sdk-shaped
public sync surface, but `sync_metadata` (Step-2's original
metadata-only refresh) stayed `pub` as a deprecated alias for
one release cycle (recorded in 14b's TDD-precursor notes). The
post-14b cleanup on 2026-05-21 moved it to `pub(crate)` rather
than deleting it. Two commits land the change:

- `2791b68` — `SyncService` → `pub(crate)`; the now-orphaned
  `SyncService::run_polling` flagged by `dead_code` and
  removed (PLAN.md row 75 had already promised "removed
  atomically", but only the call sites were gone; the
  definition was still there).
- `1d1f141` — `sync_metadata` → `pub(crate)`; the 9 eager
  `client.sync_metadata().await?` calls in `kupferdraht-cli` and
  ~24 calls across the integration tests migrated to
  `client.sync_once(SyncSettings::default()).await?`.

### Why not delete `sync_metadata` outright

Seven post-mutation call sites inside `kupferdraht-core/src/client/runtime.rs`
(`create_room`, `create_dm`, `invite_members`, `kick_members`,
`rename_room`, `set_room_access`, plus one bootstrap path) refresh
the local rooms cache after they update server-side state. They
deliberately want **metadata refresh only**, not the
notifications-drain side of `sync_once`. Replacing them with
`sync_once` would change semantics — every mutator would now
also drain queued notifications inside its critical section,
which is the wrong layering.

So `sync_metadata` stays as an internal helper, just no longer
visible from outside the crate. The public sync surface is
exclusively the four-method family.

### Why `sync_once` for the external migrations

The 9 CLI calls and ~24 test calls did not differentiate between
"need fresh metadata" and "happy with a notification drain too".
Replacing 1:1 with `sync_once(SyncSettings::default())` is
semantically a *strict superset* — same metadata refresh, plus
a queue drain that is either empty (test) or harmless (CLI). No
behaviour regression; if anything, the CLI is closer to the
audit-recommended pattern of "callers see a consistent post-sync
state".

Integration tests gained a one-line `common::mount_empty_notifications`
helper that mounts `/v14/notifications` returning an empty
queue — needed so `sync_once`'s drain phase finds something to
respond to.

### Side closures

- `docs/v1-api-ergonomics-audit.md §1.3` (CLI eager
  `sync_metadata` calls) reduced from 21 to 9 in flight, and
  `resume_client_and_store` now calls `load_rooms_from_store`
  so the cache is warm pre-sync. The audit entry stays open;
  per-handler "is `sync_once` even needed?" remains the
  unfinished half.
- The "deprecation alias for one release cycle" promise from
  14b's TDD notes is fulfilled — the cycle was exactly the
  five days between 2026-05-16 (14b's first commit) and
  2026-05-21 (today).

### How to apply (superseded by the 2026-05-22 revisit below)

Future references to "the sync_metadata pattern" mean **the
crate-internal helper**, not a public method. Public consumers
have `Client::sync_once` / `sync` / `sync_with_callback` /
`sync_stream`. Reach for `sync_metadata` only from inside
`kupferdraht-core` when the metadata-refresh-without-notification-drain
semantic is load-bearing.

### Cascading edits

- `CHANGELOG.md` §46 — this entry.
- `PLAN.md` row 75 — post-14b cleanup note appended.
- `docs/plan/archive/14-matrix-sdk-parity.md` §Public API — comment
  rewritten: `sync_metadata` is `pub(crate)`, not "public
  deprecated alias".
- `docs/library.md` — `sync_metadata` row removed from the
  Rooms table; remaining references updated to `sync_once`.
- `docs/cli.md` + `docs/api-mapping-matrix-rust-sdk.md` —
  user-facing references migrated to `sync_once`.
- `docs/v1-api-ergonomics-audit.md §1.3` — partial closure
  appended.

---

### Decision revisited 2026-05-22: `sync_metadata` back to public

The §1.3 follow-up conversation surfaced that the `pub(crate)`
move was the wrong call from a **Library-API-design** angle.
Kupferdraht is not primarily a CLI — `kupferdraht-cli` is a thin
verification harness for a library that will run on Android,
iOS, and Web. Those platforms have very different sync-shape
needs:

- **Mobile (battery-conscious):** Wants a cheap freshness check
  without holding a wakelock through the MLS decrypt of 50
  queued events.
- **Background-startup:** Refresh metadata first, decide later
  whether to drain.
- **Web (network-vorsichtig):** Cache-first, sync on-demand.
- **Foreground / Active UI:** Full `sync_once` — user is waiting.

Collapsing the public surface to *only* `sync_once` (the "drain
everything" call) forces every binding to either pay the full
cost or skip syncing entirely. The three-tier ladder gives
each binding the right primitive:

```
load_rooms_from_store  ← no network, hydrate from local DB only
       ↓ (optional, when freshness matters)
sync_metadata          ← cheap server round-trip, no decrypt
       ↓ (optional, when events matter)
sync_once              ← full refresh + drain + dispatch
```

Each layer is a meaningful boundary. The CLI uses all three —
proof that the trio works end-to-end.

### What changed (2026-05-22, commit pending)

- `Client::sync_metadata` is **public again**. The semantics
  are unchanged from yesterday's pub(crate) helper.
- `kupferdraht-cli` gains a `sync-metadata` subcommand that drives
  exactly `Client::sync_metadata` — the proof handle for
  bindings developers (and the smallest possible regression
  test that the metadata-only path still works end-to-end).
- The 9 in-flight CLI eager-call sites (§1.3) get a per-handler
  audit, using all three primitives:
  - 4× `sync_once` — write paths whose epoch validity depends
    on draining peer commits first (`send` / `send-file` /
    `mark-read` / `react` / `edit` / `delete` / `reply` via
    helpers, plus `room-join`, `rooms-join-all`).
  - 3× `sync_metadata` — pre-mutation freshness without the
    decrypt loop (`rejoin-all`, `room-invite`, `room-kick`).
  - 2× nothing (just rely on the cache `resume_client_and_store`
    warmed via `load_rooms_from_store`): `room-members` (the
    real fetch is in `sync_members`), `room-leave` (the
    mutation does its own post-call refresh).
- `docs/v1-api-ergonomics-audit.md §1.3` is now **closed**
  (no longer "partially addressed"): each of the 9 sites
  documents its intent through the call it makes.

### Why not delete `sync_metadata` outright

Same as the 2026-05-21 reasoning — 7 internal callers in
`runtime.rs` (post-mutation cache refreshes) need exactly the
metadata-only semantic. The new wrinkle (2026-05-22) is that
*external* callers benefit from the same primitive too.

### Why not extend `SyncSettings` with a `drain: bool` flag

Considered, rejected. A boolean field on `SyncSettings` would:
- Hide a major behavioural fork inside what should be a
  cheap config struct.
- Make the contract of `sync_once(settings)` depend on
  `settings.drain` — harder to grep, harder to reason about.
- Tempt callers to write `if drain { sync_once } else
  { sync_metadata }` adapters, which is what we'd give them
  for free with two distinct public methods.

Two named methods read clearly at the call site:
`client.sync_metadata().await?` vs
`client.sync_once(SyncSettings::default()).await?`. The intent
is the verb, not a config flag.

### Cascading edits (this commit)

- `CHANGELOG.md` §46 — this amendment.
- `kupferdraht-core/src/client/runtime.rs` — `sync_metadata`
  visibility back to `pub`; comment in `sync_once`
  re-classified the helper as public, not crate-private.
- `kupferdraht-cli/src/main.rs` — new `SyncMetadata` Command
  variant + `run_sync_metadata` handler; 5 §1.3 per-handler
  migrations as listed above.
- `docs/library.md` — `sync_metadata` row restored in the
  Rooms table with the three-tier composability rationale.
- `docs/cli.md` — new `sync-metadata` subcommand section.
- `docs/api-mapping-matrix-rust-sdk.md` — `sync_metadata`
  row restored, marked as Wire-extension (matrix-rust-sdk
  has no direct analog).
- `docs/plan/archive/14-matrix-sdk-parity.md` §Public API — comment
  re-rewritten to reflect the final state: `sync_metadata`
  is `pub`, `SyncService` stays `pub(crate)`.
- `docs/v1-api-ergonomics-audit.md §1.3` — note updated to
  **closed** with the per-handler breakdown.
- `PLAN.md` row 75 — cleanup note extended with the
  2026-05-22 revisit pointer.

---

## 40) `SyncSettings::poll_interval` instead of matrix-rust-sdk's `timeout` (2026-05-20)

Pre-implementation decision for the Step 14b sync-surface refactor
(`Client::sync_metadata` + `SyncService::drain_pending` → matrix-
rust-sdk-shaped `sync_once` / `sync` / `sync_with_callback` /
`sync_stream` family, per ADR-0009 and `docs/api-mapping-matrix-
rust-sdk.md` row 80).

### What matrix-rust-sdk's `SyncSettings::timeout` means

`/sync` in Matrix is a **server-side long-poll**. The client passes
`?timeout=<ms>` in the query; the server holds the request open
until either (a) new events arrive or (b) the timeout elapses. The
client immediately re-syncs with the new `since` token. Typical
default 30 s. Low `timeout` = more polls / minute, high `timeout`
= one held connection per cycle, near-zero event latency once the
server has the data. The client's HTTP-timeout must exceed this
value or the connection gets cancelled before the server can reply.

### Why this doesn't map 1:1 to Wire

Wire's `/v14/notifications` is **not a long-poll**. It returns
immediately with whatever is queued since the `since` cursor; an
empty queue returns an empty page, instantly. Wire's intended push
channel is the WebSocket (`wss://…-ssl.wire.com`) — see finding
§30 for the live observation that Wire emits no `event`-typed
frames over that WebSocket today, hence Kupferdraht's polling-only
posture. There is no `?timeout=` query parameter for
`/notifications` in the v14 Swagger.

A `SyncSettings::timeout` field in Kupferdraht's API that wore Matrix'
exact semantics would be a false promise: the parameter would
have nowhere to go in the underlying HTTP request.

### Decision

`SyncSettings` carries a **`poll_interval: Option<Duration>`** —
the wait between empty `/v14/notifications` polls in the loop
variants (`sync`, `sync_with_callback`, `sync_stream`).
`sync_once` ignores it. Default: 1 s.

```rust
pub struct SyncSettings {
    /// Backoff between empty `/v14/notifications` polls in the
    /// loop variants. Ignored in `sync_once`.
    ///
    /// Wire's notifications endpoint is not a server-side long-
    /// poll like Matrix' `/sync`, so this controls client-side
    /// pacing, not server-side hold. Higher values = fewer
    /// HTTP calls when idle, slightly higher event latency.
    pub poll_interval: Option<Duration>,
}
```

Why this name over plain `timeout`:

- **Honest about Wire's model.** A reader who sees `timeout` from
  a matrix-rust-sdk background assumes long-poll semantics. The
  rename forces the docstring to be read once, then the field
  name itself carries the meaning.
- **Kupferdraht-CLI precedent.** `kupferdraht-cli sync-stream --idle-backoff`
  already exposes this knob today (`main.rs:122`); `poll_interval`
  is the same concept under a slightly cleaner name.
- **Forward-compat-safe.** If Wire ever ships a long-poll endpoint
  or Kupferdraht re-enables the WebSocket path (finding §30), a
  separate field can be added without renaming.

### Trade-off accepted

Matrix-rust-sdk consumers reading Kupferdraht's API will notice the
divergence and must check the docstring. ADR-0009 explicitly
permits drift where Matrix concepts don't apply ("Some matrix-
rust-sdk concepts don't apply" — ADR-0009 §Negative). This is
one such site.

### Follow-up

- When the sync-surface refactor lands, update
  `docs/api-mapping-matrix-rust-sdk.md` row 80 to list the four-
  method family + record `SyncSettings { poll_interval }` as the
  Kupferdraht-specific shape with status "Drift (semantically honest)".
- `docs/v1-api-ergonomics-audit.md` §7.1 closes naturally when
  the CLI rename (`sync-fetch` → drops, `sync-stream` keeps,
  `sync` becomes `sync_once` one-shot) lands as part of the
  refactor.

---

## 41) Verb taxonomy for the public API (2026-05-20)

Decision recorded during the Step-14a/b discussion to lock in a
single, coherent verb scheme across Kupferdraht's public API. The
conversation surfaced that today's API mixes verbs inconsistently
— `Client::room(id)` has no prefix, `refresh_members` uses
`refresh_`, `sync_metadata` uses `sync_`, `fetch_room_access`
uses `fetch_`, all four for operations that fall into only two
genuine categories (cached lookup vs server-pull). The decision
fixes the scheme before Step 14 lands new symbols against it.

### The rule

Each operation class gets exactly one verb form. After Step 14
lands the migrations, the public API surface uses only these
verb forms:

| Operation class | Verb form | Net? | Examples |
|---|---|---|---|
| List accessor (plural) | noun, plural | no | `rooms()`, `joined_rooms()`, `devices()`, `feature_configs()` |
| Single-item lookup (singular) | noun, singular | no | `room(id)`, `dm_room(peer)`, `account()`, `user_id()` |
| Pull from server | `sync_*` | yes | `sync_once`, `sync`, `sync_with_callback`, `sync_stream`, `sync_members` |
| Create | `create_*` | yes (POST) | `create_room`, `create_dm`, `create_join_link` |
| Set/rename property | `set_*` / `rename_*` | yes (PUT) | `set_display_name`, `set_handle`, `set_room_access`, `rename_room`, `rename_device` |
| Membership / lifecycle action | action verb | yes | `join_room`, `leave_room`, `invite_members`, `kick_members`, `logout`, `join_via_link` |
| MLS recovery (re-do) | `resync_*` | yes | `resync_room`, `resync_existing_rooms` |
| Callback registration | `add_*` | no | `add_event_handler`, `add_timeline_listener` |

### No `get_` prefix

**Rust API Guidelines C-GETTER** explicitly bans the `get_`
prefix for accessors: idiomatic Rust uses `foo()` not
`get_foo()`. The standard library follows this throughout
(`String::len`, `Vec::iter`, `Account::display_name`).

Kupferdraht already follows this convention consistently today:
`Client::user_id()`, `Client::device_id()`, `Client::account()`,
`Client::room(String)`, `Account::display_name()`,
`Account::handle()`, `Account::email()` — all without `get_`.

matrix-rust-sdk uses `get_room(&RoomId)` and `get_dm_room(&UserId)`
despite the C-GETTER guideline. ADR-0009 prioritises Rust idiom
over matrix-rust-sdk shape when the two collide ("Established
Rust idiom" is the explicit Pro of ADR-0009 §Option B). Kupferdraht
therefore drops `get_` and accepts the resulting cosmetic Drift
from matrix-rust-sdk naming. Same caller contract, different
prefix.

Concretely:
- `Client::room(id)` — stays as is (already correct).
- `Client::dm_room(peer)` — new in 14a, no `get_`.
- Future room-related lookups follow the same form.

### One verb for "pull from server": `sync_*`

Today the public API has three different verbs for "this call
hits the server and updates local state": `sync_metadata`
(omnibus), `refresh_members` (targeted), and `fetch_room_access`
(targeted, no cache). The verbs imply distinctions that do not
actually exist at the caller's level — all three pull state from
the server.

After Step 14b:
- `sync_*` covers everything in this class.
  - Omnibus: `sync_once`, `sync`, `sync_with_callback`,
    `sync_stream` (the four-method family from finding §40).
  - Targeted: `sync_members(room_id)` (renamed from
    `refresh_members` — matrix-rust-sdk uses this exact name).
- `refresh_*` disappears from the public API entirely.
- `fetch_*` disappears from the public API entirely. The one
  remaining symbol, `Client::fetch_room_access`, migrates to
  `Room::access()` as part of the new Step 14d sub-step (also
  the first method on the `Room` type, which today is
  methodless).

### What does *not* change

- `resync_*` stays as a distinct verb. `resync_room` /
  `resync_existing_rooms` are MLS recovery operations (re-do an
  external commit because local state diverged) — semantically
  different from `sync_*` (incremental catch-up). The `re-`
  prefix signals "redo something that was done before".
- Action verbs (`join_*`, `leave_*`, `invite_*`, `kick_*`) keep
  their domain-specific names. Matrix-rust-sdk does the same;
  collapsing these into `sync_*` would lose information.
- `set_*` / `rename_*` keep their distinct verbs. They are
  mutations, not pulls; the conceptual difference matters
  enough to justify the verb split.

### Why this is recorded now, not at code time

The 14a + 14b implementation pass renames public symbols. If the
verb taxonomy is not locked first, each rename is an ad-hoc
decision and the inconsistencies recur as new symbols land. This
finding is the contract the implementation lands against. Future
PRs that add a public symbol can be reviewed against this table
in two seconds.

### Cascading edits

- `docs/plan/archive/14-matrix-sdk-parity.md`: 14a's `get_dm_room` →
  `dm_room`; 14b's scope grows to include the `refresh_members`
  → `sync_members` rename; new sub-step 14d added for
  `fetch_room_access` → `Room::access()`.
- `docs/api-mapping-matrix-rust-sdk.md`: row 178 status from
  Aligned → Drift (Kupferdraht drops `get_`); row 210 status from
  Aligned → Pending — Step 14b; row 350 status from
  Kupferdraht-specific → Kupferdraht-specific (transitional) with
  migration target; new Pending row for `Room::access()`.
- `PLAN.md` §Status overview row 14: 14a–14c → 14a–14d with the
  new sub-step described.

---

## 42) Step 14a parameter shape + `dm_with` convenience (2026-05-20)

Three follow-up decisions for the 14a sub-step, made together
because they form a coherent surface-shape choice.

### Context — Wire's native shape

Every Wire endpoint that references a user splits `id` and
`domain` at the protocol level:

- URL path: `/v14/one2one-conversations/{usr_domain}/{usr}`,
  `/v14/users/{domain}/{user_id}`,
  `/v14/conversations/{cnv_domain}/{cnv}/members/{usr_domain}/{usr}`.
- JSON body: `{ qualified_users: [{ "id": "<uuid>", "domain": "<host>" }, ...] }`.

There is no Wire endpoint that accepts a single `"uuid@domain"`
string at the protocol layer. Kupferdraht's internal `QualifiedId`
type (`wire/dto.rs:89`) matches this two-part shape.

### Context — Kupferdraht's existing public-API convention

Despite Wire's two-part native shape, every public symbol on
`Client` that takes an id uses a **single qualified-string** form:

- `Client::room(room_id: String)`
- `Client::join_room(room_id: &str)`
- `Client::leave_room(room_id: &str)`
- `Client::invite_members(room_id: &str, user_ids: &[String])`
- `Client::rename_room(room_id: &str, ...)`

The flattening is a deliberate choice rooted in:

- **UDL-friendliness:** UniFFI marshals `String` simpler than a
  two-field struct.
- **One-value-per-id convention:** callers hold IDs as one value
  (CLI flag, DB column, JSON field).
- **matrix-rust-sdk echo:** Matrix' `UserId` is also a single
  value (typed wrapper around `"@local:domain"`).
- **Unambiguous parse:** UUIDs cannot contain `@`; the qualified
  form is parse-clean.

The translation Wire's two-part shape ↔ Kupferdraht's single string
happens inside the `wire/` HTTP layer (`split_once('@')` on the
way out, `QualifiedId::Display` on the way in). Acknowledged
weakness: an unqualified caller string returns `None` silently
rather than failing at compile time. Tracked as the broader
"typed identifiers" open question in
`docs/api-mapping-matrix-rust-sdk.md` Open Questions section.

### Decision 1 — Single qualified `peer: &str` parameter

The original 14a draft proposed `(user_id: &str, user_domain: &str)`
— two separate string parameters. That was Wire-shape-honest but
**inconsistent** with every other room-/user-referencing method
on `Client`. All three 14a methods now take a single
`peer: &str` in qualified form:

```rust
Client::create_dm(peer: &str) async -> Result<Arc<Room>, KupferdrahtError>
Client::dm_room(peer: &str)              -> Option<Arc<Room>>
Client::dm_with(peer: &str) async        -> Result<Arc<Room>, KupferdrahtError>
```

Wire's two-part shape stays at the HTTP boundary, as everywhere
else.

If a future cycle introduces typed identifiers (the open question
above), all id parameters across Kupferdraht migrate together in one
coherent pass — not in 14a alone. **Half-converting the API
for one sub-step is explicitly rejected: when we touch
identifier types we touch them everywhere, or not at all.**

### Decision 2 — Add `Client::dm_with(peer)` as Wire-extension

Kupferdraht-specific convenience: lookup first; create if missing.
Calls `dm_room(peer)`; on `None`, falls through to `create_dm(peer)`.

matrix-rust-sdk has no analog because Matrix' DM model is
`is_direct`-flagged room state, not a first-class server endpoint.
For Wire, the lookup-then-create pattern is the dominant caller
shape (audit §1.1 auto-hydration class for the DM path). The
convenience earns its surface-area cost.

Documented in `docs/api-mapping-matrix-rust-sdk.md` as a Wire
extension entry — ADR-0009 explicitly permits Wire-specific
public symbols that lack a Matrix analog.

Concurrent calls against the same peer de-duplicate: an in-flight
`create_dm` POST returns its result to all waiters, so two
parallel `dm_with(peer)` calls do not POST twice (TDD test
required).

### Decision 3 — CLI keeps flat subcommands, single `--peer` flag

The original 14a draft had `dm-create --user-id <uuid>
--user-domain <d>` and `dm-show --user-id <uuid> --user-domain
<d>` — two separate flags for the two-part user reference.

The CLI now uses a **single `--peer "uuid@domain"` flag**,
matching the library's single-`peer` parameter. Subcommands stay
flat (`dm-create`, `dm-show`, `dm-with`) — consistent with every
other Kupferdraht CLI subcommand today (`room-create`, `room-invite`,
`mark-read`, `download-asset`, …).

The CLI subgroup restructure (audit §7.3 — `kupferdraht-cli dm <create
|show|with>`, `kupferdraht-cli room <create|invite|...>`, etc.) is
a separate global pass and explicitly **not** part of 14a. Doing
it for just `dm` would be inconsistent with every other CLI
command and would have to be undone or extended later. Either we
restructure the whole CLI in one coherent pass, or none of it
yet — same rule as Decision 1 applied to the CLI layer.

### Why all three together

Decisions 1–3 share a common principle: when consolidating to
matrix-rust-sdk-shape, **consolidate the simple way and do it
everywhere, or not at all**. Half-conversions (some methods
typed, some not; some CLI commands grouped, some flat) leave a
worse API than either endpoint of the choice. The two larger
restructures — typed identifiers and CLI subgroups — both live
as documented open questions and will be picked up as their own
coherent passes when they reach the top of the queue.

### Files touched

- `CHANGELOG.md` §42 — this entry.
- `docs/plan/archive/14-matrix-sdk-parity.md` — sub-steps header, 14a
  sub-step row, public-API block (single `peer: &str` + new
  `dm_with` method), implementation notes (new "parameter shape"
  block + `dm_with` block + existing `create_dm`/`dm_room`
  blocks updated), TDD starter tests (`peer: &str` expectations
  + `dm_with` tests + concurrent-call dedup test), CLI commands
  section (flat subcommands explicit, single `--peer` flag,
  subgroup pass deferred), Open Question 1 reworded (`dm_with`
  handles the cache-miss server-side case), smoke test
  (`dm-with` example).
- `docs/api-mapping-matrix-rust-sdk.md` — rows 178 / 179
  updated to single `peer: &str`, new row 180 for `dm_with` as
  Wire extension, revision-log entry.
- `PLAN.md` §Status overview row 14 — 14a sub-step description
  expanded to mention all three methods and the single-peer
  convention.

---

## 39) Step 9 — GPL-free dependency graph (ADR-0010, 2026-05-05)

Through Step 8 the dependency graph carried three GPL-3.0 Wire-origin
crates (`proteus-wasm`, `proteus-traits`, `cbor-codec`) for the prekey
bundle Wire requires at `POST /v14/clients`. Kupferdraht never executes
the Double Ratchet protocol — the Curve25519 keys exist solely to
satisfy Wire's registration schema (CLAUDE.md §Proteus). ADR-0010
treats this as a pure technical lock-in: the three GPL crates forced
Kupferdraht-as-distributable into GPL-3.0 even though zero strategic
license decision had been taken. Step 9 removes the lock-in; the
license decision itself is deferred to a separate future ADR.

### Before / after

**Before Step 9:**

* Direct git dep: `proteus-wasm = { git = "…/wireapp/proteus" }`.
* Transitives: `proteus-traits`, `cbor-codec`. All GPL-3.0.
* `deny.toml`: three per-crate `[[licenses.exceptions]]` entries
  allowing GPL-3.0 for those names.
* `[sources.allow-org]` permitted `github = ["wireapp"]` so the git
  source resolved.

**After Step 9:**

* No git-source dependencies — only crates.io.
* `Cargo.lock`: zero matches for `proteus*` or `cbor-codec`.
* `deny.toml`: GPL-3.0 unconditionally denied. `[sources]` allows
  only crates.io; the wireapp org allow-list is gone.
* New direct dep: `ed25519-dalek = "2"` (BSD-3-Clause; was already
  transitive through `openmls`).

### What replaced the GPL stack

`kupferdraht-core/src/encryption/prekeys.rs` — a ~150-LOC module that
emits the exact `proteus`-internal `PreKeyBundle` CBOR wire format
by hand. The format is a 5-entry map with `u8` integer keys
(`0..=4`), wrapping an `ed25519_dalek::VerifyingKey` (32 bytes)
inside two layers of 1-entry-map CBOR envelopes; encoder helpers
push the bytes directly without pulling in a CBOR codec crate.

The encoder schema lives in `encode_prekey_bundle`'s docstring;
the hand-rolled structural decoder in the test module is the
hermetic regression net (was a `proteus-wasm::PreKeyBundle::deserialise`
oracle through Step 9c, swapped to byte-level decode in 9e once
proteus-wasm itself left the tree).

### Live validation against Wire production (Step 9d, 2026-05-05)

Fresh `rm -rf` → `login` → `sync` → `sync-fetch` against Wire
production:

* `POST /v14/clients` returned `201` with the new device id —
  Wire's backend parsed Kupferdraht's hand-rolled CBOR cleanly.
* Subsequent `sync_metadata` returned 19 conversations.
* `sync-fetch` drained **287 notifications** including MLS
  handshake messages from other clients targeting the new device's
  leaf identity — the strongest end-to-end signal that the prekey
  bytes are wire-compatible. A mismatch would have surfaced as
  `400 mls-client-mismatch` or `404 mls-stale-message` on those
  events; both came back clean.

### Why Kupferdraht's own LICENSE didn't change in this step

ADR-0010 §Decision is explicit: this step removes the technical
constraint, it does not pre-empt the strategic license choice.
Kupferdraht remains under its current license. A future ADR will pick
between Apache-2.0 / GPL-3.0 / LGPL-3.0 with the dependency-graph
constraint already gone — both options stay open after Step 9
where Apache-2.0 was infeasible before.

### Lessons

* **Distinguish technical from strategic.** Conflating the two
  forecloses options that don't have to be foreclosed. ADR-0010
  cleanly separates them.
* **Wire's prekey format isn't copyrighted.** Protocol byte
  formats fall under EU 2009/24/EC Art. 1(2) and the equivalent
  US doctrine — implementing them from observation is
  interoperability work, not a derivative.
* **Hermetic tests > live-server-only validation.** The structural
  decoder in `prekeys::tests` catches encoder regressions in
  100ms; the live-Wire smoke catches anything subtler the decoder
  doesn't model. Both are needed; neither alone is sufficient.

---

## 35) Pre-Step-7 audit (2026-05-01) — four post-6-II review items, all resolved

Operational review after landing stable-PK resolution for inbound
`MessageEdit`/`MessageDelete`, `mentions` keyed by stable PK, and the
SQLite regression test (`canonical_message_id` no longer resolves a
prior chain link after `current_edit_event_id` advances).

Four items surfaced; all four resolved in the same commit window.

### A) Outbound `edit_message` chain-naïve when re-loading the row — **FIXED 3fb80a7**

**Where (was):** `kupferdraht-core/src/timeline/send.rs` — after a successful
`apply_message_edit`, the code called
`lookup_timeline_event(room_id, replacing_message_id)` with exact-PK
matching only. `apply_message_edit` matches via the OR-clause
`message_id = ? OR current_edit_event_id = ?`, but the post-apply re-read
silently dropped the row when the caller had a chain pointer in hand →
no `TimelineDiff::UpdateAt` for follow-up edits or deletes addressed
via a chain link.

**Fix:** commit `3fb80a7` (branch `fix/lookup-timeline-event-canonical`)
made `lookup_timeline_event` itself canonical-aware. The function now
calls `Store::canonical_message_id` first to resolve any-id → stable
PK, then loads by the PK. Defense-in-depth: every existing callsite —
outbound edit / delete / reaction post-apply re-reads, inbound dispatch
UpdateAt emissions, send_text reply parent lookup — automatically
benefits. Locked in by
`canonical_message_id_stops_matching_prior_chain_after_advance` in
`kupferdraht-core/src/storage/sqlite.rs` test module (commit `583addb`).

---

### B) `dispatch_inbound_delete` redundant `unwrap_or_else` fallback — **CLEANED UP cdbbe5f**

Was: `canonical_message_id(...).unwrap_or_else(|| delete.message_id.clone())`
solely to feed `lookup_timeline_event`. Now that `lookup_timeline_event`
resolves canonically itself (item A), the pre-resolve was dead code.
Commit `cdbbe5f` removed it from `dispatch_inbound_delete` and from
`Client::redact_message`. Same fix simultaneously made the code
symmetrical across all paths.

`dispatch_inbound_edit` and `Client::toggle_reaction` still pre-resolve
canonically because the resolved id has uses beyond the lookup
(mention storage PK / reaction storage PK). That's correct.

---

### C) `Client::edit_message` shipped empty `mentions` in the protobuf — **FIXED**

Was: `build_message_edit_generic_message(..., &[])` deliberately, with
the comment "CLI does not scan `@`-handles yet". Wire-Web/iOS render the
edited body with mention pills only when the `MessageEdit` carries
`Text.mentions` — Kupferdraht edits silently dropped mentions, so a user
who edited a message via Kupferdraht that originally contained `@alice`
would lose the pill on the edited version.

**Fix:** `Client::edit_message` now mirrors `Client::send_text` —
resolves `@<handle>` tokens in the new body via
`resolve_mentions_in_body`, threads them into
`build_message_edit_generic_message`, and (mirroring
`dispatch_inbound_edit`) calls `replace_mentions` locally so the
timeline overlay reflects the edit's mentions immediately without
waiting for the round-trip back through `dispatch_inbound_edit`.

The `UpdateAt` diff carries the resolved mentions inline so subscribers
see the edited row with the new mention list in the same dispatch.

---

### D) `resolve_qualified_handles` silent-no-op when `Account.domain()` is `None` — **DOCUMENTED + WARN-LOG**

**Where:** `Client::resolve_qualified_handles` — if `account.domain()`
is `None`, the function returns an empty map without error (unlike
`invite_members`, which rejects with `InvalidInput`).

**Decision:** keep the silent-no-op semantic for the resolver, but make
it observable. Rationale:

* The missing-domain case is defensive — corrupt / metadata-only
  resume DB, pre-login tooling. Login pipelines always populate the
  domain.
* `send` should degrade gracefully (literal `@handle` text in the body,
  no Mention pills) rather than fail the whole send. `invite_members`
  cannot do its job without a domain; `send` can.
* Failing the whole `send` because the mention resolver hit a corner
  case would be a worse UX than the body losing its pill decoration.

**Fix:** added a `warn!` log on the missing-domain branch of
`resolve_qualified_handles` so operators see the degradation in logs
without the call surfacing as an error to the caller. Added explicit
"Silent-no-op modes" docstring on `resolve_mentions_in_body` and an
explanation block on `resolve_qualified_handles` itself.

---

## 30) Wire's `/v14/events` WebSocket dropped a second time; canonical receive is now a polling loop modelled on matrix-rust-sdk (2026-04-30)

Step 5d's second attempt at WebSocket-based push receive against
Wire's `/v14/events` (ConsumableNotifications, rabbitMQ-backed)
was rebuilt from scratch and tested against Wire production over
multiple iterations. The protocol layer is byte-for-byte
determined by the Wire backend (cross-checked against
Kalium's `NotificationApiV9.consumeLiveEvents` as an
observation of that protocol):

* `Authorization: Bearer <access_token>` + `Z-Client: <id>` on the
  upgrade (no query-param token — that was a v0–v8 holdover).
* `?client=<id>&sync_marker=<uuid>` query params, fresh
  per-connection sentinel UUID.
* `consumable-notifications` capability declared at registration
  time in the `POST /v14/clients` body (Wire echoes it back in the
  response — verified server-side active).
* `GET /cookies` warm-up immediately before the WS handshake.
* `GET /v14/notifications/last` pre-connect probe (Kalium's
  slow-sync flow includes this).
* `MlsGroup` leaves present in every conversation (verified via
  `rooms-join-all`: 20 of 20 already_joined).
* All five Kalium WebSocket-lifecycle invariants reproduced.

Live observations under that setup:

* WS upgrade succeeds (`101 Switching Protocols`).
* Wire emits the initial `synchronization` frame within ~50 ms and
  echoes our sentinel `marker_id` verbatim.
* Kupferdraht acks correctly (`{"type":"ack","data":{"delivery_tag":1,
  "multiple":false}}`, `Frame.Text`); Wire does not retransmit, so
  the ack is accepted.
* Wire pushes keepalive `Ping` frames every 20 seconds — Kupferdraht
  Pong-replies, the connection stays alive indefinitely.
* **Wire never pushes a single `event`-typed frame**, regardless
  of whether the message originates from the same Kupferdraht device
  (own send), a different Wire-Web client of the same user, or a
  separate user (a second account via Wire-Web).

Read of Kalium's `EventRepository.handleEvents` and
wire-android's lifecycle (pulled as a 29 MB shallow submodule for
this investigation) shows no additional setup step beyond what
Kupferdraht does. The protocol implementation is correct on the
client side; the gap is at Wire's RabbitMQ-routing layer for
this account/team configuration. Without a same-account
comparison run of Kalium-CLI (significant build setup: JDK 21,
Gradle, AVS native libs) we cannot prove whether Kalium would
also be silent on this account or whether Kupferdraht has a subtle
difference invisible to source reading.

**Resolution: drop the WebSocket transport entirely.**
matrix-rust-sdk — Kupferdraht's canonical structural reference per
ADR-0009 — has **zero** WebSocket usage anywhere in its codebase
(`grep -rln "tungstenite" reference/matrix-rust-sdk` returns
nothing). Its `Client::sync()` is a long-polling HTTP loop
against `/sync?timeout=30s`. Kupferdraht's equivalent is a tight
short-polling loop against `/v14/notifications`:

```rust
pub async fn run_polling(&self, idle_backoff: Duration) -> Result<(), KupferdrahtError> {
    loop {
        let count = self.drain_pending().await?;
        if count == 0 {
            tokio::time::sleep(idle_backoff).await;
        }
    }
}
```

End-to-end latency with `idle_backoff = 2s` is ≤ 2 seconds in the
worst case; with `idle_backoff = 1s` (configurable via
`kupferdraht-cli sync-stream --idle-backoff 1`) it's ≤ 1s — same order
of magnitude as a WebSocket push, with none of the Wire-side
subscription pitfalls.

**Removed in the cleanup commit:**

* `kupferdraht-core/src/wire/websocket.rs` (whole file).
* `tokio-tungstenite` + `futures-util` deps in
  `kupferdraht-core/Cargo.toml`.
* `ConsumableNotification` / `EventAcknowledgeRequest` /
  `AcknowledgeData` / `AcknowledgeType` /
  `EventResponseToStore` / `SynchronizationData` DTOs from
  `wire/notification_dto.rs`.
* `HttpClient::warm_up_cookies`,
  `HttpClient::fetch_most_recent_notification`,
  `Client::http_warm_up_cookies`,
  `Client::http_fetch_most_recent_notification`.
* `SyncService::run` + `handle_consumable` +
  `dispatch_consumable_event` + `persisted_or_fresh_sync_marker` +
  `persist_sync_marker`.
* `reference/wire-android` git submodule — the investigation
  artefact, no longer relevant.

**Kept:**

* `sync_state.sync_marker_uuid` column from the V7 migration —
  unused but harmless; dropping it would require a destructive
  migration on existing user DBs.
* `consumable-notifications` in the `POST /v14/clients`
  registration body — harmless, idempotent against Wire.
* `Capabilities` field on `RegisterClientResponse` — useful for
  any future capability check, single-line addition.

`kupferdraht-cli sync-stream` is now the polling-loop variant
(blocks until Ctrl-C, default `--idle-backoff 2`). The WebSocket
investigation closes here. Should Wire's RabbitMQ-push layer
become reliable on a future account / team configuration, the
WS path can be revisited; the Kupferdraht client-side implementation
was protocol-correct.

---

## 32) `timeline_events.room_id` FK CASCADE wiped persisted history on every `sync_metadata` (2026-04-30, fixed in 6-II-a)

Surfaced during Step 6-II-a's live smoke test. After a single
`send` followed by web-UI inbound messages, each subsequent
`timeline` call showed only the **most recently drained** batch
of rows — earlier persistence had vanished. The two-call
pattern made it obvious:

```
timeline → drained: 4, items: 2 (only the 2 freshly drained)
timeline → drained: 1, items: 1 (only the 1 freshly drained)
```

Earlier rows kept disappearing.

**Root cause:** `V6__step_4_timeline_events.sql` declared

```
FOREIGN KEY (room_id) REFERENCES rooms (room_id) ON DELETE CASCADE
```

and `Client::sync_metadata` calls `Store::replace_rooms`, whose
SQL is `DELETE FROM rooms` followed by per-row `INSERT`. The
intermediate `DELETE` cascaded into `timeline_events`, wiping
every persisted row before the rooms were re-INSERTed. The CLI
helpfully runs `sync_metadata` inside several commands (`send`,
`timeline`, `room-invite`, `room-kick`, `room-leave`,
`room-rename`, `sync-fetch`, `sync-stream`), so any of them
silently destroyed the timeline cache.

The bug was invisible until Step 6-II-a actually exposed
`Room::messages` to read those rows. Step 4 only persisted
without reading; Step 5d's polling-loop dispatch ran on the
incoming event stream rather than re-reading the local DB.

**Fix:** `V9__step_6_ii_a_drop_timeline_events_cascade.sql`
recreates `timeline_events` without the FK CASCADE. Rationale:

* Timeline history is a *user-visible* artifact that must
  survive metadata refreshes. Conceptually it is a sibling of
  `rooms`, not a child.
* Wire's protocol does not give the client a "delete this
  room's history" signal. After `Room::leave` the room drops
  out of `rooms` on the next `sync_metadata`, but the user's
  local history of that room remains a sensible thing to keep
  visible.
* If a future "discard history" UX is needed, an explicit
  `DELETE FROM timeline_events WHERE room_id = ?` is one line.

`room_members` keeps its CASCADE — that table really *is* a
child of `rooms`, and members get repopulated by the same
`replace_rooms` transaction.

**Implication for Step 7's secure-storage migration:** the
`_ct / _nonce / _tag` columns on `timeline_events` need the
DEK rotation just like `rooms.name_ct` etc. The schema-
recreate pattern V9 uses (create new table, copy data, drop
old, rename) is the same move Step 7's migration will need
for every encrypted table — V9 is a useful template.

---

## 29) Re-`login` produces a new `device_id`; rooms from prior sessions become unreachable until self-resync (closed by Step 7e, 2026-04-29)

Surfaced repeatedly during Step 6c live testing. Every
`kupferdraht-cli login` registers a brand-new Wire client (a fresh
`client_id`/`device_id` pair). Wire's per-user device cap of 7
makes this visible — it's the same model on every Wire client.
Kupferdraht's `mls.db` is a single SQLite file shared across logins:
each device's `signature_keys` entry remains in storage after a
later login adds a new one. The MLS group state for a room
created by an earlier device still references the **old**
device's leaf in its tree; the new device's `MlsClient` has
fresh signature keys and a different credential identity.

Two failure surfaces follow:

1. **`Room::invite` from the new device into a prior-session
   room** — `MlsGroup::add_members_without_update` signs the
   commit with the new device's keys, but the leaf at
   `own_leaf_index` still carries the prior device's credential.
   Wire rejects with the §26 error
   (`mls-protocol-error: "Tried to add invalid LeafNode: Client
   identity does not match credential identity"`) — same error
   class, different cause.
2. **`Room::join` (External Commit) from the new device on a
   room where the same `user_id` already has a leaf** — Wire
   rejects with `400 mls-protocol-error: "Only the self client
   can be removed by an external commit"`. openmls's plain
   `ExternalCommitBuilder::build_group()` does not run the
   `resync` flavour by default; the "remove old leaf for the
   same user" semantics that Wire expects in this scenario
   require `with_resync_proposal`.

**Workaround in 6c's session:** keep the entire test (login →
create_room → invite → send) inside one login window so the
device that creates the room is also the one issuing every
subsequent commit. Documented in the `scripts/smoke-baseline.sh` flow.

**Resolution (Step 7e, merged 2026-04-29):**

* **Schema migration V8** — `session.device_id` column added so the
  active device is explicit. The pre-7e schema had `devices` keyed
  on `(user_id, client_id)` but no FK from `session`, and
  `load_device(user_id)` used a bare `LIMIT 1` (no ORDER BY) that
  returned an arbitrary row. Migration backfills the column with
  the most-recently-inserted device per user.
* `Store::load_device` signature now `(user_id, client_id) ->
  Option<DeviceRecord>` so callers must source the second argument
  from `SessionRecord::device_id`. The arbitrary-LIMIT-1 path is
  gone.
* `LoginPipeline::reuse_existing_device_if_valid` consults
  `Store::load_session` for the just-authenticated user, validates
  the persisted `device_id` against `GET /v{N}/clients/{id}`, and on
  `200` rebinds the new access token to it via
  `POST /v{N}/access?client_id=<id>`. Falls through to the existing
  `register_and_rebind` path on `404` (Wire-side eviction or
  user-driven removal) or when no session is on file.
* `LoginPipeline::resume_existing_mls_state` reloads the persisted
  `MlsClient` from `mls.db` for the reused device — no fresh
  signature keys, no fresh `KeyPackage` upload.
* `MlsClient::group_belongs_to_self` checks the loaded group's
  `own_leaf` credential identity against the current device's
  identity bytes. `MlsClient::discard_group` deletes the persisted
  state via `MlsGroup::delete`.
* `Client::resync_existing_rooms` walks every cached room: rooms
  whose `own_leaf` already matches the current device count as
  `already_current`; rooms with stale leaves get
  `discard_group + Client::join_room` (the unchanged Step-3
  external-commit path). `kupferdraht-cli rejoin-all` exposes this as a
  manual recovery surface.

**Live-smoke 2026-04-29:**

* `login` after the bug-era state reused the persisted device
  (same `<client-id>`) instead of registering a new client — `status`
  before and after carried the same `device_id`, only the access
  token rotated.
* `rejoin-all` resynced **20 of 20** prior-session rooms with no
  failures; running it again was a clean no-op (`already_current:
  20`). Sending into one of the previously-unreachable rooms
  (`c0ffee03-…@wire.com`, "Kupferdraht 6c smoke") immediately after
  resync was accepted by Wire.

---

## 25) `Application messages at epoch 0 are not supported` — Step 6a leaves rooms inert until 6b's first invite (2026-04-26 / -27, closed 2026-04-28)

> **Superseded by §61 (2026-06-16): the "inert until first invite"
> resolution was wrong.** Wire accepts a **self-update** establishing
> commit on create, so `create_room` can bring a solo room to epoch 1
> and make it immediately sendable. The self-update attempts below failed
> on framing/params, not because Wire refuses self-update — see §61. The
> original investigation is kept verbatim for the record.

**Closed 2026-04-28:** Step 6b's `Room::invite` advances a freshly
created (epoch 0) room to epoch 1 by uploading an Add-commit
bundle. Live-validated against Wire production: `Kupferdraht 6b smoke 3`
went from epoch 0 to epoch 1 on the first invite, and
`kupferdraht-cli send` then accepted an application message
(2026-04-28T07:39:59Z).
Two follow-up findings (§26, §27) document the dead ends along the
way.

---



Step 6a's `Client::create_room` brings a new conversation online,
but Wire rejects application messages there:

```
send_mls_message 400 Bad Request
{"code":400,"label":"mls-protocol-error",
 "message":"Application messages at epoch 0 are not supported"}
```

**First attempt — self-update bootstrap (didn't work):** add a
no-invitee self-update commit right after `MlsGroup::new_with_group_id`
to advance epoch 0 → 1. openmls's `self_update` with empty
`LeafNodeParameters` produces a no-op commit Wire rejects with
`Missing commit`; the same call with explicit credential params
produces a re-signed leaf Wire rejects with `Client identity does
not match credential identity`; switching to
`commit_builder().force_self_update(true)` then trips
`mls-client-mismatch: "A proposal of type Add or Remove does not
apply to the full list of clients for a user"` — Wire's server
holds implicit pending Add-proposals for the creator's other
devices that our self-update doesn't honor.

**Resolution:** match PLAN.md's original intent — Step 6a
deliberately ships zero-invitee rooms at epoch 0 with **no
commit-bundle upload**. They appear in `rooms`, are reachable as
`Room` snapshots, but `Timeline::send_text` fails until Step 6b
adds the first invitee (which produces a real Add-commit that
naturally advances the epoch). The Wire UI shows the same
behavior — empty rooms are placeholders until somebody is invited.

The unused `MlsClient` plumbing (`self_update_commit_bytes`,
`merge_pending_commit`, `clear_pending_commit`,
`MIXED_PLAINTEXT_WIRE_FORMAT_POLICY` on the create config) stays
in the tree — Step 6c's `Room::invite` and Step 6d's
`Room::kick` both produce commit bundles and will exercise the
same code paths.

---

## 23) Step 5 ships pull-only; WebSocket transport dropped pre-merge (2026-04-26)

After the Step 5a/5b/5c WebSocket-based prototype landed and the
consumable-notifications capability fix went in (finding §22), live
testing still didn't yield received events on the push path. Combined
with the operational reality that:

* the CLI has no UI loop that needs sub-second latency,
* the REST `/v14/notifications` endpoint exposes the same payload
  shape and dispatches through the same MLS decrypt pipeline,
* WebSocket auth has at least three Wire-specific quirks (LEARNINGS
  §1.6 query-param token, observations.md §22 capability, possible Kalium
  `GET /cookies` precondition on v9+) on top of the normal MLS flow,
* and the cookie-jar / reconnect / token-refresh state machine
  doubles the maintenance surface,

we deleted the WebSocket transport before merging Step 5. The pull
path (`SyncService::drain_pending` + `kupferdraht-cli sync-fetch`) is now
the canonical receive surface.

What was removed:

* `kupferdraht-core/src/wire/websocket.rs` (whole file).
* `tokio-tungstenite` + `futures-util` dependencies.
* `SyncService::run` / `SyncService::stop` / `handle_frame` /
  `handle_event` / `handle_synchronization` / `send_ack`.
* `EventStream` / `EventSink` types.
* `ConsumableNotification` / `EventDeliveryEnvelope` / `EventPayload`
  / `SyncMarkerData` / `EventAcknowledgeRequest` / `AcknowledgeData`
  DTOs.
* `kupferdraht-cli sync-stream` subcommand.

What stayed:

* `EventHandler` trait + `Message` / `MessageBody` types.
* `Client::add_event_handler` + `Client::sync()` (returns the
  pull-only `SyncService`).
* `MlsClient::process_inbound_message` — the decrypt path
  drain_pending uses.
* `sync_state` table — `last_event_id` cursor for the `?since=`
  parameter; the `sync_marker_uuid` column is now unused but
  kept for the future WebSocket return.
* `update_client_capabilities` PUT — still issued at login because
  Wire serves the same /notifications queue regardless of
  capability declaration, but a future device that wants the
  WebSocket can flip back to that path without re-registering.

The push variant comes back as a follow-up step once a
sub-second-latency consumer (UI binding, push-notification fan-in)
needs it.

---

## 24) Cross-process `/access` rebind doesn't work — resume must reuse the persisted access_token (2026-04-26)

> **Superseded by §51 (2026-06-09): live-disproven.** A cross-process
> `/access` rebind with a persisted `zuid` (~15 h old) succeeded against
> Wire production in ~144 ms. The conclusion below was almost certainly a
> symptom of the cookie-collision bug (§18 / §21); `resume` now refreshes
> a stale token in place rather than forcing a re-login. The original
> investigation is kept verbatim for the record.

After Step 5 closed and Step 6a went into live-smoke, every
`kupferdraht-cli sync` (and `room-join`, `room-create`, etc. — anything
that goes through `ClientBuilder::resume`) failed with:

```
backend rejected the request: rebind_access_token: 403 Forbidden
body={"code":403,"label":"invalid-credentials","message":"Invalid zauth token"}
```

The diagnostic at warn-level proved Wire IS rotating the cookie
correctly during login's internal /access call. Kupferdraht captures the
rotated value and persists it to `session.refresh_token`. But when
the next CLI invocation resumes and sends that rotated cookie back
to /access, Wire returns 403.

Diagnoses tried and ruled out:

* **Cookie collision via reqwest's auto-cookie-store** — turned off
  `cookie_store(true)` (the store is unused — every Wire call sets
  the Cookie header explicitly per observations.md §18). No change to the
  symptom; the rebind still fails.
* **Parser bug in `extract_zuid`** — confirmed via diagnostic
  output that the parser returns the right rotated value. Wire
  rejects that value when sent back.
* **Persistence regression** — confirmed `session.refresh_token`
  on disk holds the rotated value, not the original login zuid.

Working hypothesis: Wire's /access rotation is intended for
**single-process** Ktor-style auto-refresh (as observed in Kalium). The
cookie's rotation is bound to some session state — the Connection,
the TLS session, the in-memory cookie jar — that doesn't survive
across processes. Cross-process resume via /access just isn't a
supported path.

**Kupferdraht's fix** (this commit): `ClientBuilder::resume` skips the
rebind entirely. The persisted access_token is used directly via
Bearer auth on every downstream call. Wire's access tokens have a
~15-minute TTL; that covers single-shot CLI commands (login →
action → exit) comfortably. When the persisted token has aged past
14 minutes the resume returns a clear "re-run `kupferdraht-cli login`"
error, no surprise 403s.

Trade-offs:

* CLI workflow that pauses >14 min between commands needs a re-login.
  Acceptable because the CLI is for short-lived test runs; a
  background daemon would need a different strategy.
* The `RebindOutcome.refresh_token` carries the rotated cookie
  unused — kept on `Client` so a future long-lived consumer that
  re-implements Ktor-style auto-refresh has the latest cookie at
  hand without re-deriving it.

Follow-up backlog: investigate Wire-staging integration test as a
regression-protection layer; httpmock can't simulate Wire's
session-bound cookie semantics.

---

## 8) Fixed (2026-04-22): access-token refresh via `/access`

Wire's access tokens expire after about 15 minutes. Kupferdraht did
not refresh them; every API call after expiry returned `401`, and the
user had to run `rm -rf ~/.kupferdraht` + `login`.

**Implemented:** `http::refresh_access` calls `POST /v{N}/access` with
the persisted `zuid` cookie (observed in Kalium's
`AccessTokenApiV0.kt:33-58`; the V5 API inherits it unchanged).
`PersistedSession` now carries `access_token_issued_at: Option<u64>`
(Unix seconds); `resume_session` refreshes proactively once the token
is older than 10 minutes (Wire lifetime 15 min, 5 min of headroom for
the API calls that follow). If Wire rotates the refresh cookie, the
new one is persisted. On `401`/`403` from `/access` the local account
directory is deleted and the caller gets `Ok(None)` ("Not logged in"):
the refresh token was invalidated server-side (logout from another
device, password change).

**Open follow-ups (smaller than the original problem):**

- No reactive refresh: if a single CLI invocation runs so long that
  the proactively refreshed token expires during the call (more than
  10 min left at start, then a long sync), the proactive model does
  not help. Pragmatic way out for now: do not run that long. Real fix:
  a 401-retry wrapper around the authenticated HTTP calls in
  `http.rs`; relevant once Step 6 (event stream) opens long-lived
  connections.
- Telemetry: a refresh is not logged separately via `tracing`. Worth
  adding once we want to tell refresh failures apart in CI or test
  logs.

---

## 9) Fixed (2026-04-22): logout backend cleanup + exit code

**Backend client deregistration:** `Session::delete_client(password)`
calls `DELETE /v{N}/clients/{client_id}` (body: `{password}`). The CLI
flag `logout --purge-client` triggers it before the regular
`/access/logout`. Without the flag the old behaviour stays: token
revoke plus local cleanup only, without freeing the slot. Useful for
test workflows that otherwise keep running into Wire's 7-slot limit.
`--password-stdin` for scripting; there is deliberately no
`-p`/`--password` on the command line (same reasoning as for `login`).

**Exit code:** `logout` now always returns `exit 0` once local state
has been deleted; server errors are warned on `stderr`, not
propagated. `logout && login` chains thus work with an expired access
token too.

**Open:** no automatic listing of existing clients
(`GET /v{N}/clients`), no bulk cleanup. An account already at the
7-slot limit currently has to be cleaned up by hand in an official
Wire client. Should follow as `clients-list` and `clients-delete` CLI
commands once the limit bites in the test loop.

---

## 10) Fixed (2026-04-22): handle resolution `--user @<handle>`

`Session::resolve_handle(handle, domain?)` calls `POST /v{N}/list-users`
with `qualified_handles: [{handle, domain}]` and returns the qualified
user id. The default domain is the self domain. The CLI helper
`expand_user_args` runs before every `--user`-consuming command and
expands three input forms:

- `<uuid>@<domain>` — passed through.
- `@<handle>` — resolved against the self domain.
- `@<handle>@<domain>` — resolved against the explicit domain.

Wired into `conversation-create`, `channel-create`,
`conversation-add-users`, `conversation-remove-users`. An unknown
handle yields `InvalidInput` with a clear message.

---

## 11) Fixed (2026-04-22): counterpart user id in the 1:1 listing

`Conversation` now carries `counterpart_user_id: Option<String>`,
populated during `sync` from `members.others[0]` for 1:1
conversations. The `conversations` output shows the user id under the
name, so it can go straight into `--user` for `conversation-create` /
`conversation-add-users`.

---

## 13) Fixed (2026-04-22): Kupferdraht had to switch to Wire's ciphersuite

Learned empirically against a live Wire backend: Wire creates MLS
groups with ciphersuite **0x0002**
(`MLS_128_DHKEMP256_AES128GCM_SHA256_P256`), not with the RFC 9420
"default" 0x0001 (X25519/Ed25519). Presumably because iOS's Secure
Enclave supports only NIST curves, so P-256 is the one curve that can
be hardware-backed on every platform.

Kupferdraht was originally configured with
`Ciphersuite::MLS_128_DHKEMX25519_*_Ed25519`. Diagnosis at the first
`conversation-rejoin` attempt: the GroupInfo bytes began with
`00 01 00 02 …` (version MLS 1.0, ciphersuite 2), and openmls reported
`LeafNodeValidation(UnsupportedCredentials)` because our Ed25519
credential does not fit a P-256 group.

**Switched:**
- `crypto.rs::MLS_CIPHERSUITE` = `MLS_128_DHKEMP256_AES128GCM_SHA256_P256`.
- New `MLS_SIGNATURE_SCHEME` = `ECDSA_SECP256R1_SHA256`.
- `MLS_PUBLIC_KEY_TYPE` = `"ecdsa_secp256r1_sha256"` (Wire's DTO name
  for the signature algorithm in the `mls_public_keys` upload).
- `lib.rs` now reads the backend's external-sender key under
  `MLS_PUBLIC_KEY_TYPE` instead of the hard-coded `"ed25519"`.

Existing sessions (with an Ed25519 signature key) have to be set up
again: `logout --purge-client && rm -rf ~/.kupferdraht && login`
forces a fresh registration with P-256 keys.

**Open:** multi-ciphersuite support. Should Wire ever mix (older
groups on CS1, newer on CS2), we would need a signature key and a
key-package pool per ciphersuite and select by the GroupInfo's
ciphersuite at the external commit. Today a single suite suffices,
because every group observed live runs on CS2.

---

## 12) Fixed (2026-04-22): persistent MLS state via Step 4a

`MlsClient` now uses a `KupferdrahtProvider` that combines `RustCrypto`
(stateless) with `SqliteStorageProvider<JsonCodec, Connection>` from
`openmls_sqlite_storage 0.2`. The database lives at
`$KUPFERDRAHT_HOME/accounts/<uid>/mls/openmls.db` and is initialised
via `run_migrations()` on first open. Mutations write transactionally;
no explicit "persist after each op" is needed.

With that, `conversation-add-users`, `conversation-remove-users` and
(once Step 5/6 lands) `send_text` / `decrypt_message` can run on groups
from earlier CLI invocations.

**Open (Step 4b):** the SQLite file is still plain: MLS epoch secrets
and the ratchet tree sit unencrypted on disk, protected only by file
permissions (0600/0700). Threat-model G2 is deliberately relaxed in
this interim window, as recorded in PLAN.md Step 4a "Interim security
posture". Step 4b (DEK + keystore) closes it.

**Send/Sync consequence:** `rusqlite::Connection` is `Send` but
`!Sync`. So `MlsClient` is `Send + !Sync`, and `&MlsClient` must not
live across an `.await` in a `Send` future. Every call site in
`conversations.rs` owns its `MlsClient` for exactly one scope method
call and drops it before returning; no restriction in practice, but
documented explicitly on the `MlsClient` definition.
