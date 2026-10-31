# Kupferdraht Architecture — How It Works

The living description of **how Kupferdraht works internally**: the layers,
the modules, and the data flows that move a message from a `send_text`
call to ciphertext on the Wire backend and back. It is the counterpart
to the other docs:

- **What it is / why** — `README.md`, `docs/adrs/`, `rules/`.
- **What to do (binding rules)** — `rules/`.
- **The external backend contract** — `docs/backend-spec.md`.
- **The public API** — `docs/library.md`, `docs/bindings.md`, `docs/cli.md`.
- **The security model** — `docs/THREAT_MODEL.md`.
- **How it works (this doc)** — Kupferdraht's own internals.

This doc is kept fresh under the CLAUDE.md Freshness rule: an
architecture, module-boundary, data-flow, or storage-schema change
updates it in the same commit.

> The first draft's three _(verify)_ items were resolved during the
> 2026-07 docs catch-up: schema is V22 (V24 since Step 63 — §7), the
> event stream defaults to a REST notification pull with an opt-in
> WebSocket push since Step 62 — auto-negotiated between
> `/v14/events` and the legacy `/await` socket since Step 73,
> ADR-0019 (§5.3) — and in-order
> inbound commit processing shipped with Step 38.

---

## 1. What Kupferdraht is, in one picture

Kupferdraht is a Rust library that speaks MLS (RFC 9420) end-to-end
encryption to the **Wire** backend (HTTP API v14) and exposes a
**matrix-rust-sdk-shaped** public API — to Kotlin and Swift via UniFFI
(`ffi/`), and to the browser via a hand-written wasm-bindgen layer
(`wasm/`, parallel to `ffi/`, not layered on it). All cryptographic and
application logic lives in one crate — there is no internal FFI
boundary that plaintext must cross (ADR-0002, ADR-0006).

```
        Consumers (Kotlin / Swift / WASM / Rust CLI)
                          │
                          ▼
┌─────────────────────────────────────────────────────────┐
│  Public API  (matrix-rust-sdk shaped)                    │   ← stable
│  Client · ClientBuilder · Room · Timeline · SyncService  │     facade
│  Kupferdraht (device manager) · Account · EventHandler        │
└─────────────────────────┬───────────────────────────────┘
                          │  (no FFI boundary here)
┌─────────────────────────▼───────────────────────────────┐
│  Kupferdraht Core                                             │
│  MLS via openmls · whole-DB encryption · sync loop ·     │
│  timeline · rooms · keystore · all application logic     │
└─────────────────────────┬───────────────────────────────┘
                          │
┌─────────────────────────▼───────────────────────────────┐
│  Wire layer  (wire::*)                                   │   ← transport
│  HTTP v14, verbatim Wire field names, no business logic  │     only
└─────────────────────────┬───────────────────────────────┘
                          │  HTTPS
                          ▼
                   Wire backend (v14)
```

The facade is matrix-rust-sdk-shaped; the backend is Wire. A developer
who knows matrix-rust-sdk recognises the top layer; under the hood it
speaks Wire v14. Only the bottom layer changes if the backend ever does
(ADR-0009).

## 2. The three layers and the hard separation rule

| Layer | Owns | Vocabulary | Rule |
|---|---|---|---|
| **Public API** | `Client`, `Room`, `Timeline`, `SyncService`, `EventHandler`, `Kupferdraht`, `Account` | matrix-rust-sdk | never Wire-specific terms |
| **Kupferdraht Core** | MLS, storage, sync, timeline, rooms, keystore, identity | matrix-rust-sdk-ish | all application logic; no Wire field names |
| **Wire layer** (`wire::*`) | HTTP transport, request/response DTOs | verbatim Wire | transport only, no business logic |

The separation is a hard rule (ADR-0009, `rules/0009-matrix-rust-sdk-as-api-model.md`):

- `wire::*` uses Wire field names verbatim because they are externally
  constrained by the server.
- The translation between Wire terms and the public vocabulary happens
  inside the core, never at the API surface.
- Crypto (openmls) and transport (HTTP) never share a function:
  `timeline/` calls into `wire/`, never makes HTTP calls itself.

## 3. Module map (`kupferdraht-core/src/`)

The real layout, by responsibility:

| Module | Responsibility |
|---|---|
| `lib.rs`, `error.rs` | crate root + re-exports; `KupferdrahtError` |
| `client/` | authentication, session lifecycle, **login pipeline**, `Client`, `ClientBuilder`, `Account`; `token_refresh.rs` is the in-session access-token refresh and its cross-process critical section (Steps 39 + 85); `input.rs` checks the app's arguments at the public entry points (Step 95c) |
| `rooms/` | room state and membership (`Room`); `rooms/devices.rs` the device verification surface (`RoomDevice`, `RoomDevicesChange`, the diff — Step 95d) |
| `timeline/` | message send/receive, the `Timeline` read/send surface, per-interaction send files (text, reaction, confirmation, delete, knock, location, asset, ephemeral — the latter also owns the local expiry sweep + timers, Step 89); `timeline/task/` is the task vertical slice split by responsibility (`types` / `fold` / `order` / `views` / `cache` / `encode` / `send`) |
| `sync/` + `sync/dispatch/` | the event-stream loop and the inbound dispatcher (one handler per interaction) |
| `encryption/` | MLS client wrapper over openmls (`MlsClient`), the openmls storage `provider`, **asset crypto** (AES-256-CBC), **Proteus prekeys**; `mls_lock.rs` locks the shared `MlsClient` and recovers a poisoned mutex (Step 95c) |
| `keystore/` | the `SecureKeyStore` abstraction — a raw secret store (`get`/`set`/`delete_secret`, no KEK envelope). Four in-tree impls (`KeyringStore`, `FileKeyStore`, `InMemoryKeyStore`, wasm `IndexedDbKeyStore`); mobile embedders implement the trait via the UniFFI callback |
| `storage/` + `storage/sqlite/` | persistence: the `Store` + `GlobalStore` traits (`MaybeSendSync`-bounded), record types, `db_key.rs`, `engine.rs`, `home.rs` (the layout + the resolved home id), `home_id.rs` (`base/home.id`: read once, created under the home lock — Step 93), `atomic_file.rs` (temp → fsync → rename), `lock.rs` (native: the advisory per-directory `ProcessLock`, Step 85); ONE backend on every target since Step 91 (ADR-0022) = whole-DB-encrypted SQLite (SQLite3MC / ChaCha20-Poly1305 — `sqlite/`, `migrations/`); the browser adds `browser.rs` (wasm32-only: the OPFS `sahpool` VFS install + cipher wrap that puts the same files into the origin's private file system) |
| `identity/` | the `UserId` type (`localpart@domain`) |
| `manager/` | `Kupferdraht`, the device-level manager (multi-account / device lifecycle) |
| `wire/` + `wire/http/` | Wire HTTP API: `HttpClient`, DTOs, version negotiation |
| `ffi/` | UniFFI wrappers for the stateful objects (`Client`, `Room`, `Timeline`, …) — native targets only |
| `wasm/` | hand-written wasm-bindgen binding layer (`Kupferdraht`/`PendingLogin`/`Client` classes, `live_sync` polling loop) — parallel to `ffi/`, wasm32 only (Steps 44–59); driven from a **dedicated Web Worker** since Step 91 (OPFS) |
| `time.rs` | wall-clock shim (`now_ms`): `SystemTime` on native, `js_sys::Date` on wasm (W8, Step 48) |
| `logging.rs` | Step 117: `init_logging` installs Kupferdraht's `tracing` subscriber that hands Kupferdraht's own events (`kupferdraht::…`) to the app's `LogListener` — Kupferdraht writes nowhere itself; UniFFI-annotated in place, `wasm/logging.rs` adapts the browser's `onLog` |

Module organisation is **feature-based** (matrix-rust-sdk style), not
Kalium-style lifecycle-scope nesting (ADR-0009).

## 4. Crate & process topology

- **`kupferdraht-core/`** — the library, compiled for every target
  (Android, iOS, macOS, Linux, Windows, WASM, plus a server target).
- **`kupferdraht-cli/`** — a Rust binary (`clap` v4) that depends on
  `kupferdraht-core` directly. It is the canonical first consumer of every
  new API, before bindings exist (ADR-0007, `rules/0007-cli-in-rust-not-jvm.md`).
- **`bindings/`, `kupferdraht-uniffi-bindgen/`** — the UniFFI-generated
  Kotlin / Swift surface and its packaging scripts.
- **`vendor/`** — third-party build inputs, not Kupferdraht code: the
  SQLite3MC amalgamation (pinned by `PROVENANCE.txt`, `vendor/README.md`).
  Natively it is compiled by the `libsqlite3-sys` of the SchwarzDigits
  rusqlite fork, a git dependency by tag that carries a byte-identical copy
  (ADR-0042; refinery and `openmls_sqlite_storage` come from sibling forks
  so the graph holds one rusqlite); on wasm32 the fork's sys crate
  re-exports `sqlite-wasm-rs`, which compiles the amalgamation from
  `vendor/sqlite3mc/` for the browser (ADR-0022).
- **Web** — `wasm-pack build --target web` over `kupferdraht-core/src/wasm/`
  (hand-written wasm-bindgen, **not** UniFFI-generated — ADR-0015,
  which also carries the WASM-only delivery decision forward from the
  superseded ADR-0005). Needs a wasm-capable clang for the SQLite3MC C
  (`docs/bindings.md`); the artifact must run in a dedicated Web Worker.
  `bindings/web/kupferdraht-tabs/` (plain ES modules, no build) is the opt-in
  leader-tab / thin-client layer over it (ADR-0026); `bindings/web/opfs-smoke/`
  the consumer smoke pages.

## 5. Core data flows

This is the "how it works" heart of the system.

### 5.1 Login & client registration

`ClientBuilder` runs a multi-phase **login pipeline**
(`client/login_pipeline.rs`):

1. HTTP login → access token + cookie.
2. Register this device (`POST /v14/clients`) and rebind the access
   token to the new client. Proteus prekeys are generated **once** here
   and uploaded — never used for encryption afterwards (ADR-0010,
   `rules/0010-decouple-from-gpl-dependencies.md`).
3. Establish the MLS identity (P-256, BasicCredential), generate and
   upload KeyPackages (`POST /v14/mls/key-packages/self/{client}`),
   fetch and cache the backend's public keys (`GET /v14/mls/public-keys`).
4. Fetch **all** feature configs and user properties and persist them
   (ADR-0012, `rules/0012-server-feature-configs.md`); a fetch failure
   is non-fatal (warn + continue on safe defaults).

The result is a ready `Client` bound to an encrypted local store.

### 5.2 Sending a text message

`Timeline::send_text` (`timeline/`):

1. Build a `GenericMessage { Text }` protobuf (Wire's mandatory message
   schema).
2. Persist the outgoing row in `timeline_events` with its mentions
   (`delivery_status = sending`) and emit its `Append` — before anything
   leaves the device (Step 97a).
3. Encrypt it as an MLS application message at the group's **local
   epoch** via `MlsClient` (openmls `MlsGroup::create_message`) and POST
   the ciphertext to `POST /v14/mls/messages` (content-type
   `message/mls`).
4. Settle the row to `submitted` or `failed` and emit an `UpdateAt`. A
   `sending` row no longer in flight on this `Client` reads as `failed`
   and resends under the same id.

All sends funnel through one chokepoint, `Client::encrypt_and_upload`,
which gives every message type **epoch self-healing** for free (§5.6).

`Timeline::send_ephemeral_text` (Step 89) is the same pipeline with
the `Text` wrapped in Wire's `Ephemeral` (field 18) and the own row
stamped `expires_at_ms = sent_at_ms + ttl` once the upload succeeded;
a native `tokio` timer is armed for the stamp. Expiry is **local
self-redaction** on every client (Wire's timer is purely client-side):
`Client::expire_due_ephemeral_messages` tombstones elapsed rows via
the `MessageDelete` mechanics, emits `TimelineDiff::UpdateAt`, and for
inbound rows broadcasts one `MessageDelete` into the room
(`Client::upload_message_delete`, the network half of
`Timeline::redact`; best-effort) — the receiver-side enforcement
official senders wait for. It runs from the per-message timers, at
the end of every dispatch cycle (`SyncService::end_dispatch_cycle`,
shared with the Step-74 removal flush), and on resume
(`load_rooms_from_store` → sweep + re-arm).

**Failed sends and resend (Step 79).** Every send path persists its
row even when step 3 fails (`delivery_status = failed`, Step 25a) —
the wedged local echo. Since Step 97a the row is written as `sending`
**before** the upload and settled afterwards (`timeline/send_state.rs`);
the `Client` tracks its in-flight sends, and a `sending` row not in
flight (a dropped future, a restart) reads as `failed`. Nothing in core retries it (no scheduler, no
network observer — `docs/library.md` "Reconnect contract"). The
consumer re-drives it explicitly: `Timeline::resend` (`timeline/
resend.rs`) rebuilds the `GenericMessage` from the persisted row under
the **same `message_id`** (`timeline/resend_payload.rs`: text with
mentions + quote re-read from the store, an ephemeral row re-wrapped
with its persisted TTL, knock, location, and the asset reference from
the `attachments` row — the blob is already uploaded), runs steps 2–3
through the same `encrypt_and_upload` funnel, and flips the row to
`submitted` with `Store::mark_message_resent` (`sent_at_ms` re-stamped,
ephemeral timer armed). Task / case / marker rows are refused — they
are full-state snapshots and a stale replay could regress every
receiver's fold. `Timeline::resend_failed` runs the room oldest-first
and stops at the first hard error so a stuck message is never
overtaken; `Room::failed_sends` is the enumeration.

### 5.3 Receiving messages — the sync loop

The sync family (`Client::sync_once` / `sync` / `sync_stream`) drains a
**REST notification pull** by default; since Step 62 the loop variants
can opt into **WebSocket push** (`SyncSettings.transport =
SyncTransport::WebSocket`, native only — `wire/websocket.rs` +
`sync/push/`). Since Step 73 the concrete socket is **auto-negotiated
per connection** (ADR-0019, via
`Client::server_supports_push_events()`): the `/v14/events`
ConsumableNotifications dialect (per-frame ack strictly after the
cursor persist, redelivery dedup) when the server enables it, the
legacy version-less `/await` route otherwise — i.e. on every current
wire-server (observations §68). The legacy dialect
(`sync/push/legacy.rs`) has no acks and no redelivery, so its socket
is only a **doorbell** (Step 97b, ADR-0030): connect first, then one
REST drain; every durable frame triggers another drain (frames during
a running drain coalesce into one follow-up), and only the drain
dispatches and advances the pull cursor — a push the socket misses is
picked up by the next drain. `transient: true` frames (typing)
dispatch directly without touching the cursor, and client-side
keepalive pings hold cannon's idle timeout. Since Step 118 both
dialects share one pace (`sync/pace.rs`, `SyncSettings::idle_pause` —
`poll_interval`, 10 s unset, the polling loop's pause too): at most one connect per pace, a connect
that fails drains anyway (receive never depends on the socket), and a
legacy socket without a frame for a minute is renewed when no drain
runs. Push is the native default since Step 118 (ADR-0033).

Every transport fetches the room list (`list-ids`, `list`, `list-users`)
only when it may be out of date (Step 118, `client/room_freshness.rs`):
at start; when the drain hands over an event Kupferdraht does not process
itself (a room created or renamed, a member joined or left, a profile
changed) or a Welcome; and when this client cannot have seen every
event — the drain does not start where its last one ended (another
client or process drained the queue) or the backend lost the cursor. As a
safety net a list older than 5 minutes is refreshed too, so a change no
trigger caught is stale for at most that long.
`sync_once` refreshes a stale list before its drain (the rooms' device
baselines are still checked every pass — a local read), and every
dispatch cycle refreshes one it marked — a new room shows up in the same
pass. An idle pass is one request.

The browser has a doorbell of its own since Step 115
(`wasm/push_socket.rs`): a `web_sys::WebSocket` on the same `/await`
route, ringing the `spawn_local` live-sync loop so it drains on a frame
instead of on a timer. Since Step 118 (`wasm/sync_pace.rs`) the loop does
not poll beside a live socket: the socket's close rings too (reconnect,
then drain), a socket silent for a minute is renewed and drained once,
and only without a socket does the loop poll (every 10 s, retrying the
socket at most once per 10 s). Since Step 122 a successful own send wakes
it too (`client/sent_signal.rs`: every `POST /mls/messages` notes one, the
loop absorbs the ones its own pass caused) — the backend sends the sender
no frame, and the pass's `onSync` is the embedder's cue to read again; the
loop records per account whether it has an open socket
(`wasm/push_state.rs`, `pushState()`). Three differences from the native path,
all forced by the platform. Both credentials ride the query string —
a browser cannot set the `Authorization` header
`connect_with_bearer` uses. It sends **no keepalive**: the WebSocket
API exposes no `ping()`, and a silent socket was measured open for
392 s (2026-09-30), so none is needed — the one-minute renewal
catches a socket that died without a close. Durable frames are not read at all;
`transient: true` ones are parsed and dispatched as data (Step 115b),
since typing is live-only and a drain can never return it. The pull path:

1. `GET /v14/notifications?client=…&size=…[&since=<cursor>]` — pages
   are oldest-first; the loop chains `since` until `has_more` is false.
   The cursor (last notification id) is persisted in the sync state and
   survives restarts. A `404` on a stale cursor (queue purged past
   `since`) resets the cursor and refetches marker-less within the
   same drain — once per drain, so a marker-less `404` still surfaces
   (Step 64, backend-spec §11.4). A notification whose id equals the
   page's cursor is skipped as a redelivery (push-loop-symmetric
   dedup). Since Step 97d (ADR-0031, `sync/drain.rs`) each page is
   fetched without a lock and dispatched under the account's drain
   lock (`client/drain_lock.rs`: an in-process mutex, then
   `accounts/<id>/sync.lock`); a cursor another `Client` or process
   moved in between means refetch, not a second dispatch. Network work
   an event triggers — healing a room, a 1:1 delivery receipt, a config
   re-fetch — is queued (`sync/follow_ups.rs`) and runs in
   `end_dispatch_cycle`, after the lock. Since Step 97e (ADR-0032) and
   per page since Step 109 (ADR-0037, `sync/page.rs`, `sync/window.rs`)
   a fetched page runs in one window: one `mls.db` transaction and one
   `kupferdraht.db` page transaction with a savepoint per notification; the
   data commits first, then the MLS state, then the embedder is told,
   then the cursor advances — at-least-once processing; a replay skips
   what MLS already processed (older epoch, consumed key).
2. For each event, the dispatcher (`sync/dispatch/`, one handler per
   interaction — text, reaction, edit, delete, confirmation, knock,
   location, asset, ephemeral, config) decodes the envelope; the
   transient `conversation.typing` events are handled inline in
   `sync/service.rs` (no persist, no decrypt). An inbound
   `Ephemeral(Text)` (Step 89) reuses the text handler's tail with an
   `expires_at_ms` stamp (V1 timer start = dispatch) and arms the
   local expiry timer; non-`Text` inner arms are tolerated-unknown.
3. MLS application messages are decrypted via `MlsClient`; the
   `GenericMessage` content variant is matched and the timeline /
   storage is updated. The message is **attributed to the leaf
   openmls authenticated as its signer** (Step 95a, ADR-0027):
   `process_inbound_message` parses the signer's W-4 credential
   (`encryption/leaf_identity.rs`) before openmls drops it, and
   `sync/sender.rs` decides — the envelope's `qualified_from` /
   `from_client` is a cross-check only (mismatch → `warn!`, dispatch
   under the MLS sender), an X.509 signer keeps the envelope
   attribution until X.509 parsing is verified, a malformed Basic
   signer drops the message. Every handler (text, reaction, receipt,
   task creator, the ADR-0018 case gate, marker, asset, ephemeral)
   sees only the attributed sender. `conversation.typing` is not an
   MLS message and stays envelope-attributed by design. An inbound
   `MessageEdit` / `MessageDelete` is then **authorized against that
   sender** (Step 95b, ADR-0028, `sync/dispatch/authorize.rs`): the
   target is resolved in the same room (`Store::load_timeline_event_in_room`);
   an edit needs the target's original user, a delete the original
   user — or, for an ephemeral target, the self user (another own
   device) or a self-sent target (receiver-enforced expiry). Anything else is dropped with a
   `warn!` — no row change, no diff, no task / case fold change.

Unknown `GenericMessage` variants are logged and discarded gracefully —
never skipped at the envelope layer (CLAUDE.md "Wire Message Types").
Inbound commits are processed **in order** (Step 38). Transient events
that Wire delivers only on the live WebSocket (`conversation.typing`)
cannot arrive on the pull transport — they arrive on the Step-62 push
transport (see `docs/library.md`, "Choosing the receive transport").

### 5.4 Creating / joining an MLS group

- **Join** uses an MLS **external commit**: fetch `/groupinfo`, build the
  external commit, post via `/commit-bundles` (Steps 3/3a–3f). The Wire
  backend is registered as an **External Sender** in every group; its
  admin proposals (e.g. Remove on team departure) are validated against
  the cached backend key and **auto-committed** (ADR-0004, Step 28,
  `rules/0004-integrate-external-senders-extension.md`) — since
  Step 74 batched once per dispatch cycle (`sync/removal_flush.rs`):
  one commit covers every proposal the cycle delivered, failed
  flushes stay queued and retry next cycle.
- **Create** advances a fresh group from epoch 0 → 1 with a
  self-establishing commit so the room is sendable immediately
  (Step 31), rather than inert until the first invite.
- **The self conversation** (`RoomType::SelfRoom` — Wire's notes-to-self
  room, one MLS group of the user's own devices) is listed by Wire at
  epoch 0 with no `GroupInfo` on file, so an external join is impossible
  until a member commits (`404 mls-missing-group-info`, live 2026-09-24).
  `Client::join_room` establishes it from the first device (the Step-31
  commit: own other devices Added, self-update when solo) and
  external-joins from every later one; `join_all_rooms` inherits the
  branch, `Client::self_room` is the lookup (Step 86,
  `client/self_room.rs`).

### 5.5 File attachments — the two-channel flow

Files do **not** travel through MLS (ADR-0011,
`rules/0011-asset-content-protocol.md`):

1. Generate a fresh AES-256 key + 16-byte IV; stream-encrypt the file to
   a tempfile as `iv(16) || aes-cbc-pkcs7(plaintext)`
   (`encryption/asset_crypto.rs`), computing SHA-256 over the blob.
2. Upload the ciphertext to `POST /v14/assets` (multipart/mixed),
   receiving an `(asset_id, asset_token, asset_domain)` triple.
3. Send an MLS `GenericMessage { Asset }` carrying the key, the SHA-256,
   the mime/size, and the locator — the small **reference** rides
   through MLS; the bytes do not.
4. The receiver decrypts the reference, downloads the ciphertext,
   verifies SHA-256, decrypts, and surfaces it. Plaintext is never
   persisted; only ciphertext may be cached. Inbound AES-GCM assets are
   warn-and-skipped as "unsupported".

Integrity is layered: SHA-256 binds the reference to the blob; MLS
authenticates the reference's sender.

### 5.6 Epoch self-healing

When a send targets a stale MLS epoch the backend returns
`409 mls-stale-message`. `Client::encrypt_and_upload` maps this to a
typed `KupferdrahtError::MlsStaleMessage`, resyncs the room to the current
epoch (`resync_room`), and retries the send **once**; a second stale or
any other error propagates (Step 37). Because the fix lives in the
shared chokepoint, every message type inherits it.

## 6. Encryption & key management

Three distinct cryptographic concerns:

### 6.1 Message encryption — MLS only

All messaging is MLS (RFC 9420) via **openmls**, used directly with no
intermediate FFI (ADR-0001, ADR-0006). No Olm/Megolm, no Signal
protocol, no custom primitives, no security-disabling feature flags
(CLAUDE.md "MLS Protocol"). Perfect Forward Secrecy and Post-Compromise
Security are not optional.

### 6.2 Encryption at rest — whole-DB SQLite3MC (every target)

On every target — native since Step 90, the browser since Step 91
(ADR-0022) — every local database is encrypted whole-file with
**SQLite3MC** (SQLite3 Multiple Ciphers) in its `chacha20` scheme —
ChaCha20-Poly1305, sqleet format, a 16-byte Poly1305 tag per page
(ADR-0021, `rules/0021-sqlite3mc-engine-chacha20.md`; supersedes the
SQLCipher engine of ADR-0013). The engine is the vendored amalgamation
(`vendor/sqlite3mc/`, pinned by version + SHA-256 in its `PROVENANCE.txt`),
compiled by the `libsqlite3-sys` of the SchwarzDigits rusqlite fork, which
Kupferdraht takes as a git dependency by tag together with the refinery
and openmls_sqlite_storage forks on it (ADR-0042) — no root `[patch]` a
consumer would have to repeat, no OpenSSL, no system SQLite, no
pkg-config anywhere in the build. Each database file has its **own**
independently generated key (no key sharing):

| Database | Holds | Keystore id |
|---|---|---|
| `kupferdraht.db` | per-account app state (rooms, timeline, configs) | `kupferdraht.<home-id>.account-<id>` |
| `mls.db` | openmls group state (`openmls_sqlite_storage`) | `kupferdraht.<home-id>.account-<id>-mls` |
| `global.db` | device-level state | `kupferdraht.<home-id>.global-db` |

`<home-id>` is the home's id — `base/home.id`, 32 lowercase hex, created
once under the home lock on the first open (Step 93, ADR-0025). The
keystore is device-wide, the home is the installation unit: scoping
every id per home is what keeps two homes on one keystore from sharing
a key, from racing a first key creation, and from evicting each other's
login cookie (§6.3).

Keys are **raw 32-byte keys** (`PRAGMA key = "x'<64 hex>'"`): no KDF,
no salt, no HMAC subkey in the trust path — the 256-bit key is the whole
security parameter. The pragma order is SQLite3MC's, parameters BEFORE
the key: `cipher = 'chacha20'` → `plaintext_header_size = 32` → `key`
(`storage::apply_whole_db_key`, the one keying site for all three
databases). The 32-byte plaintext header keeps the SQLite magic and WAL
bytes readable so iOS can identify a WAL-mode database in a shared
container; everything after it, page 1 included, is ciphertext. The
key is then **verified by a first read** of `sqlite_master`, so a wrong
key fails at open as a recoverable `KupferdrahtError::Storage` — never
mid-migration, never as a panic. `journal_mode = WAL` follows the keying.
The linked engine is pinned at runtime, not trusted from the build
(`tests/step_90_sqlite3mc_engine.rs`: `sqlite3mc_version()` against the
vendored release, the at-rest byte layout, an encrypted-header control);
`Store::storage_engine` / `Client::storage_engine` report engine, cipher
scheme and header mode off the live connection (`StorageEngineInfo`,
the `kupferdraht-cli status` storage line).

**Accepted deviation (ADR-0021):** ChaCha20-Poly1305 is not on BSI
TR-02102-1's lists (the guideline recommends AES only); the defence is
RFC 8439 / TLS 1.3 / WireGuard state of the art, and SQLite3MC's
`sqlcipher` scheme (AES-256-CBC + HMAC-SHA512) stays compiled in as the
documented fallback — a new decision plus a database rewrite, not a
consumer knob. See THREAT_MODEL.md G3.

**In the browser the same store runs over sqlite-remote-vfs** (Step 91,
ADR-0022; the VFS since Step 111, ADR-0035 — rules `0022` and `0035`):
`sqlite-wasm-rs` compiles the vendored amalgamation for wasm32 (the root
`.cargo/config.toml` points `SQLITE_WASM_RS_SOURCE_DIR` at it) and rusqlite
reaches it through the wasm arm of the SchwarzDigits rusqlite fork's sys crate (ADR-0042). `storage/browser.rs` registers
one `sqlite-remote-vfs` per database — the VFS allows one database per
registration and one connection per database — in an async *prepare* step
before the synchronous open, which names that VFS's SQLite3MC encrypting
layer explicitly (`storage::open_connection`, the one open of `kupferdraht.db`,
`mls.db` and `global.db`). Step 111a uses the VFS's local mode: each
database is an IndexedDB database `sqlite-remote-vfs-local/<home-id>/
kupferdraht/…`, one Web Lock per database. In the **server mode** (Step 111b,
`storage/remote/`) an account's `kupferdraht.db` and `mls.db` live on a
`sqlite-remote-server` (`global.db` stays local): `remote/login_key.rs`
holds the account's Ed25519 storage-login key (derived from its seed, the
server's subject), `remote/token_exchange.rs` buys a storage token for it
with a Wire session (`remote/wire_session.rs`: the login's token, then the
client's refreshing session), `remote/token_cache.rs` serves it to the
VFS's synchronous token source, and `remote/server.rs` registers each
database with a signer over the key — never the key — and an instance id
stable per home and account (`remote/instance_id.rs`, Step 111d: a reload
takes its own lease back), and renews the token at half its lifetime. `manager::Kupferdraht::resume` renews the Wire session
with the browser's cookie (`POST /access` without `client_id`) before the
databases open, without creating them. Every operation on either database
(`SqliteStore::lock()`, the MLS provider's transaction entry) goes through
`storage::check_database` first: another instance's takeover stops the
instance (`onEvicted("device")`) — since Step 119 at once, through the
VFS's takeover event (`storage::browser::database_taken_over`), and an
error that met a takeover surfaces as `Evicted`; the evicted instance's
sync loop ends, so its push socket cannot close the new browser's — a
refused token is
`StorageAccessDenied`, and while the server is unreachable a cheap read
lets the VFS heal the database in place — it reconnects in the background —
or the operation fails at once with `ServiceUnavailable`; the account's
health (`remote/health.rs`) turns that into `online` / `reconnecting` for
`onStorage` (Step 113, ADR-0038). Since Step 120 (ADR-0040) the storage
server keeps one **slot** per user — the user's one remote device, the
key that may open the databases, labelled with its Wire client id
(`storage/remote/slot.rs`, the VFS's `claim_slot` / `set_slot_label` /
`delete_slot` through `storage::browser`): a login without the seed asks
`GET /v1/slot` (with the PIN token) for `recoveryExpected`; a new setup
claims the slot — replacing another device and deleting its databases —
retires the replaced device's Wire client, and labels the slot once its
own client is registered (again at every resume, so an interrupted setup
heals); an open the slot no longer admits (`SQLITE_PERM`) is
`DeviceRetired`, and `resume` then forgets the browser's keys of the
account; `purgeDevice` releases the slot. Recovery data names the device
(format version 2; version 3 since Step 124 adds the seed's receipt key)
and is refused as `oldDevice` when the slot names another. The browser keeps SQLite's rollback
journal, so a commit the outage interrupted is rolled back. Differences from native: a
**dedicated Web Worker in a cross-origin isolated page** is mandatory (the
VFS blocks on a `SharedArrayBuffer`; the open fails closed elsewhere — no
in-memory fallback), the journal is a rollback journal (no shared memory,
no WAL), and **one active instance per origin** (Step 92, ADR-0023):
`storage/browser.rs` takes the Web Locks lease `kupferdraht:instance` before any
database; another tab's open fails closed, an explicit takeover evicts the
holder and takes each database's lock over (every store operation there
fails as `Evicted` via the one `lock()` accessor, and `onEvicted` gets the
reason `"tab"`), and its embedder terminates that worker. **Many tabs are served by that one instance** (Step 94,
ADR-0026, `bindings/web/kupferdraht-tabs/`): every tab's page keeps a
candidate worker queued on the same lease — the browser's FIFO grant on
a dead holder's release is the election — and the worker that wins
opens Kupferdraht by the hand-off `Kupferdraht.open({ takeOver: true })` (a lock
is not reentrant within an agent, so the Rust instance steals the lease
from the candidate's own grant callback) and becomes the **leader**;
the other tabs are **thin clients**: their `KupferdrahtProxy` sends
`{tabId, requestId, target, method, args}` on the `BroadcastChannel`
`kupferdraht:tabs`, the leader answers on `kupferdraht:tabs/<tabId>` and
broadcasts its sync loop's events (`message`, `typing`, `expired`, the
per-pass `sync` summary) to all. A thin tab has no store of its own; it
pulls through the proxy. On a leader change an idempotent call in
flight is re-issued, a mutation in flight fails `LeaderChanged` and is
never resent.
The keystore stays `IndexedDbKeyStore` (IndexedDB + WebCrypto — a
non-extractable `CryptoKey` lives only there). The pre-ADR-0022 IndexedDB
record store with per-record AES-256-GCM and plaintext index keys is gone;
existing browser data is not migrated (test-only deployment). The recovery data (Step 112, ADR-0035) lives in `recovery/`: `pin`
(NFC, the rules for a new PIN), `oprf` (the client side of RFC 9497,
`P256-SHA256`), `pin_service` (`POST /v1/evaluate`, since Step 124
`POST /v1/refund`), `receipt` (Step 124: the seed's ECDSA P-256 receipt
key — HKDF, then the OPRF's `DeriveKeyPair` — and the receipts that give a
successful attempt back), `seal` (Argon2id, HKDF, AES-256-GCM over the
seed), `data` (the versioned format whose header carries the account id's
hash, since Step 120 the device's client id and since Step 124 the receipt
key), `verifier` (the local PIN check) and
`flow` (sealing and opening for an account against the token exchange's
PIN token and the PIN service) and `account` (Step 112b: the state,
`enable`, `changePin`, `verifyPin` of `Client.recovery()`, with the state
in the account's `kupferdraht.db`). `PendingLogin::recover` opens recovery data
with the login's token, stores the seed durably and continues on the
registration path; the manager keeps the recovery services (token
exchange + PIN service, from `Kupferdraht.open`'s `pinServiceUrl`) beside its
keystore and home. Leaving a browser (Step 114) is `manager/purge.rs`:
`logout` with `purgeDevice` / `purgeLocal` runs the native logout first (a
refused password changes nothing), then closes the account's two databases
(`Store::close` and `MlsClient::close_storage` swap each connection for an
empty in-memory one — the VFS deletes only a closed database), deletes them
where they are the device's (the storage server with `purgeDevice`, the
local mode with `purgeLocal`), forgets seed, keys and registry entry with
`purgeLocal`, and drops the client from the manager's cache.

### 6.3 Key storage — the keystore boundary

The `SecureKeyStore` trait (`keystore/`) is a **raw secret store** —
`get_secret` / `set_secret` / `delete_secret` over the raw 32-byte DB
key directly (`DbKey`, exactly 32 bytes — the SQLCipher-era 48-byte
`key ‖ salt` blob is rejected, ADR-0021 I3); the former KEK wrap/unwrap
envelope was removed
(`open.md` §53). Backends: `KeyringStore` (desktop default — OS
keyring), `IndexedDbKeyStore` (browser built-in, IndexedDB +
WebCrypto; it serves the synchronous trait from memory and persists in
the background — every write is tracked, and the key-creating paths
await `keystore::flush_pending_writes` before a database is created
with a new key, Step 104), `FileKeyStore` / `InMemoryKeyStore` (dev/CI — the file
store is loud on construction and writes atomically, Step 77); mobile
embedders implement the trait from Kotlin/Swift via the UniFFI
callback — the one sanctioned callback across the otherwise
boundary-free core (ADR-0002). Sensitive values in Rust are zeroized
via `Zeroizing` / `ZeroizeOnDrop` (THREAT_MODEL.md G4).

**Ids are home-scoped (Step 93, ADR-0025).** `keystore::key_id` is one
function family taking the home id — `global_db(home_id)`,
`account_db(home_id, user)`, `account_mls(home_id, user)` — used by
every open, by the one-time adoption and by the Step-78 purge alike.
Each `KeyId` also names its pre-Step-93 keystore-wide form; inside the
locked first-creation path (`storage::load_or_create_key_locked`) a
legacy entry is adopted under the home-scoped id **only when the
database it opens already exists** (a fresh database always gets a
fresh key), and is never deleted — no home knows whether a sibling home
on the same keystore still needs it. An existing database with no key
under either id is `KupferdrahtError::KeyUnavailable` — a key is generated
only for a database that does not exist yet, never as a replacement
(Step 96, ADR-0029). **Where a new key comes from (Step 110, ADR-0035):**
a home carries an account key source. Natively it is `Random`; the
browser's home is `Seed`: a missing key of an account is HKDF-SHA-256 of
the account's seed (`storage::account_keys`, labels
`kupferdraht/v1/db-key/kupferdraht` and `…/mls`), stored under its native id, and
without the seed its creation is `KupferdrahtError::RecoveryDataRequired`.
Device keys (`global-db`) stay random. A derived key is deterministic, so
the ADR-0029 refusal applies to an account key only without the seed. The
seed is a keystore entry of its own, `kupferdraht.<home-id>.account-<user>-seed`.
In such a home the login authenticates and opens nothing
(`client::builder::opens_at_login`, Step 111c): `registerDevice` opens the
account's storage — with the seed here, or after a first setup
(`manager::PendingLogin`'s `first_setup` module, compiled for wasm and
tests) created it durably — so a takeover of databases another browser
holds (`registerDevice` / `resume` with `takeOver`) never needs a second
login. The login cookie label
`kupferdraht-<home-id>` (`KupferdrahtHome::cookie_label`) derives from the same
id and replaces the Step-41 per-install label that lived in the
keystore (`kupferdraht.install-id`, retired, left in place); the login
records it in `SessionRecord.cookie_label`. The home id itself is an
identifier, not a secret (THREAT_MODEL.md); on wasm it is the keystore
entry `kupferdraht.home-id` (Step 111a — it lives exactly as long as the keys
whose ids carry it) and the namespace of the local databases; the
per-origin IndexedDB keystore makes the keystore-id namespace redundant
there — kept for one contract on every target.

### 6.4 Proteus prekeys — registration only

The Wire backend requires Proteus prekeys at `POST /clients` even for an
MLS-only client. Kupferdraht generates them once (Ed25519 keys,
Wire-exact CBOR), uploads them, and never runs the Double Ratchet
(ADR-0010).

## 7. Storage model

`storage/` exposes a `Store` trait (bound: `MaybeSendSync` — `Send +
Sync` on native, empty on wasm32) with one shipped backend on every
target: SQLite (`storage/sqlite/`, with `refinery`-style numbered SQL
migrations under `storage/migrations/`) — a file under `base_dir` on
native, a database on sqlite-remote-vfs in the browser (Step 111,
ADR-0035; in IndexedDB in the local mode).
Shipped migrations are immutable and SHA-pinned (Step 18b). A migration
run is atomic (Step 87): every pending migration plus its
`refinery_schema_history` row commits in one transaction (`set_grouped`),
so a crash mid-run leaves the file exactly as it was — never a migration
applied but unrecorded, which the next open would re-apply. The current
schema version is **V30**
(`V30__step_112_recovery_state.sql` — `recovery_state`, one row: when the
current recovery data was made and the PIN's local verifier, so a browser
that restored the account knows its recovery data exists; V29
`V29__step_95d_room_devices.sql` — `room_devices`, the device set
last observed per room: the baseline Step 95d diffs a room's MLS leaves
against on every epoch change (`client/device_watch.rs`), persisted so a
change made while no client ran is still reported; cleared by the
Step-40 new-device wipe; V28
`V28__step_79_ephemeral_ttl.sql` — `timeline_events.ephemeral_ttl_ms`,
the TTL an own ephemeral send chose, so a `failed` row can be resent
under the same timer; V27 `timeline_events.expires_at_ms`,
the ephemeral timer stamp, plus a partial index over live ephemeral
rows for the sweep; V26 `room_members.role`, the neutral
`administrator`/`member` vocabulary behind `RoomMemberRole`;
V24 key-package check, V25 device-local read markers). Per ADR-0012,
feature-config and user-property tables are intentionally **not**
field-encrypted (the whole-DB encryption layer already protects them, and
they are policy-/user-public).

**Case state is fold-on-read (Step 72b, ADR-0018)** — deliberately
*no* materialized table: `Room::case()` folds the room's
`timeline_events` rows (`kind='case'`; the body carries the snapshot
JSON + the receive-time `sender_was_admin` flag) on every read, so
redaction needs no rebuild hook and no cache can go stale. Cases have
no cross-room views in v1; if those ever land, a cache becomes its
own step (the task cache below is the template).
The task-list marker (Step 75, ADR-0020) follows the same pattern:
`Room::is_task_list()` folds `kind='task_list_marker'` rows on read
(any-member, last write wins in event order) — no materialized
column, no rebuild hook.

**Materialized task cache (Step 60c, ADR-0014).** Tasks are
event-sourced (`timeline_events` rows with `kind='task'`); the fold —
LWW per `task_id` — is materialized into the `tasks` table
(PK `(room_id, task_id)`, indexes on `(assignee, status)` and
`creator_user_id`) so per-room lists and the cross-room views
(`my_tasks`, `delegated_tasks`) are index-backed reads, never a
timeline re-fold. Maintenance is fold-on-write: send and inbound
dispatch upsert the row (first-writer-wins on `creator`/`created_at`);
a redacted task message triggers a per-room rebuild from the surviving
rows; `Client::load_rooms_from_store` backfills the table once after
the V23 migration (a populated cache short-circuits via `COUNT`). The
`Store` trait ships the task methods with default no-op bodies for
custom backends; the shipped SQLite store implements them on every
target (the browser included since Step 91).

The V24 migration (Step 63) adds
`sync_state.last_key_package_check_at_ms` — the 24-h throttle stamp
for the sync-surface key-package pool check (`client/key_packages.rs`);
stamped before each attempt so a failing endpoint is not hammered per
iteration.

The V25 migration (Step 67) adds `room_read_markers` — the
device-local read position per room (its own table because `rooms`
is replace-managed by every metadata sync). Unread counts derive
from `timeline_events` against this marker
(`storage/sqlite/read_markers.rs`); the cross-device read position
rides the self-conversation in a later step.

**Two processes on one home (Step 85, native).** Every native
connection carries `busy_timeout = 5000` and an explicit
`synchronous = FULL` (`storage::set_multi_process_pragmas`, on all
three open paths: `kupferdraht.db`, `global.db`, `mls.db`), so a second
process's write waits out a writer instead of failing `database is
locked` — which also needs every writing transaction to start
`IMMEDIATE` (Step 97d): a `DEFERRED` one that reads first fails at once
when another process commits while it waits. Two critical sections cannot be shared through the database
and are guarded by an advisory file lock (`storage/lock.rs`,
`std::fs::File::try_lock` — released by the kernel with the process):
the token refresh + persist (Wire invalidates the `zuid` on every
`/access`; under the lock the persisted session is re-read and a
rotation another process persisted is **adopted** instead of rebound —
`client/token_refresh.rs`, `client/resume.rs`) and the first-time
creation of a database key (`storage::load_or_create_key_locked`:
lock-free fast path for an existing key, lock + re-check only when the
key is absent). Layout: `base/kupferdraht.lock` (the registry key) and
`base/accounts/<id>/kupferdraht.lock` (the account's keys and its token
refresh) — per home, per account, never machine-wide. Fail-fast: a
held lock is `KupferdrahtError::AccountLocked`, retryable; no blocking, no
process-lifelong lock. The browser needs none of this — one instance
per origin (ADR-0023). Since Step 93 the home lock also guards the
creation of `base/home.id`, and keystore ids carry the home id
(ADR-0025, §6.3): two *different* homes on one keystore never contend
for one entry, which closes the residual Step 85 left (open.md
gap-B.34) — the lock now covers every first-time creation there is.

**One transaction per MLS operation (Step 97c).** openmls writes one
record per statement through `openmls_sqlite_storage` — a commit merge
is about a dozen, the group state (and with it the pending commit)
first — and leaves atomicity to the application. `KupferdrahtProvider`
(`encryption/provider.rs`) therefore owns the `mls.db` connection and
runs every `MlsClient` operation inside one transaction: writing
operations in `BEGIN IMMEDIATE … COMMIT` (`with_write_tx`), multi-row
loads in a deferred snapshot (`with_read_tx`); openmls gets a
per-operation provider whose storage borrows that connection. An
error or an unwind rolls the whole operation back — a failed merge
keeps the old epoch and its pending commit, a failed Welcome keeps
its key package — and the write lock serialises single operations
across processes and `Client`s (the waiter uses `busy_timeout`). Flows
spanning several operations and network I/O (stage → upload → merge)
are not serialised by this; that is Step 97d.

**Own commits and the drain (Step 97d, ADR-0031).** Every own commit
flow (invite, kick, room / DM establishment, the removal flush) runs
stage → upload → merge/clear under its group's async lock
(`client/commit_flow.rs`, `Client::stage_and_upload_commit`).
`process_inbound_message` sorts a message by epoch before openmls sees
it: older than the group → `StaleEpoch` (skipped, never healed); the
next epoch while the group has an own pending commit →
`AwaitsOwnCommit`. The drain stops before such a message (the cursor
stays), waits for this process's flow and drains again — or, when the
flow belongs to another `Client` or process, leaves the message to a
later drain; after 60 s the pending commit counts as abandoned, is
cleared, and the message heals as before. On `/v14/events` the frame
stays unacked and the connection is cycled for its redelivery.

**When a room heals (Step 106, ADR-0034).** Healing — discard the local
group, rejoin by external commit — needs evidence the epoch number of an
undecryptable message cannot give. A message from a later epoch queues
`HealRoomIfBehind`: after the page, `Client::backend_epoch_ahead` reads
the conversation's current epoch from the backend
(`client/epoch_gap.rs`), and the room rejoins only when it is ahead of
the local group; otherwise the message is reported and skipped. A
missing local group (`InvalidInput`) still heals (`HealRoom`). A storage
error while processing an MLS message never heals — it aborts the
notification for a retry like a failed persist. A Remove of our own leaf
drops the local group only when the external sender proposed it; a
member's such proposal holds back the removal flush until another member
commits it.

**Persist before the MLS commit (Step 97e, ADR-0032; per page since
Step 109, ADR-0037).** Decrypting deletes the message key, so the MLS
state an inbound page changes must not commit before its data is in
`kupferdraht.db`. The drain opens one window per fetched page
(`sync/window.rs::PageWindow`): the MLS write gate, one `mls.db` write
transaction (`KupferdrahtProvider::begin_window`) and one `kupferdraht.db` page
transaction (`Store::begin_page`). Every notification starts with a
savepoint in both databases and in the outbox of held notifications
(`sync/outbox.rs`); the inbound operations (`process_inbound_message`,
`absorb_welcome`) join the MLS window (`with_inbound_tx`), the store's
per-event transactions of Step 105 become savepoints
(`storage/sqlite/write_tx.rs`). At the end `kupferdraht.db` commits, then
`mls.db`, then the held notifications are emitted, then the cursor
advances (`sync/page.rs`). A notification that fails to persist, or is
deferred behind an own pending commit, rolls back to its savepoint:
everything before it commits and the cursor stops before it. A failed
commit rolls the whole page back; it replays with its keys intact.
While a page transaction is open, every write to `kupferdraht.db` from
outside the drain waits (`SqliteStore`'s page gate — an async
`RwLock` the page holds exclusively, every write method shares; the
drain's own task passes it through a task-local marker,
`storage/page.rs`); reads never wait. On remote storage a page costs
three commits instead of three per notification. Handlers are idempotent per message id and
notify only on a fresh effect; a replay after the MLS commit meets an
older epoch or a consumed key and is skipped quietly
(`AlreadyDecrypted`). No other MLS write may land in an open window:
every MLS write outside inbound dispatch takes the `Client`'s MLS
write gate (`client/mls_gate.rs`), and a write that skipped it fails
closed while a window is open. Reads take no gate.

## 8. Identifiers

Identifiers are plain `&str` / `String` on the public API, with **one**
typed exception: `identity::UserId` (the qualified user id, returned by
`Client::user_id()`). The broader typed-ids plan (`kupferdraht::ids::*` with
`RoomId` / `DeviceId` / `MessageId` newtypes) was rejected
(CHANGELOG.md §49) and never shipped. User and room ids are
`<uuid>@<domain>`; device id is Wire's hex `client_id`; message id is a
UUID. Full table in `docs/library.md` §Identifiers.

## 9. Where else to look

| Question | Document |
|---|---|
| Why a decision was made | `docs/adrs/` (+ condensed in `rules/`) |
| The exact Wire HTTP contract | `docs/backend-spec.md`, `reference/wire.swagger.json` |
| Live-observed Wire endpoint behaviour | `docs/wire/api-notes.md` |
| Where Wire's MLS deviates from RFC 9420 | `docs/wire/mls-deviations.md` |
| The public API surface | `docs/library.md`, `docs/bindings.md`, `docs/cli.md` |
| Security goals & trust model | `docs/THREAT_MODEL.md` |
| Roadmap status | `PLAN.md` |
| Observations log | `docs/observations.md` |

## 10. Maintenance

This document is part of the project's living documentation and is
covered by the CLAUDE.md Freshness rule: a change to a layer boundary, a
module's responsibility, a send/receive/sync/storage data flow, or the
storage schema updates this file **in the same commit**. Out-of-date
architecture documentation is a defect, not a follow-up.
