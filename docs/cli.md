# `kupferdraht-cli` reference

Complete command reference for the `kupferdraht-cli` binary — Kupferdraht's
thin CLI wrapper around `kupferdraht-core` for live testing and
single-shot operations against a Wire-compatible backend.

This document is the canonical CLI manual; `README.md` only
covers a quickstart. Every change to a command's flags, output, or
exit semantics updates this file in the same commit per
CLAUDE.md's Freshness rule.

---

## Build

```bash
cargo build --release -p kupferdraht-cli
```

The binary lands at `target/release/kupferdraht-cli`. Examples below
assume you invoke it via that path; aliasing `kupferdraht-cli=...` in
your shell rc is fine.

## Storage layout

Kupferdraht persists session and MLS state under a per-user data
directory:

| File          | Contents                                                         |
|---------------|------------------------------------------------------------------|
| `kupferdraht.db`   | SQLite — sessions, devices, rooms snapshot, timeline events, sync cursor. Whole-DB encrypted via SQLite3MC in its `chacha20` scheme (ChaCha20-Poly1305, raw 32-byte key, 32-byte plaintext header), keyed per account from the OS keystore (ADR-0021). |
| `mls.db`      | openmls's storage — group state, leaf signatures, key packages, ratchet state. Same engine and scheme, its own key. |
| `global.db`   | device-level account registry (`accounts`); same engine and scheme, keyed under the home-scoped `kupferdraht.<home-id>.global-db` id. |
| `home.id`     | the home id (Step 93, ADR-0025): 32 lowercase hex, plaintext — an identifier, not a secret. Created once on the first open (under the home lock); every keystore id of this home (`kupferdraht.<home-id>.…`) and the login cookie label `kupferdraht-<home-id>` carry it. **Never edit, delete or copy it**: a new id orphans every key of the home, a copied home shares keys and label with the original. |
| `kupferdraht.lock` | advisory cross-process lock (Step 85): one in the home (the registry key's and the home id's first creation), one per `accounts/<id>/` (that account's key creation + token refresh). Holds the holder's pid; stays on disk, holds no lock once the process is gone. |
| `sync.lock`   | per `accounts/<id>/` (Step 97d, ADR-0031): one sync dispatch per account at a time across processes — held while a fetched page is processed, waited for up to 5 s. Same mechanics as `kupferdraht.lock`. |

`kupferdraht-cli status` prints the engine actually linked and in effect as
its last line, e.g. `storage: SQLite3 Multiple Ciphers 2.5.1 · chacha20 ·
plaintext header 32`. Databases written before Step 90 (SQLCipher format)
are not migrated — start from a fresh `KUPFERDRAHT_HOME`.

Default location:

| Platform    | Path                                                |
|-------------|-----------------------------------------------------|
| macOS       | `$HOME/Library/Application Support/kupferdraht/`         |
| Linux       | `$XDG_DATA_HOME/kupferdraht/` (or `$HOME/.local/share/kupferdraht/`) |
| Windows     | `%LOCALAPPDATA%\kupferdraht\`                            |

Override the entire base path via `KUPFERDRAHT_HOME=/some/other/dir`.
`home.id`, `global.db` and `kupferdraht.lock` live directly under that
directory, the per-account files under `accounts/<id>/`; nothing else
is written there by Kupferdraht. Two processes may share a home (Step 85):
the databases wait for each other's writes (`busy_timeout` 5 s), and
the two critical sections that cannot be shared — the token refresh and
a first-time key creation — fail fast with exit code 3 when the other
process is inside them; just retry. Two homes may share a keystore
(Step 93): each keys its databases under its own home id and logs in
under its own cookie label, so a second home of the same user — a
second `KUPFERDRAHT_HOME` on one `KUPFERDRAHT_DEV_KEYSTORE`, or the CLI beside
kupferdraht-tui on one keychain — no longer evicts the first one's session
(observations §83). A home keyed before Step 93 adopts its keys on its
first open by a Step-93 build (the keystore-wide entries stay behind —
a manual cleanup) and takes its own label at its next `login`.

## Environment variables

| Variable         | Effect                                                                |
|------------------|-----------------------------------------------------------------------|
| `KUPFERDRAHT_HOME`    | Override the storage directory shown above. Useful for sandboxed test runs. |
| `KUPFERDRAHT_DEV_KEYSTORE` | Directory for the file-backed dev keystore instead of the OS keyring (the macOS escape hatch for unsigned `cargo build` binaries; also the sandboxed-gate recipe, `docs/LEARNINGS.md` §5.12). **Plaintext key files** — every invocation prints a `WARN` naming the directory (Step 77). The value is a directory, never `1`. |
| `RUST_LOG`       | `tracing-subscriber` filter. Default `warn`. Examples: `RUST_LOG=info`, `RUST_LOG=kupferdraht=debug,kupferdraht_core::wire=trace`. |

## Exit codes

| Code | Meaning                                                                            |
|------|------------------------------------------------------------------------------------|
| 0    | Success.                                                                           |
| 1    | A `KupferdrahtError` from the library (network failure, backend rejection, MLS error, malformed input, missing session, …). Stderr carries `kupferdraht-cli error: <message>`. |
| 2    | Login-specific input error: the password could not be read or was empty.           |
| 3    | `KupferdrahtError::AccountLocked` (Steps 85, 97d): another Kupferdraht process is inside this account's critical section (token refresh, first key creation) or kept the sync lock for more than 5 s — retryable; stderr names the holder's pid when known. |
| 4    | `KupferdrahtError::KeyUnavailable` (Step 96): a database exists but the keystore has no key for it — nothing was written. Retry after unlocking the keystore; if the key is gone for good, remove the account directory and log in again as a new device (stderr names the database). |

Stdout is reserved for command output (CLI is a tool — its stdout
IS its interface). Diagnostic logging from `tracing` goes to stderr.

## Backend selection

The backend is chosen at `login` / `register` / `activate` time;
subsequent commands read it out of `kupferdraht.db`. There is exactly
one way to choose, and it is required: `--api-url`. There is no
default backend and no named preset: a session-creating command
without `--api-url` refuses to run. Earlier versions offered
`-b production|staging` presets for Wire's hosted service; they were
removed before publication because Wire's Terms of Use restrict
third-party clients there (README "Known limitations").

`--api-url <https://…>` points at a self-hosted deployment of
wire-server, Wire's open-source backend (API v14), or at another
backend that speaks the same API (backend-spec §20: changing the
base URL is all a client needs). The WebSocket URL is derived by
Wire's deployment naming convention
(`https://nginz-https.<domain>` → `wss://nginz-ssl.<domain>`; hosts
outside that convention keep their host) and can be overridden with
`--ws-url <wss://…>`. The API URL must be a bare `https://` origin
without a path.

```bash
kupferdraht-cli login --api-url https://nginz-https.chat.example.org -e you@example.org
```

---

## How sync, join, and receive fit together (read this first)

Kupferdraht's commands fall into three orthogonal layers. Knowing
which layer a command operates on tells you when to call it
and what it depends on.

| Command(s)                     | Layer            | What it does                                                                                       | Wire endpoints                                                                            |
|--------------------------------|------------------|----------------------------------------------------------------------------------------------------|-------------------------------------------------------------------------------------------|
| `sync`                         | **Metadata**     | Fetches Wire's room list + member profiles. Updates the in-memory cache + `rooms` / `room_members`. **Does not touch MLS.** | `POST /v{N}/conversations/list-ids` + `POST /v{N}/conversations/list` + `POST /v{N}/list-users` |
| `rooms-join-all`               | **MLS bootstrap** | For each cached room *without* local MLS state → join via external commit; the never-established self conversation (epoch 0) is **established** from this device instead (Step 86). Idempotent (skips rooms whose MLS group already exists). | `GET /v{N}/conversations/{d}/{id}/groupinfo` + `POST /v{N}/mls/commit-bundles` per room; self room at epoch 0: self-claim `POST /v{N}/mls/key-packages/claim/{d}/{u}` + `POST /v{N}/mls/commit-bundles` |
| `rejoin-all`                   | **MLS recovery** | For each cached room *with* local MLS state whose `own_leaf` belongs to a **prior** `device_id` → re-join via external commit (Step 7e). | same as `rooms-join-all`, but only for stale rooms                                        |
| `sync` / `sync-stream`         | **One iteration / loop** | `sync` drives one `Client::sync_once` — rooms-metadata refresh + one notifications drain + cursor advance, in a single iteration. `sync-stream` runs the same loop continuously with `--idle-backoff` between empty drains, refreshing the rooms metadata only on its first iteration, after an event changed the room list, and when it is older than 5 minutes (Step 118); `--transport websocket` (Steps 62/73) switches it to live push — the concrete socket is negotiated per connection (`/v14/events` when the server enables ConsumableNotifications, the legacy `/await` socket otherwise, whose frames only trigger a REST drain (ADR-0030); the startup banner prints the choice). | `GET /v{N}/notifications` · `GET /v{N}/events` or legacy `GET /await` (WS, `--transport websocket`, negotiated per ADR-0019) |
| `send` / `room-create` / etc.  | **App actions**  | Encrypt + POST a `GenericMessage` (`send`); create / invite / kick / leave / rename rooms.        | various                                                                                   |

`sync` is the prerequisite for everything else: without a
populated rooms cache, `rooms-join-all` and `rejoin-all` have
nothing to iterate over and `send` has no group to encrypt to.
Several commands therefore call `sync_once` internally
(see "Workflows" below for which).

```
       ┌─────────────────────────────────────────────┐
       │  Wire Server (truth)                        │
       │  - Conversation membership (REST)           │
       │  - MLS GroupInfo per room (REST)            │
       │  - Notifications stream (REST polling)      │
       └─────────────────────────────────────────────┘
              ▲                  ▲                  ▲
              │                  │                  │
       ┌──────┴──────┐  ┌────────┴─────────┐  ┌─────┴──────────────┐
       │ sync        │  │ rooms-join-all   │  │ sync /       │
       │ (metadata)  │  │ rejoin-all       │  │ sync-stream        │
       │             │  │ (MLS join /      │  │ (event drain)      │
       │             │  │  re-join)        │  │                    │
       └──────┬──────┘  └────────┬─────────┘  └─────┬──────────────┘
              ▼                  ▼                  ▼
       ┌─────────────────────────────────────────────────────────────┐
       │ Local kupferdraht.db                                              │
       │ - rooms / room_members  ← sync writes                        │
       │ - openmls (mls.db)      ← rooms-join-all / rejoin-all write │
       │ - timeline_events       ← send / sync / sync-stream    │
       └─────────────────────────────────────────────────────────────┘
```

### Workflows

| Scenario | Command sequence | Notes |
|---|---|---|
| **Fresh install** (first login, or after `logout --purge-device`, or `rm -rf ~/.kupferdraht`) | `login` → `sync` → `rooms-join-all` → `sync-stream` | No local MLS state — every cached room is bootstrapped via external commit. |
| **Daily use** (kupferdraht.db preserved, same device_id) | `login` → `sync` → `sync-stream` | Step 7e's reuse path detects the existing device on Wire and rebinds an access token without registering a new client. MLS state is already consistent. |
| **Re-login after device replacement** (kupferdraht.db preserved, but Wire returned a *new* `device_id` because the old client was deleted server-side) | `login` → `sync` → `rejoin-all` → `sync-stream` | Step 7e's recovery path. `rejoin-all` finds rooms whose `own_leaf` belongs to the prior device_id and re-joins each via external commit. |
| **One-shot send** | `login` → `send --id <room>` | Several commands (`send`, `room-invite`, `room-kick`, `room-leave`, `room-create`) call `sync_once` internally so a fresh metadata refresh is not strictly necessary; an explicit `sync` first is still the cleanest pattern. |
| **One-shot receive** | `login` → `sync` | One `Client::sync_once` iteration drains the notification queue + refreshes rooms metadata, then exits. Useful in scripts. |
| **Continuous receive** (long-running) | `login` → `sync-stream --idle-backoff 1` | Blocks until Ctrl-C. Default poll cadence ≤ 2 s. |
| **7-device cap hit at login** | (login fails with `too-many-clients`) → delete a device via Wire web UI → retry `login` | The CLI's `devices` / `device-delete` need an active session, so they cannot recover from this on their own. After regaining a slot via Wire web UI, `device-delete` from the CLI keeps the count manageable on subsequent runs. |

`rooms-join-all` and `rejoin-all` are deliberately separate.
They cover orthogonal recovery scenarios (no local state vs.
stale local state) and the diagnostic output of each tells you
exactly what happened — a single combined command would hide
the distinction.

---

## Commands

The natural order is **set-up → discover → act**:

```
login → status → sync → rooms → room-create / room-join → room-invite → send → sync

# After a device replacement (e.g. fresh install with a copied
# mls.db, or Wire UI manually deleting the prior client):
login → rejoin-all → ...
```

Run `kupferdraht-cli <subcommand> --help` for the same information at
the prompt.

### `login`

Authenticate with email + password, register a new device on Wire,
upload Proteus prekeys + the MLS public key, generate the initial
KeyPackage pool, and persist the session.

```bash
kupferdraht-cli login --api-url https://nginz-https.example.com -e <email>
kupferdraht-cli login --api-url https://nginz-https.example.com -e you@example.com --password-stdin   # piped
```

| Flag                     | Required | Description                                      |
|--------------------------|----------|--------------------------------------------------|
| `--api-url <url>`        | yes      | Base https URL of a self-hosted Wire-compatible backend ("Backend selection" above). |
| `--ws-url <url>`         | no       | WebSocket URL override (`wss://…`); only with `--api-url`, derived by nginz convention when omitted. |
| `-e`, `--email`          | yes      | Wire account email.                              |
| `--password-stdin`       | no       | Read the password from stdin (no TTY prompt). Useful for password-manager pipes and non-interactive runs. |
| `--delete-device <id>`   | no       | On `too-many-clients`: delete this device id to free a slot, then retry the login (Step 24). |

There is **no** `--password` flag by design — secrets in argv leak
into `ps`, shell history, and process listings. Without
`--password-stdin` the CLI prompts on the TTY with `rpassword`
(echo off).

Re-running `login` against the same account creates a **new device**
on Wire, which enforces a per-user device cap of 7. When the cap is
hit, login fails with `too-many-clients` — the CLI then **prints the
registered devices**. Re-run with `--delete-device <id>` to remove one
(freeing a slot) and retry the login in a single invocation:

```bash
kupferdraht-cli login --api-url https://nginz-https.example.com -e you@example.com                       # prints the device list
kupferdraht-cli login --api-url https://nginz-https.example.com -e you@example.com --delete-device <id>  # frees a slot + retries
```

### `register`

Create a fresh account. POSTs `/register` against the chosen
backend and prints the pending `user_id` plus the email Wire will
deliver the activation code to. Nothing is persisted locally — the
in-memory `PendingClient` does not survive the process. Run
`kupferdraht-cli activate` once the email arrives to complete the loop.

```bash
kupferdraht-cli register --api-url https://nginz-https.example.com -e alice+test@example.com -n "Alice"
kupferdraht-cli register --api-url https://nginz-https.example.com -e alice+test@example.com -n "Alice" --password-stdin
```

| Flag                     | Required | Description                                                      |
|--------------------------|----------|------------------------------------------------------------------|
| `--api-url <url>`        | yes      | Self-hosted Wire-compatible backend ("Backend selection" above). `--ws-url` optional. |
| `-e`, `--email`          | yes      | Email to register; Wire delivers the activation code here.       |
| `-n`, `--name`           | yes      | Display name for the new account.                                |
| `--password-stdin`       | no       | Read the password from stdin (no TTY prompt).                    |

Same no-`--password`-flag rule as `login`: secrets in argv leak.

### `activate`

Redeem the email-delivered activation code and complete the post-
registration handshake. POSTs `/activate`, then drives the same
login pipeline `kupferdraht-cli login` runs (device registration, access-
token rebind, MLS key-package upload) and persists the full session
state to `kupferdraht.db`. After this command succeeds, `rooms`, `sync`,
`send`, etc. work immediately.

```bash
kupferdraht-cli activate --api-url https://nginz-https.example.com -e alice+test@example.com -c "ABC123"
kupferdraht-cli activate --api-url https://nginz-https.example.com -e alice+test@example.com -c "ABC123" --password-stdin
```

| Flag                     | Required | Description                                                      |
|--------------------------|----------|------------------------------------------------------------------|
| `--api-url <url>`        | yes      | Same custom backend the `register` call used. `--ws-url` optional. |
| `-e`, `--email`          | yes      | Email used at `register` time. Required explicitly because the CLI does not persist any state between `register` and `activate` invocations. |
| `-c`, `--code`           | yes      | Activation code from Wire's delivery email.                      |
| `--password-stdin`       | no       | Read the password from stdin.                                    |

The password must be re-supplied here because each CLI process
starts with a fresh in-memory state — `register`'s `PendingClient`
(and its `Zeroizing<String>`-wrapped password) is gone by the time
`activate` runs.

### `sso initiate`

```bash
kupferdraht-cli sso initiate --api-url https://nginz-https.example.com -c <sso-code>          # explicit SSO code
kupferdraht-cli sso initiate --api-url https://nginz-https.example.com -e you@example.com      # discover the team default code
kupferdraht-cli sso initiate --api-url https://nginz-https.example.com -c <sso-code> \
    --success-redirect kupferdraht://sso/done \
    --error-redirect   kupferdraht://sso/error
```

Step 17β (interactive SSO). With `-c/--code`, validates it against the backend
(`HEAD /sso/initiate-login/{code}`; an unknown code errors). With `-e/--email`
(mutually exclusive), discovers the backend's **default** SSO code via
`GET /sso/settings` — Wire v14 has no per-domain discovery, so multi-IdP backends
must pass an explicit `--code`. Either way it prints the
`GET /sso/initiate-login/{code}?success_redirect&error_redirect` URL to open in a
browser. After the IdP round-trip Wire redirects to `success_redirect` carrying
the `zuid` cookie — which `sso finalize` (Step 17b) will consume to complete
login. `--success-redirect` / `--error-redirect` default to `kupferdraht://sso/…`.

### `sso finalize`

```bash
kupferdraht-cli sso finalize --api-url https://nginz-https.example.com --cookie <zuid-value>
kupferdraht-cli sso finalize --api-url https://nginz-https.example.com --cookie <zuid> --delete-device <id>   # frees a slot on the 7-device cap
```

Step 17β. Takes the `zuid` cookie captured off the `success_redirect` and mints
the session (`POST /access`, no `client_id`), then registers this device and
persists the account — immediately listable via `accounts` and resumable via
`resume`, exactly like `login`. The 7-device cap is recoverable with
`--delete-device` on the same finalized session (no re-auth).

### `status`

Print the most-recently-persisted session: `user_id`, `device_id`,
`api_version`, `access_token_issued_at_ms`, and key prefixes for
the access token and the MLS public key, the device's `mls_thumbprint`
(Step 95d — Wire's "MLS Thumbprint", 2-character groups); then the home lines (Step 93)
— `home_id` (the id in `$KUPFERDRAHT_HOME/home.id`), `cookie_label` (the
login label in effect for this home, `kupferdraht-<home-id>`) and
`session_label` (the label the persisted session was logged in with;
`—` for a session persisted before Step 93, which still holds the old
shared label on Wire's side until its next `login`) — followed by one
`storage:` line naming the whole-DB encryption engine, cipher scheme
and header mode read off the open store (`storage: SQLite3 Multiple
Ciphers 2.5.1 · chacha20 · plaintext header 32`; a custom
`StorageProvider` that reports no engine prints `storage: (engine not
reported by this backend)`).

```bash
kupferdraht-cli status
```

```text
session:
  user_id:                   f7da3466-…@example.com
  device_id:                 3a1f…
  api_version:               14
  access_token_prefix:       eyJhbGciOiJI…
  mls_public_key_prefix:     MCowBQYDK2VwAy…
  home_id:                   0123456789abcdef0123456789abcdef
  cookie_label:              kupferdraht-0123456789abcdef0123456789abcdef
  session_label:             kupferdraht-0123456789abcdef0123456789abcdef
storage: SQLite3 Multiple Ciphers 2.5.1 · chacha20 · plaintext header 32
```

A `session_label` that differs from `cookie_label` (or shows `—`) is a
two-home diagnosis in one glance: this session was not logged in under
this home's label.

No flags. No network calls. Run this first when something feels
off. An `access_token_issued_at_ms` older than ~15 minutes is no
longer a problem on its own: the next command's `resume` refreshes
the token in place via an `/access` rebind with the persisted
`zuid` (CHANGELOG.md §51). You only need to re-`login` once the `zuid`
refresh cookie itself expires or is revoked.

### `accounts`

List the accounts registered on this device, reading only the
device-level `GlobalStore` (`global.db`) — **no account unlock and no
network calls**. The active account is marked with `*`.

```bash
kupferdraht-cli accounts
```

```text
* user-uuid@example.com
    handle:  @alice
    name:    Alice Example
    server:  https://nginz-https.example.com
```

The registry is populated by `login` (which registers the account, marks
it active, and persists the session via the device manager). Empty until
the first `login`.

### `sync`

One iteration of the matrix-rust-sdk-shaped sync family (Step 14b).
Drives `Client::sync_once(SyncSettings::default())`:

1. Refreshes the access token via `POST /v{N}/access` using the
   persisted `zuid` cookie.
2. Lists conversation IDs (`POST /v{N}/conversations/list-ids`)
   and fetches their metadata
   (`POST /v{N}/conversations/list`).
3. Resolves member display names / handles
   (`POST /v{N}/list-users`).
4. Drains `GET /v{N}/notifications` once, decrypts + dispatches
   every queued event through the registered handler, and
   advances the persisted `last_event_id` cursor.
5. Writes everything back into `rooms` + `room_members` in
   `kupferdraht.db`.

```bash
kupferdraht-cli sync
```

No flags. Idempotent. Output is a one-line summary:

```
sync ok
  rooms synced:           18
  notifications drained:  3
```

Requires a prior `login`. For continuous receive use
[`sync-stream`](#sync-stream).

> **Step 14b note:** The previous `sync-fetch` subcommand has been
> removed. `sync` is now the canonical one-iteration entry point —
> it does what `sync` + `sync-fetch` used to do in two steps.

Since Step 95d `sync` and `sync-stream` also print every change of a
room's device set (`devices changed in <room>` with `+` / `-` lines and
each device's MLS thumbprint) — see `room-devices`.

### `sync-metadata`

Lightweight metadata-only refresh. Drives `Client::sync_metadata()`:
runs the conversations list-ids → list → list-users round-trip
and replaces the local `rooms` + `room_members` snapshot,
**without** draining `/v{N}/notifications`. The
per-event MLS-decrypt loop that `sync` performs is skipped.

```bash
kupferdraht-cli sync-metadata
```

Output:

```
sync-metadata ok
  rooms cached: 18
```

Use over `sync` when you want a cheap freshness check and don't
need queued events processed. Typical scenarios on future
non-CLI bindings: background-startup, battery-conscious mobile
launches, or "verify what's there before deciding whether to
drain". Pair with a later `sync` once foreground attention
arrives. Decision rationale: CHANGELOG.md §46 (three-tier
composability — `rooms` /`sync-metadata` / `sync`).

### `sync-stream`

Long-running polling loop — Kupferdraht's canonical continuous-receive
surface. Repeatedly drains `/v{N}/notifications` and dispatches
events through the registered handler; sleeps `--idle-backoff`
seconds between empty drains. Modelled on
`matrix_sdk::Client::sync()`. Blocks until Ctrl-C.

```bash
kupferdraht-cli sync-stream
kupferdraht-cli sync-stream --idle-backoff 1     # snappier latency, more idle traffic
```

| Flag             | Required | Default | Description |
|------------------|----------|---------|-------------|
| `--idle-backoff` | no       | `2`     | Seconds to sleep when a drain returns zero notifications. Lower values cut perceived receive latency at the cost of more idle HTTP traffic against `/notifications`. |

Output:

```
sync-stream — polling /v14/notifications continuously
  idle backoff:  2s
  press Ctrl-C to stop
[message] room=<room>@<domain> sender=… "hello"
[message] room=… sender=… "ping"
^C
sync-stream stopped
```

Since Step 118 (ADR-0033) `sync-stream` holds the push socket by
default — the socket negotiated per connection (the legacy `/await`
doorbell on current wire-servers), each frame triggering a REST drain;
without a socket it drains anyway, at most once per push pace.
`--transport polling` drains in a loop instead. `--idle-backoff <s>`
sets the polling pause or the push pace; unset, the library's
default applies (10 s for both). See the `sync` /
`sync-stream` table entry above.

### `rooms`

List the rooms previously written to `kupferdraht.db` by `sync`. Reads
the local store only — no network, no authentication. Each room
shows its `unread` count (Step 67 — inbound messages newer than the
device-local read marker; `mark-read` advances the marker, no marker
means everything inbound counts).

```bash
kupferdraht-cli rooms
```

Output (one block per room):

```
- c0ffee01-0000-4000-8000-000000000001@example.com
    type:   group | direct_message | self_room
    name:   Kupferdraht 6b smoke 3 | —          # — for DMs and self_room
    epoch:  1                              # local MLS group epoch
    members (2):
      * a11ce000-…@example.com — Alice Example @alice
        8d1f2c34-…@example.com — Bob Example @bob
```

The `*` prefix marks the local device. For DMs, an extra
`counterpart:` line names the peer.

### `room-create`

Create a new MLS conversation (Step 6a, zero-invitee). The group is
registered server-side via `POST /v{N}/conversations` and the local
MLS state is built at epoch 0 with Wire's backend removal key in the
`external_senders` extension.

```bash
kupferdraht-cli room-create -n "Kupferdraht 6c smoke"
```

| Flag             | Required | Description                                       |
|------------------|----------|---------------------------------------------------|
| `-n`, `--name`   | yes      | Room name (1–256 characters per Wire's schema).   |

Output prints the qualified room id so subsequent commands can
target it. The room is **inert** until a successful `room-invite`
advances it past epoch 0 — application messages at epoch 0 are
rejected by Wire (`mls-protocol-error`, CHANGELOG.md §25).

### `dm-create`

Create — and **establish** — the Wire MLS 1:1 with a peer
(Steps 14a + 68). `GET /v{N}/one2one-conversations/{d}/{u}`
reserves Wire's first-class 1:1 — idempotent server-side, so
calling against an existing 1:1 returns it rather than erroring.
The reservation alone leaves the conversation hidden from
`list-ids` (`docs/wire/api-notes.md`), so the command then
branches: `epoch == 0` → claim the peer's key packages and upload
the establishing Add-commit; `epoch > 0` → join via external
commit; local group already present → plain resume. Ends with a
metadata sync and prints the resulting qualified `room_id`.

```bash
kupferdraht-cli dm-create --peer c0ffee02-…@example.com
```

| Flag       | Required | Description                                                  |
|------------|----------|--------------------------------------------------------------|
| `--peer`   | yes      | Peer's qualified user id `<uuid>@<domain>`.                  |

Output prints `room_id` + `epoch`. Subsequent `send` / `timeline` /
other room commands target the printed id. Multiple `dm-create`
calls against the same peer return the same room — Wire allows at
most one 1:1 per `(self, peer)` pair.

### `dm-show`

Strict local-cache lookup of the 1:1 with a peer (Step 14a). No
network, no authentication. Prints `room_id` + `epoch` on a cache
hit; exits with status 2 on a miss (run `sync` first to populate
the cache, or `dm-create` / `dm-with` to ensure a 1:1 exists).

```bash
kupferdraht-cli dm-show --peer c0ffee02-…@example.com
```

| Flag       | Required | Description                                                  |
|------------|----------|--------------------------------------------------------------|
| `--peer`   | yes      | Peer's qualified user id `<uuid>@<domain>`.                  |

### `dm-with`

"Look up first, create if missing" convenience for 1:1
conversations (Step 14a). Cache hit prints the existing `room_id`
without any HTTP traffic; cache miss falls through to `dm-create`.
Concurrent invocations against the same peer de-duplicate the POST.

```bash
kupferdraht-cli dm-with --peer c0ffee02-…@example.com
```

| Flag       | Required | Description                                                  |
|------------|----------|--------------------------------------------------------------|
| `--peer`   | yes      | Peer's qualified user id `<uuid>@<domain>`.                  |

This is the recommended command for "give me the DM with this user,
whatever it takes" — the dominant caller shape for Wire's
first-class 1:1 endpoint.

### `self-room`

Look up the caller's self conversation (Step 86) — Wire's
notes-to-self room, an MLS group of the user's own devices and the
private cross-device channel (personal task list, read position,
drafts later ride it). Strict local-cache lookup: no network, no
authentication. Prints `room_id` + `epoch` when the last `sync`'s
snapshot carries it; exits with status 2 otherwise.

```bash
kupferdraht-cli self-room
```

No flags. Output:

```
self-room ok
  room_id: <uuid>@<domain>
  epoch:   0
  note:    not established yet — `rooms-join-all` establishes it from this device
```

Epoch 0 means no device has established the group yet; run
`rooms-join-all` (or `room-join --id <room_id>`) — see there for
what "establish" does. Afterwards `send --id <room_id>` and
`task add --id <room_id>` work like in any room, and every other
device of the same user reads them after its own `sync` +
`rooms-join-all`.

### `room-join`

Join one specific existing MLS group via external commit. Fetches
the group's current `GroupInfo` from Wire, runs openmls's external-
commit builder locally, and uploads the resulting commit bundle to
`POST /v{N}/mls/commit-bundles`.

```bash
kupferdraht-cli room-join --id <uuid>@<domain>
```

| Flag       | Required | Description                                                   |
|------------|----------|---------------------------------------------------------------|
| `--id`     | yes      | Qualified room id (`<uuid>@<domain>`) as printed by `rooms`. |

Requires a prior `sync` so the target room is in the cache.
**Not idempotent:** re-running against an already-joined room issues a
fresh external commit — a resync that rebuilds the local group from
Wire's current `GroupInfo` and advances the epoch by one (live
2026-09-24: epoch 3 → 4 on a second `room-join`). `rooms-join-all` is
the idempotent form: it skips rooms whose group is already in openmls
storage.

**Self conversation (Step 86):** when `--id` names the user's self
room and the snapshot shows `epoch: 0`, Wire has no `GroupInfo` yet
(`404 mls-missing-group-info`, live 2026-09-24) — the command
**establishes** the group from this device instead: local group
under the snapshot's `group_id`, the user's other devices Added
(they absorb the Welcome on their next `sync`) or a self-update
commit when this is the only device, then `epoch: 1`. Any later
device of the same user external-joins as usual. A rejected
establishing commit discards the local group so a retry (after a
fresh `sync`) starts clean.

### `rooms-join-all`

Join every room the local device is not yet a member of via external
commit. Skips rooms already in openmls storage with no extra network
traffic. Prints a summary and a per-room failure list.

```bash
kupferdraht-cli rooms-join-all
```

No flags. Use this after a fresh `login` to bootstrap into every
conversation Kupferdraht can see. The self conversation is healed on the
same pass (Step 86): established when it is still at epoch 0,
external-joined when another own device got there first — both
count under `joined`.

### `rejoin-all`

Re-join every room whose persisted MLS leaf belongs to a prior
`device_id` of the same user. Step 7e's recovery surface for the
"re-login changed my Wire client_id, prior-session rooms are
unreachable" case. Idempotent: rooms whose leaf already matches
the current device are reported as `already_current` and skipped.

```bash
kupferdraht-cli rejoin-all
```

No flags. Per-room flow:

1. Skip rooms with no `group_id` (un-`sync`'d) and rooms whose
   `group_id` is unknown to the local openmls storage (never
   joined by *any* device — covered by `rooms-join-all`).
2. Inspect the persisted group's `own_leaf` credential. When the
   identity matches the current device, count as
   `already_current` and continue.
3. Otherwise: drop the stale group state and re-join via the
   unchanged external-commit path.

Output:

```
rejoin-all
  attempted:       20
  already_current: 18
  resynced:        2
  failed:          0
```

Per-room failures are listed under `failed:` with the room id and
the surfaced error text — one stuck room does not block recovery
of the others.

Note that `login` now reuses the persisted device whenever Wire
still has it (Step 7e), so day-to-day flows do not need to call
`rejoin-all`. Reach for it after a deliberate device-replacement
(persisted client deleted via the Wire UI, fresh install with a
copied `mls.db`, or any other case where the active `device_id`
changed under the same `user_id`).

### `room-members`

List the members of a room as derived from the local MLS group's
leaves (post-`room-join`). Resolves display name / handle for every
non-self leaf via `POST /v{N}/list-users`.

```bash
kupferdraht-cli room-members --id <uuid>@<domain>
```

| Flag       | Required | Description                                       |
|------------|----------|---------------------------------------------------|
| `--id`     | yes      | Qualified room id.                                |

Output:

```
room-members
  room_id: c0ffee01-…@example.com
  count:   2
  * a11ce000-…@example.com — Alice Example @alice  [admin]
    8d1f2c34-…@example.com — Bob Example @bob
```

The `*` prefix marks the local device; `[admin]` marks conversation
administrators (Step 72a — the ADR-0018 case-fold gate keys off this
role).

### `room-devices`

List the devices of a room — one per leaf of the local MLS group
(Step 95d) — with the **MLS thumbprint** of each device's signature key,
printed in 2-character groups like the "MLS Thumbprint" an official Wire
app shows in the device details, so the two can be compared by eye. `*`
marks this device. An e2ei X.509 device is listed with `(identity not
parsed)`.

```bash
kupferdraht-cli room-devices --id <room>@<domain>
```

| Flag   | Required | Description                     |
|--------|----------|---------------------------------|
| `--id` | yes      | Qualified room id.              |

Output:

```
room-devices
  room_id: <room>@<domain>
  count:   N
  * <user>@<domain> / <device>  [basic]
      mls thumbprint: oK Iy wv GU pT VT …
    <user>@<domain> / <device>  [basic]
      mls thumbprint: …
```

### `room-invite`

Invite one or more Wire users into an existing room (Step 6b /
6c). Claims one KeyPackage per device for each invitee, builds a
single Add-commit, uploads it to `/v{N}/mls/commit-bundles`, and
re-syncs the room metadata.

```bash
kupferdraht-cli room-invite --id <room>@<domain> -u <user>@<domain>
kupferdraht-cli room-invite --id <room>@<domain> -u alice@... -u bob@...   # multi
```

| Flag            | Required | Description                                                                |
|-----------------|----------|----------------------------------------------------------------------------|
| `--id`          | yes      | Qualified room id.                                                         |
| `-u`, `--user`  | yes      | One or more qualified user ids. Repeat the flag for multiple invitees.    |

Self-bootstrap: the orchestrator also claims the caller's own
`KeyPackage`s (excluding the current device) and folds them into
the same commit. Wire enforces every device of every member-user to
have a leaf (observations.md §27); without the self-claim the upload
trips `409 mls-client-mismatch`.

Idempotent re-invites: a follower-pass duplicate filter drops any
KeyPackage whose signature key already corresponds to a current
group member.

The first `room-invite` against a freshly-`room-create`d room is
also what advances it from epoch 0 → 1 and unlocks `send`.

### `room-kick`

Remove one or more Wire users from an existing room (Step 6d).
Builds a single Remove-commit covering every leaf belonging to
the listed users (one user can have up to 7 device leaves),
uploads it to `/v{N}/mls/commit-bundles`, and re-syncs the room
metadata.

```bash
kupferdraht-cli room-kick --id <room>@<domain> -u <user>@<domain>
kupferdraht-cli room-kick --id <room>@<domain> -u alice@... -u bob@...   # multi
```

| Flag            | Required | Description                                                                |
|-----------------|----------|----------------------------------------------------------------------------|
| `--id`          | yes      | Qualified room id.                                                         |
| `-u`, `--user`  | yes      | One or more qualified user ids. Repeat the flag for multiple targets.     |

**Self-kick is rejected.** Removing your own last device from a
room collapses your own MLS state in a way that needs the proper
logout / device-revocation lifecycle. Trying surfaces
`KupferdrahtError::InvalidInput` with a clear message.

A user not currently in the room — or a malformed user_id — is
also surfaced as `InvalidInput`; the call is a no-op against
Wire when no leaf can be removed.

Output:

```
room-kick ok
  room_id: <room>@<domain>
  kicked:  N
  epoch:   <new>
  members (M):
    * <self> — <name> @<handle>
      <other> — <name> @<handle>
      ...
```

### `send` (with mentions, Step 6-II-g)

The `send` command auto-parses `@<handle>` tokens in the body
and resolves each one to a qualified user-id via Wire's
`POST /v{N}/list-users` (qualified-handles shape). Each
resolved handle becomes a `Mention { start, length,
qualified_user_id }` entry in the outbound `Text.mentions`,
with offsets in UTF-16 code units to match what
iOS / Android / Web emit. Handles Wire can't resolve are
left as plain text in the body.

```bash
kupferdraht-cli send --id <room>@<domain> -m "hi @alice, see this"
# Output adds a `mentions: N` line when at least one handle
# resolved.
```

The `timeline` printer suffixes mention-carrying lines with
`[mentions: @<uid>@<domain> @<uid>@<domain>]` so the qualified
ids land in the visible output.

### `send`

Encrypt a plain-text `GenericMessage` and POST it to
`/v{N}/mls/messages`. The room must have a local MLS group (run
`room-join` first if Kupferdraht did not create the group). Persists a
`timeline_events` row as `delivery_status='sending'` before the upload
and settles it to `'submitted'` (or `'failed'`) after (Step 97a).

```bash
kupferdraht-cli send --id <room>@<domain> -m "hello"
kupferdraht-cli send --id <room>@<domain> -m "Your one-time code: 123456" --ephemeral 300
```

| Flag              | Required | Description                                       |
|-------------------|----------|---------------------------------------------------|
| `--id`            | yes      | Qualified room id.                                |
| `-m`, `--message` | yes      | UTF-8 message body.                               |
| `--ephemeral <SECS>` | no    | **Step 89.** Send as a self-deleting message with this timer (1 … 31536000 s): the body rides Wire's `Ephemeral` wrapper, official clients show a countdown and delete their copy, Kupferdraht's own row self-redacts too (`Timeline::send_ephemeral_text`). `@handle` mentions are not parsed in this mode (V1 carries none inside the wrapper). |

Output:

```
send ok
  room_id:    <room>@<domain>
  message_id: <uuid>
  sent_at:    <rfc3339, local offset>
```

With `--ephemeral`, two more lines follow:

```
  ephemeral:  300s
  expires_at: <rfc3339, local offset>
```

### `failed-sends` (Step 79)

List a room's failed sends — `timeline_events` rows with
`delivery_status='failed'` (the upload never reached the backend,
Step 25a) or an abandoned `'sending'` row (the send was interrupted,
Step 97a), oldest first, in the `timeline` rendering (`✗`; a send
still in flight shows `…`). These are
the rows `resend` can re-drive; task / case / marker snapshots are
listed too (nothing hides) but are refused by `resend`. Library:
`Room::failed_sends`.

```bash
kupferdraht-cli failed-sends --id <room>@<domain>
```

Output:

```
failed-sends ok
  room_id: <room>@<domain>
  failed:  N
    [<sent_at rfc3339>] ✗ (<message_id>)  <body or <kind …> preview>[  (reply to <parent_id>)]
```

### `resend` (Step 79)

Re-drive a failed send from its persisted row under the **same
message_id** — text incl. reply / mentions, ephemeral text (re-wrapped
with its timer), knock, location, and files (the blob is already on
`/v14/assets`; only the MLS reference travels again). A task / case /
marker snapshot is refused with a pointer at the state-sending command
(`task edit` / `case-set` / `task mark`) — re-sending a stale snapshot
could regress the receivers' state. Library: `Timeline::resend` /
`Timeline::resend_failed`.

```bash
kupferdraht-cli resend --message <message_id>                 # one row; room auto-resolved
kupferdraht-cli resend --id <room>@<domain> --message <id>    # one row, explicit room
kupferdraht-cli resend --id <room>@<domain> --all             # every failed row, oldest first
```

| Flag        | Required | Description |
|-------------|----------|-------------|
| `--message` | with one row | `message_id` of the failed row (from `failed-sends` / `timeline`). |
| `--id`      | with `--all` | Qualified room id. Optional with `--message` (auto-resolved from the message id against the local timeline cache). |
| `--all`     | no       | Re-drive every failed send of the room, oldest first. The pass **stops at the first hard error** (a stuck message must not be overtaken) and leaves the rest `failed` for the next run; snapshot rows are skipped. |

Output (one row):

```
resend ok
  room_id:    <room>@<domain>
  message_id: <uuid>            # unchanged
  sent_at:    <rfc3339>         # re-stamped to the resend instant
```

Output (`--all`):

```
resend ok
  room_id:   <room>@<domain>
  resent:    N
    <message_id>  sent_at <rfc3339>
  skipped:   M
    <message_id>  (snapshot row — re-send the current state instead)
  stopped:   <message_id>: <error>      # only when the pass stopped early
  remaining: K                          # failed rows behind the stop, not attempted
```

A failed resend exits non-zero (the row stays `failed`); a `--all` pass
exits zero and reports its stop in the summary — re-run it once the
backend is back. This is the CLI form of the reconnect contract
(`docs/library.md` "Reconnect contract"): the core never retries on its
own.

### `devices`

List every device registered for the authenticated user
(`GET /v{N}/clients`). Wire enforces a 7-device cap; this command
is the CLI escape hatch when the cap is hit and `login` (which
calls `POST /v{N}/clients`) starts failing.

```bash
kupferdraht-cli devices
```

No flags — reads the current session.

Output (`*` marks the current device):

```
devices ok
  registered: M/7
    * <client_id> [desktop] — <label> (last_active: <UTC>)
      <client_id> [phone]   — <label> (last_active: <UTC>)
      ...
```

### `key-packages`

Show this device's server-side MLS key-package pool (Step 63,
`GET /v{N}/mls/key-packages/self/{client}/count`). Every peer that
adds this device to a conversation claims one package; an empty pool
means the device can no longer be added to new conversations. The
sync loop tops the pool back up automatically (below 50 → refill to
100, checked at most once per 24 h); `--replenish` forces the top-up
now.

```bash
kupferdraht-cli key-packages
kupferdraht-cli key-packages --replenish
```

| Flag          | Required | Description                                        |
|---------------|----------|----------------------------------------------------|
| `--replenish` | no       | Force a top-up now (no-op while the pool is healthy). |

Output:

```
key-packages
  server pool:  N (target 100, auto top-up below 50)
  status:       LOW — auto top-up on next sync, or run --replenish
```

### `device-delete`

Revoke a registered device by its `client_id`
(`DELETE /v{N}/clients/{id}`). Wire requires the user's password
in the body — pass it via stdin (`--password-stdin`) or be
prompted on the TTY.

The CLI **refuses** to delete the current device (Kupferdraht's own
session would be revoked mid-call). Use `logout` once Step 6e
ships for that flow.

```bash
kupferdraht-cli device-delete --id <client_id>
kupferdraht-cli device-delete --id <client_id> --password-stdin < pw.txt
```

| Flag                | Required | Description                                              |
|---------------------|----------|----------------------------------------------------------|
| `--id`              | yes      | Wire `client_id` (hexadecimal) of the device to delete.  |
| `--password-stdin`  | no       | Read password from stdin instead of prompting.           |

Output:

```
device-delete ok
  device_id: <client_id>
```

Common errors:
- `403 invalid-credentials` — password mismatch; surfaced as
  `BackendRejected`. Re-run with the correct password.
- `404 client-not-found` — the `client_id` doesn't belong to
  this user (or already deleted). Re-run `devices` to see what
  remains.

### `device-rename`

Update the user-visible label of a registered device
(`PUT /v{N}/clients/{id}` with `{"label": "..."}`). The label
shows up in Wire's settings UI on every Wire client.

```bash
kupferdraht-cli device-rename --id <client_id> -l "MacBook Pro M2"
```

| Flag           | Required | Description                                          |
|----------------|----------|------------------------------------------------------|
| `--id`         | yes      | Wire `client_id` of the device to rename.            |
| `-l`, `--label`| yes      | New label.                                           |

Output:

```
device-rename ok
  device_id: <client_id>
  label:     <new label>
```

### `profile-set`

Update the authenticated user's profile (Step 2g). At least
one of `--display-name` or `--handle` must be supplied; both can
be supplied in one call (each is sent as a separate Wire request).

```bash
kupferdraht-cli profile-set --display-name "Alice (Kupferdraht)"
kupferdraht-cli profile-set --handle "alice"
kupferdraht-cli profile-set --display-name "Alice" --handle "alice2"
```

| Flag             | Required                  | Description                                                                  |
|------------------|---------------------------|------------------------------------------------------------------------------|
| `--display-name` | one of these two required | Patches `PUT /v{N}/self` with `{"name": "..."}`.                            |
| `--handle`       | one of these two required | Patches `PUT /v{N}/self/handle` with `{"handle": "..."}`. Wire enforces global uniqueness — collisions surface as `BackendRejected`. |

Output:

```
profile-set ok
  display_name: <new display_name>     # only if --display-name was passed
  handle:       <new handle>           # only if --handle was passed
```

Note: the local `Account` snapshot (visible via `kupferdraht-cli status`)
reflects login-time values until the next `sync`.

### `email-update`

Request a change of the account email (Step 98a — the CLI consumer of
`Client::update_email`, which had none). Wire answers before the address
changes: it mails a verification link to the new address and applies the
change only once that link is followed, so a success here means "mail
sent", not "email changed".

```bash
kupferdraht-cli email-update --email "new@example.com"
```

| Flag      | Required | Description                                                                 |
|-----------|----------|-----------------------------------------------------------------------------|
| `--email` | yes      | Sends `PUT /v{N}/access/self/email` with `{"email": "..."}`. Must not be empty.     |

Output:

```
email-update ok
  verification sent to: <email>
  the address changes once the link in that mail is followed
```

### `change-password`

Change the account password (Step 27). Prompts on the TTY for the
current password, the new password, and a confirmation — no passwords
are passed as arguments (they would leak into `ps` / shell history).

```bash
kupferdraht-cli change-password
# Current account password: ****
# New password: ****
# New password (again): ****
```

Sends `PUT /v{N}/self/password` with `{"old_password","new_password"}`.
On success Wire **invalidates every device's session**, so all other
devices must re-`login`; registered devices and MLS group state survive.
This session keeps working until its access token expires (~15 min).
`403 invalid-credentials` (wrong current password) and `400` (password
too short / policy) surface as `BackendRejected`.

Output:

```
change-password ok
  every device's session is now invalidated — other devices must re-login.
  this session keeps working until its access token expires (~15 min).
```

### `room-rename`

Rename a conversation server-side (Step 2g)
(`PUT /v{N}/conversations/{domain}/{id}/name`). Wire emits a
`conversation.rename` event to all members; their clients show
the new name on their next sync.

```bash
kupferdraht-cli room-rename --id <room>@<domain> -n "New room name"
```

| Flag         | Required | Description                          |
|--------------|----------|--------------------------------------|
| `--id`       | yes      | Qualified room id.                   |
| `-n`, `--name` | yes    | New conversation name.               |

Output:

```
room-rename ok
  room_id: <room>@<domain>
  name:    <new name>
```

Common errors:
- `403 invalid-op` — the calling user lacks the role to rename
  the conversation (e.g. read-only member). Surfaced as
  `BackendRejected`.
- `404 no-conversation` — `room_id` doesn't exist or the user
  is not a member.

### `mark-read`

Mark a message as read (Step 6-II-f). Wire-side emits a
`Confirmation { type: READ, first_message_id }` carrying just
the supplied id. Recipients stamp `(message_id, reader_user_id)`
into `read_receipts`; Wire web shows a "read" indicator next to
the original message.

**Step 10f gate**: when the user has read receipts disabled
(`WIRE_RECEIPT_MODE = 0` — toggled via Wire-Web → Settings →
"Read receipts"), this command short-circuits without sending the
`Confirmation`. The output prints `mark-read skipped` and the
reason. Other Wire clients respect the same setting, so peers
won't expect a receipt.

```bash
kupferdraht-cli mark-read --id <room>@<domain> --message <message_id>
```

| Flag        | Required | Description                                                                       |
|-------------|----------|-----------------------------------------------------------------------------------|
| `--id`      | no¹      | Qualified room id. When omitted, auto-resolved from `--message` against local timeline. |
| `--message` | yes      | `message_id` of the message to confirm.                                           |

¹ The CLI looks the message up in `timeline_events`. If the
message hasn't been synced locally (run `sync` /
`sync-stream` / `timeline --id <room>` first) or matches multiple
rooms (UUIDv4 collisions don't happen in practice), the CLI
errors out with a hint to pass `--id` explicitly. Same rule
applies to `react`, `delete-message`, `edit`, `reply`.

Output (receipts on):

```
mark-read ok
  room_id:           <room>@<domain>
  target:            <message_id>
  confirmation_event: <fresh uuid>
  confirmed_at:      <rfc3339, local offset>
```

Output (receipts off — gate fired):

```
mark-read skipped
  room_id:           <room>@<domain>
  target:            <message_id>
  reason:            WIRE_RECEIPT_MODE = Off
```

The local `read_receipts` table also records the caller's own
receipt so `timeline` shows `✓×N` (where N is the count of
distinct reader user-ids that have confirmed).

Idempotence: receiving the same `Confirmation` for the same
`(message, reader)` pair is a `INSERT OR IGNORE` no-op.

### `react`

Toggle a single emoji reaction on a message (Step 6-II-e).
Wire's `Reaction` payload is the caller's COMPLETE current
set of emojis on the parent (comma-separated CSV); the CLI
loads the existing set, toggles `--emoji` in or out, and
sends the new set.

```bash
kupferdraht-cli react --id <room>@<domain> --message <message_id> --emoji "👍"
# Re-run with the same emoji to toggle off:
kupferdraht-cli react --id <room>@<domain> --message <message_id> --emoji "👍"
```

| Flag        | Required | Description                                                                       |
|-------------|----------|-----------------------------------------------------------------------------------|
| `--id`      | no       | Qualified room id. Auto-resolved from `--message` when omitted (see `mark-read`). |
| `--message` | yes      | `message_id` of the message to react to.                                          |
| `--emoji`   | yes      | Emoji to toggle (single, e.g. `👍`).                                              |

Output:

```
react ok
  room_id:        <room>@<domain>
  target:         <message_id>
  emoji_toggled:  👍
  reaction_event: <fresh uuid>
  applied_at:     <rfc3339, local offset>
```

The `timeline` printer suffixes message lines with their
reaction summary `[👍×2 ❤️×1]` when reactions are present.

Idempotence: the receive path uses Wire's "replace the
sender's full set" semantic, so redelivery of the same
`Reaction` event is a no-op (the resulting set is identical).

### `delete-message`

Delete-for-everyone a previously-sent text message (Step
6-II-d). Wire-side: `GenericMessage { content: MessageDelete
{ message_id } }`. Receivers tombstone the row — body
hidden, but the row stays so reply chains stay renderable. Only your
own messages (from any of your devices) can be deleted for everyone —
another user's message is refused before anything is sent (Step 95b,
ADR-0028).

```bash
kupferdraht-cli delete-message --id <room>@<domain> --message <message_id>
```

| Flag           | Required | Description                                                                       |
|----------------|----------|-----------------------------------------------------------------------------------|
| `--id`         | no       | Qualified room id. Auto-resolved from `--message` when omitted (see `mark-read`). |
| `--message`    | yes      | `message_id` of the message to delete.                                            |

Output:

```
delete-message ok
  room_id:      <room>@<domain>
  target:       <message_id>
  delete_event: <fresh uuid for the delete event itself>
  redacted_at:  <rfc3339, local offset>
```

The local `timeline_events` row gets `body_ct` cleared and
`redacted_at_ms` stamped. The `timeline` printer renders
redacted rows as `<redacted>` with `(deleted)` suffix.

Idempotence: receiving the same `MessageDelete` twice is a
no-op — the SQL guard `redacted_at_ms IS NULL OR
redacted_at_ms < ?` blocks redelivery from re-firing.

### `edit`

Replace the body of a previously-sent text message (Step
6-II-c). Wire-side: a `GenericMessage { content: MessageEdit
{ replacing_message_id, text { new_body } } }`. The recipient's
client mutates the existing row in place — the original
`message_id` stays valid. Only your own messages (from any of your
devices) can be edited — another user's message is refused before
anything is sent (Step 95b, ADR-0028: every receiver would reject it).

```bash
kupferdraht-cli edit --id <room>@<domain> --message <message_id> -m "<new body>"
```

| Flag             | Required | Description                                                                       |
|------------------|----------|-----------------------------------------------------------------------------------|
| `--id`           | no       | Qualified room id. Auto-resolved from `--message` when omitted (see `mark-read`). |
| `--message`      | yes      | `message_id` of the original to replace.                                          |
| `-m`, `--new-body` | yes    | Updated body.                                                                     |

Output:

```
edit ok
  room_id:    <room>@<domain>
  replacing:  <original message_id>
  new_body:   <new body>
  edit_event: <fresh uuid for the edit event itself>
  edited_at:  <rfc3339 when the edit was applied locally>
```

The local `timeline_events` row for the original message_id has
its body replaced and `edited_at_ms` stamped. The `timeline`
printer suffixes edited lines with `(edited at <ts>)`.

Idempotence: receiving the same `MessageEdit` twice (Wire
redelivery) is a no-op — the SQL `UPDATE` only fires when the
incoming `edited_at_ms` is strictly newer than the persisted
one, so a redelivered older edit cannot roll back a newer one.

### `reply`

Send a text message that quotes a previous one in the same room
(Step 6-II-b). Wire-side: a regular `GenericMessage` whose
`Text.quote.quoted_message_id` points at the parent. Receivers
thread it as a quote / reply.

```bash
kupferdraht-cli reply --id <room>@<domain> --to <message_id> -m "..."
```

| Flag           | Required | Description                                                                                 |
|----------------|----------|---------------------------------------------------------------------------------------------|
| `--id`         | no       | Qualified room id. Auto-resolved from `--to` (the parent's id) when omitted (see `mark-read`). |
| `--to`         | yes      | `message_id` of the parent message (UUID from `send` / `timeline`).                         |
| `-m`, `--message` | yes   | Reply body.                                                                                 |

Output:

```
reply ok
  room_id:    <room>@<domain>
  reply_to:   <parent message_id>
  message_id: <new uuid>
  sent_at:    <rfc3339, local offset>
```

The `timeline` printer renders replies with a `↳ in reply to
<parent>` decoration above the body.

### `knock`

Send a `Knock` (nudge / "ping") to a room (Step 26). Wire-side this is a
`GenericMessage { knock { hot_knock } }`; receivers surface it as a poke
with no body.

```bash
kupferdraht-cli knock --id <room>@<domain>
# A "hot" (double-tap) knock:
kupferdraht-cli knock --id <room>@<domain> --hot
```

| Flag    | Required | Description                                       |
|---------|----------|---------------------------------------------------|
| `--id`  | yes      | Qualified room id.                                |
| `--hot` | no       | Send a hot (repeat) knock instead of a plain one. |

Output:

```
knock ok
  room_id:    <room>@<domain>
  hot_knock:  false
  message_id: <fresh uuid>
  sent_at:    <rfc3339, local offset>
```

The `timeline` printer renders an inbound knock as `<knock>` or
`<knock (hot)>`.

### `location`

Share a `Location` (map point) with a room (Step 26). Wire-side this is a
`GenericMessage { location { longitude, latitude, name?, zoom? } }`.

```bash
kupferdraht-cli location --id <room>@<domain> --lat 48.1372 --lon 11.5756
# With a label and zoom level:
kupferdraht-cli location --id <room>@<domain> --lat 48.1372 --lon 11.5756 --name "Marienplatz" --zoom 16
```

| Flag     | Required | Description                          |
|----------|----------|--------------------------------------|
| `--id`   | yes      | Qualified room id.                   |
| `--lat`  | yes      | Latitude in decimal degrees.         |
| `--lon`  | yes      | Longitude in decimal degrees.        |
| `--name` | no       | Optional human-readable place label. |
| `--zoom` | no       | Optional map zoom level.             |

Output:

```
location ok
  room_id:    <room>@<domain>
  lat/lon:    48.1372, 11.5756
  name:       Marienplatz        (only when given)
  zoom:       16                 (only when given)
  message_id: <fresh uuid>
  sent_at:    <rfc3339, local offset>
```

The `timeline` printer renders an inbound location as
`<location 48.1372, 11.5756 "Marienplatz", zoom=16>` (label and zoom
only when present).

### `case-set` / `case-show`

Per-room case state (Step 72c, ADR-0018). `case-set`
patches the folded state — unset flags keep their values — bumps
`revision`, and sends the resulting **full** `CasePayload` snapshot
(no text fallback; plain Wire clients see nothing). Requires the
caller to be a conversation administrator; the receive-side fold
ignores non-admin snapshots either way. `case-show` prints the
folded state (latest admin-authorized core + display-only
extensions).

```bash
kupferdraht-cli case-set  --id <uuid>@<domain> --title "Building permit, 1 Sample Street"     --state active --external-ref "REF 12/2026-K" --authority BA-Central     --ext source=portal --ext cost_center=4711
kupferdraht-cli case-show --id <uuid>@<domain>
```

| Flag | Description |
|------|-------------|
| `--id` | Qualified room id (required). |
| `--case-ref` | Case identity — honored only on create (default: generated UUID). |
| `--title` / `--case-type` / `--detail` / `--external-ref` / `--authority` | Core text fields. |
| `--state` | `open`\|`active`\|`waiting`\|`resolved`\|`closed`\|`cancelled`. |
| `--assignee` | Qualified user id. |
| `--due-ms` | Deadline (Unix ms). |
| `--priority` | `low`\|`normal`\|`high`\|`urgent`. |
| `--ext KEY=VALUE` | Display-only extras, repeatable; replaces the whole map when given. |

Inviting a member re-broadcasts the current snapshot automatically
(ADR-0018) — a fresh member sees the case on their first sync.

### `timeline`

Print persisted timeline rows for a room (Step 6-II-a). Outbound
rows (`→`) come from prior `send` calls; inbound rows (`←`) come
from notifications drained on the receive path; a `✗` row is an
outbound send whose upload failed (Step 25a) — `resend` re-drives it
(Step 79).

The CLI implementation drains pending notifications **before**
reading so users get the expected "show me what's new" behaviour
in one shot. Non-text rows render as previews: `<asset …>`,
`<knock>`, `<location …>`, `<task …>` and — since Step 72b —
`<case <case_ref> rev <n> <title>>` (the folded room state comes
from `case-show`, landing in 72c). Since Step 89 a live ephemeral
row carries a `⏱ until <ts>` suffix and an elapsed one prints
`(expired)` in place of `(deleted)` — the pre-read drain also runs
the expiry sweep, so a row whose timer passed while the CLI was not
running is already tombstoned when it prints. The underlying library call (`Client::messages` /
`Room::messages` / `Timeline::items`) is still local-cache-only
— long-running consumers should keep `sync-stream` running
separately and treat `timeline` as a pure cache read.

```bash
kupferdraht-cli timeline --id <room>@<domain>
kupferdraht-cli timeline --id <room>@<domain> --limit 10
kupferdraht-cli timeline --id <room>@<domain> --before <message_id>
```

| Flag        | Required | Description                                                            |
|-------------|----------|------------------------------------------------------------------------|
| `--id`      | yes      | Qualified room id.                                                     |
| `--limit`   | no       | Max items to return (omitted = no limit). Returns the newest window of size `limit`, then sorted oldest-first. **Semantic change:** `--limit 0` now prints zero items — the 0-as-no-limit sentinel is gone. |
| `--before`  | no       | Cursor: only items strictly older than this `message_id` are returned. |

Output (oldest-first):

```
timeline ok
  room_id:  <room>@<domain>
  drained:  K       # notifications drained from /v{N}/notifications this call
  items:    N
    [<sent_at rfc3339>] → <self_user_id> (<message_id>)  <body>
    [<sent_at rfc3339>] ← <peer_user_id> (<message_id>)  <body>
    ...
```

### `task add` (Step 60a)

Create and send a task into a room (ADR-0014). The library generates
the stable `task_id` (UUID) and stamps `status: open`; state changes
travel as later full-state updates (Step 60b). Without a fallback
flag the encoding mode derives from the room's task-list marker
(Step 75, ADR-0020): an unmarked room also carries a human-readable
`Text` fallback ("☐ <title> — @<handle>") that official Wire clients
render, a marked room sends the bare payload. `--no-fallback` forces
mode a (bare payload; official clients ignore the message entirely),
`--fallback` forces mode c — an explicit flag always wins.

`--due` takes ISO 8601: `yyyy-mm-dd` (due by **end** of that local
day, 23:59:59), `yyyy-mm-ddThh:mm[:ss]` or `yyyy-mm-dd hh:mm[:ss]`
(local time), or RFC 3339 with explicit offset/`Z`. `--due-ms` remains
as a raw Unix-milliseconds escape hatch (mutually exclusive). The wire
format and library API stay epoch-ms (`due_at_ms`) — an unambiguous,
integer-sortable instant; ISO is the CLI's human boundary only. All
CLI timestamp *output* is RFC 3339 in the local offset, truncated to
seconds (`task list`, `timeline`, send echoes, `config`).

```sh
kupferdraht-cli task add --id <room>@<domain> --title "Prepare deploy" \
  [--description "Runbook: https://…"] \
  [--assignee <uuid>@<domain>] \
  [--due 2026-07-20T15:00] \
  [--priority low|medium|high|urgent] \
  [--no-fallback | --fallback]
```

```
task add ok
  room_id:    <room>@<domain>
  title:      Prepare deploy
  assignee:   <uuid>@<domain>
  due:        2026-07-20T15:00:00+02:00
  priority:   high
  fallback:   auto (from the room's task-list marker)
  message_id: <uuid>
  sent_at:    <rfc3339, local offset>
```

### `task list` (Steps 60b/60c)

The **folded** task list of a room: one entry per `task_id`, latest
full state wins (LWW in arrival order); manually moved tasks come
first in `sort_key` order, the rest by due date (deadlines before
none), then priority (urgent first), then first-seen. Drains pending
notifications first so fresh updates are folded in. A deleted task
message drops out of the fold (its body is wiped); a task whose only
message was deleted disappears. Reads the materialized `tasks` table
(fold-on-write) — no timeline re-fold per invocation.

Cross-room views (Step 60c) replace `--id`: `--mine` lists tasks
assigned to this account that are still open/in-progress;
`--delegated` lists tasks this account created and assigned to
someone else (any status, so progress stays visible).

```sh
kupferdraht-cli task list --id <room>@<domain>
kupferdraht-cli task list --mine
kupferdraht-cli task list --delegated
```

```
task list ok
  room_id: <room>@<domain>
  tasks:   N
    ☐ "<title>" [open] @<assignee> due=<rfc3339> prio=<p>  (task_id=…, updated=<rfc3339>, by=<creator>)
    ☑ "<done title>" [done]  (task_id=…, updated=<rfc3339>, by=<creator>)

task list --mine ok
  tasks: N
    ☐ "<title>" [open] @<me>  (room=<room>@<domain>, task_id=…, updated=<rfc3339>)
```

### `task done` / `task assign` / `task edit` (Step 60b)

Change a task by sending a new **full-state** message (create ==
update, ADR-0014): the CLI reads the current folded state, applies the
change, and sends the result — every receiver folds it in (LWW).
`--task` takes the stable `task_id` printed by `task list`. All three
accept `--no-fallback` / `--fallback`; without either flag the mode
derives from the room's task-list marker (ADR-0020). `edit` is
set-only: fields can be set, not cleared; `--status open` on a done
task reopens it.

```sh
kupferdraht-cli task done   --id <room>@<domain> --task <task_id>
kupferdraht-cli task assign --id <room>@<domain> --task <task_id> -u <uuid>@<domain>
kupferdraht-cli task edit   --id <room>@<domain> --task <task_id> \
  [--title "…"] [--description "…"] [--assignee <uuid>@<domain>] \
  [--due <iso-8601>] [--priority low|medium|high|urgent] \
  [--status open|in-progress|done|cancelled]
```

```
task done ok
  room_id:    <room>@<domain>
  task_id:    <task_id>
  message_id: <uuid>
  sent_at:    <rfc3339, local offset>
```

### `task move` (Step 60c)

Move a task in the room's manual order. The CLI computes a fractional
`sort_key` strictly between the `--after`/`--before` neighbors
(omitting one means the corresponding end of the list; omitting
**both** moves the task to the front of the manual order — the way to
key a room's first task) and sends it as an ordinary full-state
update — a move touches exactly one task. Manual ordering opts in per
task: a referenced neighbor must already carry a `sort_key` (i.e.
must have been moved itself); keyless tasks sort after all keyed
ones.

```sh
kupferdraht-cli task move --id <room>@<domain> --task <task_id> \
  [--after <task_id>] [--before <task_id>] [--no-fallback | --fallback]
```

```
task move ok
  room_id:    <room>@<domain>
  task_id:    <task_id>
  message_id: <uuid>
  sent_at:    <rfc3339, local offset>
```

### `task mark` (Step 75)

Set or clear the room's task-list marker (ADR-0020): a
`TaskListMarker` full-state message (`GenericMessage` field 262002,
bare payload — official Wire clients see nothing). Any member may
flip it; there is no admin gate. A marked room groups under
task-list navigation in consumers (`Room::is_task_list()`), task
sends default to the bare payload, and `invite_members`
re-broadcasts the marker plus every task's folded state to late
joiners. `task list` shows the marker state.

```sh
kupferdraht-cli task mark --id <room>@<domain> [--clear]
```

```
task mark ok
  room_id:   <room>@<domain>
  task_list: marked
```

### `logout`

Sign out of the backend (Step 6e-2). Issues
`POST /v{N}/access/logout` to revoke the `zuid` refresh cookie
server-side. With `--purge-device`, **also** issues
`DELETE /v{N}/clients/{device_id}` first to remove the registered
device entirely (frees a 7-cap slot) — Wire requires the user's
password for that. With `--purge-device`, the CLI then wipes the
home it runs on — `$KUPFERDRAHT_HOME`, else the platform default directory
(see [Storage layout](#storage-layout)) — so the next `login` starts
from a clean slate. That wipe is the whole home: every account's
databases and `home.id`; the home's keystore entries stay behind
until Step 78 (the next `login` creates a new home id and fresh keys).

```bash
# Soft logout — kills the refresh cookie, keeps the device:
kupferdraht-cli logout

# Full logout — also deletes the device + wipes local DB:
pbpaste | kupferdraht-cli logout --purge-device --password-stdin
```

| Flag                 | Required | Description                                                                              |
|----------------------|----------|------------------------------------------------------------------------------------------|
| `--purge-device`     | no       | Also delete the device + wipe the home (`$KUPFERDRAHT_HOME` or the platform default).       |
| `--password-stdin`   | no       | Read password from stdin. Only relevant with `--purge-device`. Without it, the CLI prompts. |

Output:

```
logout ok
  device_id:    <client_id>
  purge_device: true | false
  local state:  wiped (<the home's path>) | preserved (next login reuses this device)
  next login:   will register a fresh device + MLS identity        # purge only
```

Common errors:
- `403 invalid-credentials` (logout) — refresh cookie already
  invalid (e.g. previously logged out). Surfaced as
  `BackendRejected`.
- `403 invalid-credentials` (delete-device) — wrong password.
  Surfaced as `BackendRejected`. The logout step is not attempted
  in this case (delete-device runs first).

Notes:
- Without `--purge-device`, local state is preserved and the
  next `login` reuses this device via the Step 7e reuse path
  (no extra 7-cap slot consumed).
- The access token expires naturally within its remaining ~15 min
  TTL; logout only kills the refresh cookie that would otherwise
  rebind it.

### `room-leave`

Leave a conversation (Step 6e-1). Issues
`DELETE /v{N}/conversations/{domain}/{id}/members/{self_d}/{self_uuid}`
to Wire — the **server** then produces the MLS Remove commit on the
leaver's behalf via its external-sender authority (observations.md §31,
the flow every Wire client uses). Locally, Kupferdraht discards the persisted MLS group
state and re-syncs metadata so the room drops out of `rooms`.

After a successful leave:
- `rooms` no longer lists the room.
- `send` against the room id fails (no local group).
- The remaining members receive the server-built Remove commit
  via their `/v{N}/notifications` stream.

```bash
kupferdraht-cli room-leave --id <room>@<domain>
```

| Flag    | Required | Description           |
|---------|----------|-----------------------|
| `--id`  | yes      | Qualified room id.    |

Output:

```
room-leave ok
  room_id: <room>@<domain>
```

Common errors:
- `403 invalid-op` — the calling user lacks the role to leave
  this conversation.
- `404 no-conversation` — `room_id` is unknown or the user is
  not a member.

---

### `room-delete`

Delete a team conversation for **everyone** (Step 23) — a team-admin
action, distinct from `room-leave` (which only removes you). Issues
`DELETE /v{N}/teams/{tid}/conversations/{cid}`; the owning team id is
read from the conversation's `team` field, falling back to your own team
(`GET /self`) since Wire's conversation-list response often omits it. An
account in no team is rejected. A `404` (already gone) is treated as
idempotent success. On success the cache is re-synced so the room drops
out of `rooms`.

```bash
kupferdraht-cli room-delete --id <room>@<domain>
```

| Flag    | Required | Description           |
|---------|----------|-----------------------|
| `--id`  | yes      | Qualified room id.    |

Output:

```
room-delete ok
  room_id: <room>@<domain>
```

Common errors:
- `InvalidInput` — `room_id` is not a team conversation (no owning
  team); use `room-leave` instead.
- `403` — the calling user lacks the team-admin role.

---

### room-access show / set / allow-guests (Step 8b)

Display and replace a conversation's `access` + `access_role`
arrays. Required prerequisite for `join-link-generate`: Wire's
`POST /code` returns `403 access-denied` on conversations whose
`access` array doesn't include `"code"`. Kupferdraht-created
conversations default to `access=["invite"]` (members only).

```bash
kupferdraht-cli room-access show --id <room-id>
kupferdraht-cli room-access set --id <room-id> \
    --modes "invite,code" \
    --roles "team_member,non_team_member,guest"
kupferdraht-cli room-access allow-guests --id <room-id>   # canonical guest-link config
```

`room-access show` re-fetches via
`POST /v{N}/conversations/list` so the output reflects Wire's
live state, not the local sync snapshot.

`--modes` is comma-separated; valid values per Wire's schema:
`invite`, `code`, `link`, `private`. `--roles` is comma-separated;
valid values: `team_member`, `non_team_member`, `guest`,
`service`. Both lists must be non-empty.

`allow-guests` is the convenience wrapper around the canonical
`access=["invite","code"]`,
`access_role=["team_member","non_team_member","guest"]`
configuration the Wire web client applies when you turn on "guest
links" in conversation settings.

**Errors:**
- `403 access-denied` / `403 action-denied` — caller's role lacks
  the `modify_conversation_access` permission.
- `403 invalid-op` — Wire rejected the requested combination as
  invalid (e.g. `access=["link"]` without `"code"` in some team
  configurations).
- `404 no-conversation` — unknown room.

### join-link-generate (Step 8)

Issue (or fetch the existing) reusable guest-link for a
conversation. Sends
`POST /v{N}/conversations/{cnv}/code`. Prints the link the backend
issues (`https://account.<operator>/conversation-join/?key=…&code=…`)
and whether the link is password-protected.

```bash
kupferdraht-cli join-link-generate --id <room-id>      # open link
kupferdraht-cli join-link-generate --id <room-id> -p   # password-protected (TTY prompt)
```

`-p` reads the password from the TTY (no echo). Wire requires
≥ 8 characters when present.

**Errors:**
- `403 access-denied` — caller's role can't issue codes.
- `404 no-conversation` — unknown room.

### join-link-current (Step 8)

Fetch the conversation's current guest link via
`GET /v{N}/conversations/{cnv}/code`. Prints `(no link issued)`
when none is on file.

```bash
kupferdraht-cli join-link-current --id <room-id>
```

### join-link-revoke (Step 8)

Revoke the conversation's current guest link via
`DELETE /v{N}/conversations/{cnv}/code`. Idempotent: revoking a
non-existent code prints success.

```bash
kupferdraht-cli join-link-revoke --id <room-id>
```

### join-via-link (Steps 8, 122)

Redeem a guest link and MLS-join the conversation it names. `--code`
and `--key` are the link's query parameters. Four phases:
`GET /v{N}/conversations/join` (the conversation the link names),
`POST /v{N}/conversations/join` (Wire-side membership), `sync_metadata`,
then the Step-3 external-commit `join_room` — skipped when the room's
group is here already. Prints the `room_id` of the joined conversation.

```bash
kupferdraht-cli join-via-link --code <code> --key <key>
kupferdraht-cli join-via-link --code <code> --key <key> -p
```

`-p` reads the link's password from the TTY (no echo); supply it
when the link was issued password-protected. Step 122 dropped
`--domain`: the link does not carry it, and the conversation id finds
the room.

**Errors:**
- `403 invalid-conversation-password` — wrong password (or none
  supplied for a protected link).
- `404 no-conversation-code` — link revoked, never issued, or of
  another operator (a redeem reaches only the own backend's
  conversations); nothing was redeemed.
- `409 guest-links-disabled` — Wire team has the feature off.

### features (Step 10c)

List Wire's team-feature configs as persisted at the last successful
login or `feature-config.update` event. Reads from the local store
only — no network call. Empty when login has not yet populated the
V18 `feature_configs` table.

```bash
kupferdraht-cli features                   # list every persisted entry
kupferdraht-cli features --name fileSharing  # detail for one entry
```

**Brief listing format:** `<name>  <status>  <config_json>` per row,
sorted alphabetically by name.

**Detail format:** four lines — `name`, `status` (`—` when null),
`last_synced_at` (RFC 3339), raw `config_json`. Use this for configs whose
brief `config_json` is too long to read inline.

**Typical output for a full-plan team:** 30+ entries on a full team
plan — `fileSharing`, `mls`, `mlsE2EId`, `selfDeletingMessages`,
`appLock`, `legalhold`, `classifiedDomains`, `conferenceCalling`,
`digitalSignatures`, plus further team features (`cellsInternal`,
`apps`, `meetings`, `validateSAMLemails`, …). The exact set depends
on the team's plan and Wire-side feature flags. Trimmed-down team
plans may show fewer.

**Note:** Kupferdraht does not yet *react* to most of these — typed getters
(`Client::file_sharing` for Step 11, `Client::receipt_mode` for
Step 6-II-f gating) land in 10e. The brief listing is the diagnostic
surface for "what does the server say about this team?".

### properties (Step 10c)

List the authenticated user's properties as persisted at the last
login or `user.properties-set` event. Reads from the local store
only — no network call.

```bash
kupferdraht-cli properties                       # list every property
kupferdraht-cli properties --key WIRE_RECEIPT_MODE  # detail for one
```

**Brief listing format:** `<key>  <value-or-truncated>` per row,
sorted alphabetically. Values longer than 80 bytes are replaced by
`<N bytes — run with --key for full output>`; Wire-Web's `webapp`
blob can be multiple KB and is the most common offender.

**Detail format:** three lines — `key`, `last_synced_at`, full
`value`.

**Out of scope (V1):** `PUT /v{N}/properties/{key}` and
`DELETE /v{N}/properties/{key}` are intentionally not implemented per
ADR-0012 §Out of scope. Kupferdraht reads only; the user sets and clears
properties via Wire's other clients. The next sync cycle picks the
new value up via the `user.properties-set` event (Step 10d).

### send-file (Step 11h)

Encrypt + upload + MLS-send a file as an `Asset` content variant
(Step 11, ADR-0011). The team's `fileSharing` policy (Step 10e)
gates the call before any Wire network traffic — `Disabled`
rejects with `InvalidInput`, `Enabled` proceeds (the common case per
Step-10 carryover §C).

```bash
kupferdraht-cli send-file --id <room>@<domain> --path <path> --mime <mime>
```

| Flag       | Required | Description                                                         |
|------------|----------|---------------------------------------------------------------------|
| `--id`     | yes      | Qualified room id.                                                  |
| `--path`   | yes      | Plaintext file path. Streamed through AES-256-CBC + SHA-256.        |
| `--mime`   | yes      | Mime type (e.g. `image/png`). Embedded into `Asset.Original.mime_type`. |

Output:

```
send-file ok
  room_id:    <room>@<domain>
  path:       <local-path>
  mime:       <mime>
  message_id: <fresh uuid>
  sent_at:    <rfc3339, local offset>
```

**Cache:** the encrypted ciphertext tempfile is moved into
`~/.kupferdraht/<account_id>/assets/<message_id>.enc` after a
successful send so a future `download-asset` on the same device
hits the local file. Plaintext is never persisted by Kupferdraht
(ADR-0011 §Plaintext persistence).

**Errors:**
- `InvalidInput` — `fileSharing` policy rejected (disabled or
  mime not in allow-list), or `--path` unreadable.
- `BackendRejected` — `/v14/assets` returned 4xx/5xx (413 =
  asset exceeds Wire's per-message cap; 403 =
  `not-allowed-by-team-feature-config` if Wire-server enforces
  the policy server-side too).

### download-asset (Step 11h)

Download + decrypt an inbound asset that has been dispatched
into the local store. Verifies SHA-256 against the value carried
inside the MLS-protected `Asset.RemoteData.sha256`; rejects
tampered ciphertext with `IntegrityCheck`.

```bash
kupferdraht-cli download-asset --message <id> [--id <room>] [--save-to <path>]
```

| Flag         | Required | Description                                                                       |
|--------------|----------|-----------------------------------------------------------------------------------|
| `--message`  | yes      | `message_id` of the asset message.                                                |
| `--id`       | no       | Qualified room id. Auto-resolved from `--message` against the local timeline cache when omitted (same resolver as `mark-read`). |
| `--save-to`  | no       | Filesystem path to write the decrypted plaintext to. Without this flag, prints metadata only — useful for inspecting multi-megabyte assets before deciding to decrypt. |

Output (metadata only — no `--save-to`):

```
download-asset (metadata only — pass --save-to <path> to decrypt)
  message_id:  <id>
  room_id:     <room>@<domain>
  filename:    <Asset.Original.name or —>
  mime:        <Asset.Original.mime_type>
  size_bytes:  <Asset.Original.size>
  sender:      <qualified-user-id>
  asset_id:    <Wire asset key>
  asset_dom:   <Wire asset domain>
  encryption:  AES_CBC | AES_GCM
```

Output (`--save-to <path>` supplied):

```
download-asset ok
  message_id: <id>
  room_id:    <room>@<domain>
  filename:   <Asset.Original.name>
  mime:       <Asset.Original.mime_type>
  size_bytes: <Asset.Original.size>
  sender:     <qualified-user-id>
  saved_to:   <path>
```

**Cache:** the encrypted ciphertext is downloaded once and cached
at `~/.kupferdraht/<account_id>/assets/<message_id>.enc`; subsequent
calls re-decrypt from the cache without re-hitting Wire.

**Errors:**
- `InvalidInput` — `encryption_algo = "AES_GCM"` (unsupported in
  V1 per ADR-0011 §AES-GCM toleration); asset not in local
  store; no store available (client built without `base_dir`).
- `IntegrityCheck` — SHA-256 mismatch over the downloaded
  ciphertext. Suggests a tampered Wire `/v14/assets` response.
- `BackendRejected` — `/v14/assets/...` returned 4xx/5xx.

### Pre-smoke checklist for Step 11h live tests

Step-10 learnings carryover §D + §E (see
`docs/plan/archive/11-asset-content.md` §11e) flag two pre-flight tasks
before exercising the asset pipeline against a live backend:

1. **Counterpart sender available** — for the inbound round-trip
   you need a second account that can emit an asset from an official Wire client
   into a shared room. Without this you cannot validate the
   decrypt path end-to-end; outbound-only smoke is fine but
   doesn't pin the receive side.
2. **Wire-Web still emits `AES_CBC`** — open Wire-Web DevTools →
   Network → inspect a Wire-Web asset upload's MLS payload →
   confirm `Asset.RemoteData.encryption = 0` (AES_CBC). If it
   shows `1` (AES_GCM), Wire's deployed format has changed
   during Step 11's implementation window — stop and open a
   follow-up ADR before running the inbound smoke.

---

## Workflows

### First-time setup (fresh device)

```bash
kupferdraht-cli login --api-url https://nginz-https.example.com -e you@example.com
kupferdraht-cli status                              # confirm session persisted
kupferdraht-cli sync                                # populate rooms cache
kupferdraht-cli rooms                               # see what's there
kupferdraht-cli rooms-join-all                      # bootstrap into every room (establishes the self room, Step 86)
kupferdraht-cli self-room                           # your notes-to-self room: room_id + epoch
kupferdraht-cli room-members --id <some-room>       # verify membership
kupferdraht-cli send --id <some-room> -m "hello"    # send your first message
```

### Daily use after a previous login

```bash
# resume auto-refreshes a stale token; re-login only if the zuid expired:
kupferdraht-cli status                              # optional: inspect token age
# Then:
kupferdraht-cli sync                                # refresh metadata + drain events (Step 14b)
kupferdraht-cli rooms                               # see updates
```

### Live regression baseline

Before testing a new Step's CLI subcommand against Wire, prove
Steps 1–6b still work end-to-end:

```bash
./scripts/smoke-baseline.sh
```

The script walks `status` → `sync` → `rooms` →
`send` (into a known-good 2-member room) and exits non-zero on the
first failure. The send target is
`KUPFERDRAHT_BASELINE_ROOM=<room-id>` (required).

---

## See also

- `README.md` — high-level Kupferdraht overview and quickstart.
- `docs/library.md` — the Rust `kupferdraht-core` library API
  (`ClientBuilder`, `Client`, `Room`, `Timeline`, …).
- `docs/bindings.md` — Kotlin / Swift / WASM consumption of the
  generated UniFFI bindings.
- `docs/wire/api-notes.md` — Wire HTTP endpoint observations.
- `docs/wire/mls-deviations.md` — places Wire deviates from
  RFC 9420.
- `docs/observations.md` — Wire/MLS observation log, indexed §1…§N (resolved changes: `CHANGELOG.md`; open items: `open.md`).
- `PLAN.md` — the ten-step roadmap that drives the CLI's growth.


---

### Example Workflow:

```bash
  ./target/release/kupferdraht-cli login --api-url https://nginz-https.example.com -e you@example.com
  ./target/release/kupferdraht-cli status
  ./target/release/kupferdraht-cli sync
  ./target/release/kupferdraht-cli rooms | grep -B1 -A4 "<room name>"
```

Note the <uuid>@<domain> of the new group from the output; below, replace <ROOM> with that id.

```bash
  ./target/release/kupferdraht-cli rooms-join-all
  ./target/release/kupferdraht-cli room-members --id <ROOM>
  ./target/release/kupferdraht-cli room-invite --id <ROOM> -u 8d1f2c34-aaaa-bbbb-cccc-dddddddddddd@example.com
  ./target/release/kupferdraht-cli room-members --id <ROOM>
  ./target/release/kupferdraht-cli send --id <ROOM> -m "6c smoke ok"
```