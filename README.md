# Kupferdraht

A Wire-compatible MLS end-to-end-encrypted messaging client in Rust,
exposed to Kotlin and Swift via UniFFI and to the browser via a
wasm-bindgen layer.

Motivation and architecture are in [`CLAUDE.md`](CLAUDE.md) (the
contributor charter) and [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md);
the "why not Kalium" rationale is [ADR-0009](docs/adrs/0009-matrix-rust-sdk-as-api-model.md)
and [`reference/README.md`](reference/README.md); [`PLAN.md`](PLAN.md) is the open backlog.

See also:
- [`docs/library.md`](docs/library.md) — public Rust API of
  `kupferdraht-core`.
- [`docs/api-mapping-matrix-rust-sdk.md`](docs/api-mapping-matrix-rust-sdk.md)
  — matrix-rust-sdk mapping and intentional drift.
- [`docs/cli.md`](docs/cli.md) — `kupferdraht-cli` command reference
  (populated as the CLI grows).
- [`docs/bindings.md`](docs/bindings.md) — consumer-facing notes
  for the UniFFI-generated Kotlin / Swift / TypeScript-over-WASM
  bindings.
- [`docs/adrs/`](docs/adrs/) — architecture decision records.
- [`docs/observations.md`](docs/observations.md) — Wire/MLS observation
  log (resolved changes in [`CHANGELOG.md`](CHANGELOG.md), open items in
  [`open.md`](open.md)).
- [`docs/wire/kalium-readings.md`](docs/wire/kalium-readings.md) — every
  place where Wire's Kalium source was read, with its source and what
  Kupferdraht did with it; [`docs/wire/provenance-audit-2026-10.md`](docs/wire/provenance-audit-2026-10.md)
  — the structural comparison against Kalium made before publication.

## What it does

Kupferdraht is limited to Wire's messaging feature set (text + files),
end-to-end encrypted with MLS (RFC 9420):

- **Account & login**
  - registration with e-mail activation
  - password login (incl. second-factor codes)
  - interactive SSO login
  - device-limit recovery during login (list / free a slot, then continue on the same handle)
  - multi-account device registry with offline resume
  - two processes on one home coexist (per-account cross-process lock around token refresh and key creation, fail-fast; databases wait for each other's writes)
  - logout, optionally purging the device
  - password / e-mail / display-name / handle changes
- **Devices**
  - list, rename, and revoke registered devices
  - stable device identity that survives re-login
  - session-expiry detection with a one-shot re-login gate
- **Rooms**
  - create (self-establishing MLS group at epoch 0)
  - 1:1 DMs with idempotent lookup-or-create
  - invite / kick members
  - join via MLS external commit (one room, all rooms, or re-join after a device switch)
  - self conversation (notes-to-self, one MLS group of the user's own devices): established by the first device, joined by the others, looked up via `self_room`
  - leave, rename, team-admin delete for everyone
  - member lists derived from the MLS leaves
  - backend-mediated administration via the External Senders extension
- **Access & guest links**
  - issue / fetch / revoke reusable guest links (optionally password-protected)
  - join via link (links of the own operator)
  - show / set a room's access modes and roles
- **Text messaging**
  - plain text, with MLS auto-join on send
  - replies / quotes and @-mentions
  - edits and delete-for-everyone (tombstoned, reply chains stay renderable)
  - emoji reactions
  - read receipts and delivery status
  - knock and location messages
  - self-deleting (ephemeral) text with a sender-chosen timer — Wire's official clients show the countdown; inbound ones self-redact locally when the timer elapses
  - failed-send recovery: a send that failed offline stays a listable `Failed` row and is re-driven explicitly under the same message id (text, replies, mentions, ephemeral, knock, location, files) — no background scheduler; the reconnect contract is the consumer's loop
  - typing indicators
  - local timeline reads plus diff-based timeline observers
- **File attachments**
  - send and receive any file via Wire's `/assets` endpoint (AES-256-CBC, MLS-protected reference)
  - SHA-256 verification on download
  - path-based or streamed from a caller-supplied source
  - gated by the team's `fileSharing` policy
  - AES-GCM assets tolerated on receive
- **Tasks / todos**
  - assignee, due date, priority, status changes, re-assignment
  - manual ordering inside any conversation
  - per-room and cross-room views ("my open work" / "what I delegated")
  - event-sourced over MLS — the server never sees task content; Wire's official clients render a plain-text fallback
- **Sync**
  - full and metadata-only sync
  - continuous polling loop with per-event acknowledgement
  - live push receive over WebSocket — the default on native targets (ADR-0033; `SyncTransport::Polling` / `sync-stream --transport polling` opt out), and the browser's live sync — the socket auto-negotiates per connection between `/v14/events` and Wire's legacy `/await` route (ADR-0019), so push works on wire-server deployments where ConsumableNotifications is disabled; without a socket the loop keeps draining
  - in-order commit processing, stale-epoch retry, and auto-commit of external proposals
- **Server-administered config**
  - read-only awareness of team feature configs and user properties (e.g. file-sharing policy, receipt mode)
  - refreshed at login and via events
- **Security**
  - MLS (RFC 9420) only, with no security-disabling options
  - whole-DB encryption via SQLite3MC (ChaCha20-Poly1305, raw 32-byte key per database; vendored amalgamation, no OpenSSL) — an accepted deviation from BSI TR-02102-1's AES-only lists, see ADR-0021
  - OS-keyring, file, in-memory, and IndexedDB keystores
  - compiler-enforced zeroization of sensitive material
  - Proteus prekeys generated and uploaded at registration, never used to encrypt
- **Platforms**
  - matrix-rust-sdk-shaped Rust library
  - Rust CLI (~50 commands, the API's first consumer)
  - UniFFI bindings for Kotlin (Android / JVM / KMP) and Swift (iOS / macOS)
  - browser target: the same crate compiles to `wasm32` with the same SQLite3MC-encrypted storage on sqlite-remote-vfs — in IndexedDB, or an account's databases on a sqlite-remote-server with storage tokens from a token exchange (dedicated Web Worker, cross-origin isolated page; ADR-0022, ADR-0035), recovery data — the account's keys sealed with a PIN through a PIN service that limits the guesses, restored in a new browser — and leaving a browser for good with `logout({ purgeDevice, purgeLocal })` and a hand-written TypeScript-facing binding layer (login incl. SSO, sync, send/receive, assets) — see [`docs/bindings.md`](docs/bindings.md)
  - many tabs, one instance in the browser: the tab holding the origin's lease is the leader, every other tab is a thin client over `BroadcastChannel` — `bindings/web/kupferdraht-tabs/` (opt-in reference module, ADR-0026); closing the leader promotes the next tab, in-flight mutations fail typed instead of being sent twice
  - a self-hosted deployment of wire-server, Wire's open-source backend (API v14), named by its base URL (`--api-url`, required; no default)
  - Kupferdraht's logs reach the app through a listener on every surface (Rust, Kotlin, Swift, browser `onLog`) — Kupferdraht writes nowhere itself

On the wire, every feature above is one `GenericMessage` Protobuf.
Kupferdraht implements nine of the content variants of Wire's
`GenericMessage` schema — `Text`,
`Knock`, `Asset`, `Location`, `MessageDelete`, `MessageEdit`,
`Confirmation` (READ + DELIVERED), `Reaction`, `Ephemeral` — plus its
own `TaskPayload`, `CasePayload` and `TaskListMarker` extensions; every
other variant is decoded, logged, and discarded. The complete
catalogue, including a per-variant scope verdict, is
[`docs/wire/message-types.md`](docs/wire/message-types.md).

How it all fits together is described in
[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md).

## Known limitations

- **No voice / video calls.** Calling is a hard non-goal — Kupferdraht is
  messaging only. Also out of scope: asset preview thumbnails,
  AES-GCM asset emission, and federation across non-Wire backends.
  The MLS stack is RFC 9420 only, with no security-disabling options.
- **Wire's hosted service is for Wire's official apps.** Wire is a
  trademark of Wire Swiss GmbH; Kupferdraht is an independent project,
  not affiliated with, endorsed by or supported by Wire Swiss GmbH.
  Wire's published Terms of Use reserve access to Wire's hosted service
  for Wire's own apps; read the current terms before pointing any
  third-party client at an account there — doing so is your decision
  and your responsibility. Kupferdraht is developed and tested against
  self-hosted deployments of wire-server, Wire's open-source backend
  (API v14), which is the supported target. The CLI talks only to the
  backend named by `--api-url`, which is required: no default, no
  preset for Wire's hosted service, so nobody reaches Wire's servers
  by accident.
- **Typing-indicator receive needs a live socket.** Wire delivers
  `conversation.typing` only on the WebSocket. Native targets receive
  it on the push transport (the default since Step 118; an app that
  chooses `SyncTransport::Polling` misses it); the browser receives it
  through its push socket (Steps 115, 115b).

## Bindings status

The public Rust API is FFI-mirrored by convention (a `pub` change
lands with its binding mirror in the same commit), so the features
above are reachable from every UniFFI target. Nothing is published
to a package registry yet — each artifact is built from source via
the scripts under [`bindings/`](bindings/):

| Target | Artifact / build | Status |
|---|---|---|
| Rust (library + CLI) | `kupferdraht-core` crate, `kupferdraht-cli` | usable today — the CLI exercises every feature |
| Kotlin / JVM (desktop) | JAR — `bindings/kotlin/build-jvm.sh` | implemented, binding-level tests (`./gradlew test`) |
| Android | AAR (arm64-v8a) — `bindings/android/build-android.sh` (NDK + cargo-ndk) | implemented, builds + packages |
| Swift (iOS + macOS) | `kupferdrahtFFI.xcframework` + SwiftPM — `bindings/swift/build-xcframework.sh` | implemented, binding-level tests (`swift test`) |
| Kotlin Multiplatform | `io.github.schwarzdigits:kupferdraht-bindings` (`jvm`, `androidTarget`, `iosArm64`, `iosSimulatorArm64`) | implemented, consumed downstream via Gradle composite build |
| Browser (WASM) | `wasm-pack build --target web` (from `kupferdraht-core/`, needs a wasm-capable clang) + `bindings/web/kupferdraht-tabs/` (plain ES modules, no build) | usable subset — login incl. SSO, sync, send / receive, assets over the OPFS-backed SQLite3MC store (run in a dedicated Web Worker); many tabs served by one leader tab (`kupferdraht-tabs`, ADR-0026); storage pins and the tabs scenarios run in headless Firefox (`scripts/wasm-test.sh`), live-browser verification of the login vertical pending |

Per-target surface, packaging details, and the gaps on the wasm
surface: [`docs/bindings.md`](docs/bindings.md).

## Getting started

The fastest way to see Kupferdraht work is the CLI — log in, sync, send:

```sh
# build everything (prerequisites: see "Building" below)
cargo build --workspace

# log in against your self-hosted wire-server (password prompted on the TTY)
cargo run -q -p kupferdraht-cli -- login --api-url https://nginz-https.example.com -e <email>

# pull conversations + events, then list the rooms
cargo run -q -p kupferdraht-cli -- sync
cargo run -q -p kupferdraht-cli -- rooms

# send a first message (room id is `<uuid>@<domain>` from `rooms`)
cargo run -q -p kupferdraht-cli -- send --id <room-id> -m "Hello from Kupferdraht"

# follow incoming messages continuously (Ctrl-C to stop)
cargo run -q -p kupferdraht-cli -- sync-stream
```

`--api-url` is required and takes a self-hosted deployment of wire-server,
Wire's open-source backend (API v14);
there is no preset for Wire's own hosted service, whose Terms of Use
restrict third-party clients (see "Known limitations"). The full
command reference is
[`docs/cli.md`](docs/cli.md); for embedding Kupferdraht as a library see
[`docs/library.md`](docs/library.md) (Rust) and
[`docs/bindings.md`](docs/bindings.md) (Kotlin / Swift / browser).

## Building

Prerequisites:

- Rust 2024 edition on the toolchain pinned in `rust-toolchain.toml`
  (1.98 today; rustup installs it automatically — [rustup](https://rustup.rs/)).
  CI builds with the same pin.
- `cargo` (bundled with rustup).
- No system `protoc` — the build takes its `protoc` binary from the
  `protoc-bin-vendored` crate to compile the Protobuf schema.

Workspace build:

```sh
cargo build --workspace
```

Browser build (wasm-bindgen surface, from `kupferdraht-core/`; the SQLite3MC
C needs a clang with the WebAssembly backend — Apple's has none, Homebrew
LLVM does):

```sh
export CC_wasm32_unknown_unknown=/opt/homebrew/opt/llvm/bin/clang
export AR_wasm32_unknown_unknown=/opt/homebrew/opt/llvm/bin/llvm-ar
wasm-pack build --target web
scripts/wasm-test.sh            # the browser tests, headless Firefox
```

### Mandatory pre-commit gate

Every commit that lands on `main` must pass all five gates:

```sh
cargo fmt --all -- --check                         # format drift = 0
cargo build --workspace 2>&1 | grep -c "warning"   # must print 0
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --no-fail-fast
cargo deny check
```

The `cargo deny check` step enforces Kupferdraht's license allow-list and
RustSec advisory policy (see [`deny.toml`](deny.toml)). Install
cargo-deny once per machine:

```sh
cargo install --locked cargo-deny
```

`CLAUDE.md` lists the same five commands as the non-negotiable
pre-commit contract.

### Third-party licenses

[`THIRD-PARTY-NOTICES`](THIRD-PARTY-NOTICES) is the attribution file an
embedder reproduces: every component with license, authors and
repository, grouped by license, plus the components outside the crate
graph and the trademark note.
[`docs/THIRD_PARTY_LICENSES.html`](docs/THIRD_PARTY_LICENSES.html)
enumerates every crate Kupferdraht links against, grouped by license,
plus a hand-kept section for the two components outside the crate
graph: the vendored SQLite3MC engine (MIT) and the git-pinned
`sqlite-remote-vfs` (Apache-2.0), which cargo-about skips as private.
Regenerate it when the dependency graph changes:

```sh
cargo install --locked cargo-about   # once per machine
cargo about generate about.hbs > docs/THIRD_PARTY_LICENSES.html
```

`about.toml`'s accepted-license set is kept in sync with
`deny.toml` — any drift between the two is a review defect.

### Generating API docs

`docs/library.md` and `docs/bindings.md` are the hand-written
API references. For a per-symbol HTML reference generated from
the inline doc-comments, run:

```sh
cargo doc --workspace --no-deps --document-private-items --open
```

`--no-deps` skips the transitive dependency graph and keeps the
build under a minute. Drop `--open` for scripted runs.

To fail on broken intra-doc-links (e.g. `[Type]`, `[Self::method]`
references that no longer resolve) — recommended before pushing
public-API changes:

```sh
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
```

This is **not** part of the five mandatory pre-commit gates above
— it is an opt-in check for API-surface changes.

## Running the CLI

```sh
cargo run -q -p kupferdraht-cli -- login --api-url https://nginz-https.example.com -e <email>
cargo run -q -p kupferdraht-cli -- status
```

Passwords are read from the TTY; use `--password-stdin` for
non-interactive runs. See [`docs/cli.md`](docs/cli.md) for the full
command reference as it grows.

## How to contribute

Issues and pull requests are welcome, with no promise of timely
review. Contributions follow the Schwarz Digits open-source guidelines
([`CONTRIBUTING.md`](CONTRIBUTING.md)); the Contributor License
Agreement is signed on the first pull request.

- Read [`CLAUDE.md`](CLAUDE.md) first — it is the binding contributor
  charter: architecture invariants, coding conventions, and the
  12-phase step procedure every change follows.
- **Provenance rule:** Kupferdraht is written from the Wire v14 Swagger
  spec, RFC 9420 and live observation of Wire v14 backends. Wire's own
  source code (Kalium or any other Wire repository) is never a model for
  structure, naming or implementation and is never transcribed; it may
  be read to observe protocol behaviour where the spec is silent or an
  interoperability failure needs explaining, and every such reading is
  recorded with its source in `docs/wire/api-notes.md` or
  `docs/observations.md` and indexed in `docs/wire/kalium-readings.md`.
  The one
  Wire-authored artifact in the tree is declared as such: the Protobuf
  schema under `kupferdraht-core/proto/` is a modified subset of Wire's
  GPL-3.0-or-later `GenericMessage` schema and keeps Wire's notice (see
  "License").
- Every commit must pass the five pre-commit gates listed under
  "Building".
- Branch per step (`step/NN-slug`), Conventional-Commits messages
  (`type(scope): subject`), library + CLI land in one commit.

## License

GPL-3.0-only (see [`LICENSE`](LICENSE)); attribution and the notices of
the components in [`NOTICE`](NOTICE) and
[`THIRD-PARTY-NOTICES`](THIRD-PARTY-NOTICES).
Every source file Kupferdraht authors carries the matching SPDX notice;
the untagged exceptions are listed in [`CLAUDE.md`](CLAUDE.md) "License
Headers".

Copyright (C) 2026 Schwarz Digits KG. Originally developed by
Volker Zöpfel ([@reklov](https://github.com/reklov)), with contributions
from Markus Troßbach ([@mtrossbach](https://github.com/mtrossbach)) and
Sven Braune ([@sbra0902](https://github.com/sbra0902)).

This repository was published as a single snapshot. The complete
development history since April 2026 is retained privately by
Schwarz Digits KG and is available to answer provenance questions.

Kupferdraht's transitive dependencies are all permissive
(Apache-2.0 / MIT / BSD-style / ISC / MPL-2.0 / Unicode), so Kupferdraht
itself is the only copyleft component in the combined work — the GPL
is a deliberate choice, not one inherited from the dependency tree.
`cargo deny check` enforces this against
[`deny.toml`](deny.toml)'s allow-list — a copyleft pull into the
dependency graph is a build-stopper, not a follow-up.

Kupferdraht's public API follows the conceptual model of
[matrix-rust-sdk](https://github.com/matrix-org/matrix-rust-sdk)
(Apache-2.0), with thanks; no code was taken from it.

The Protobuf schema `kupferdraht-core/proto/generic_message.proto` is a
modified subset of Wire's `GenericMessage` schema (Copyright (C) Wire
Swiss GmbH, GPL-3.0-or-later): the declarations Kupferdraht needs for
interoperability, with Wire's notice kept intact and the modifications
stated in the file's header, as GPL-3.0 §§ 4–5 require. GPL-3.0-or-later
material combines into a GPL-3.0-only work; the whole is GPL-3.0-only.

Wire publishes its software under GPL-3.0 and AGPL-3.0. Where Wire's
work is incorporated into Kupferdraht it keeps Wire's notice, as the
schema above shows. Should any other part of this tree be found to
carry Wire's expression, it is to be attributed the same way: report it
to <opensource@digits.schwarz> and the notice will be added.

### Trademarks

"Wire" is a trademark of Wire Swiss GmbH. Kupferdraht is an independent
project that implements the Wire v14 protocol and is not affiliated with,
endorsed by or supported by Wire Swiss GmbH; the name is used only to
describe compatibility. "Kupferdraht" and the other marks of Schwarz
Digits KG are covered by the Schwarz Digits trademark policy,
<https://github.com/SchwarzDigits/.github/blob/main/TRADEMARKS.md>; the
GPL grants no rights to use them.

Kupferdraht is experimental software, provided as-is with no warranty
or support commitment (GPL-3.0 §§ 15–16). Do not rely on it in
production without your own security review.
