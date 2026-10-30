# Step 117 — Kupferdraht's logs reach the app through a listener

**Status: MERGED 2026-10-01** — commissioned 2026-10-01 (Markus — gap G11
of the natrium web SDK's list: in the browser nobody sees Kupferdraht's logs).
`edc1bf6`. Merged to `main` as PR #104 (`f919f64`); CI green. Smoke
2026-10-01 against the test stage (release wasm). Scope shipped:
`init_logging(level, listener)` on every surface (Rust, UniFFI, wasm
`Kupferdraht.open({ logLevel, onLog })`, kupferdraht-tabs `serveTab`); only Kupferdraht's
own targets; Kupferdraht writes nowhere itself. Retrospective:
`docs/LEARNINGS.md` §5.60. Archived with this header.

## Problem

Kupferdraht logs through `tracing` (113 call sites: storage reconnecting, a
storage-token renewal failing, the push socket falling back to the timer,
…). Natively the CLI installs a subscriber; nothing else does. In the
browser, and in a Kotlin or Swift app, those events go nowhere: an
embedder debugging a problem sees nothing of Kupferdraht. Step 98 left the
log-level hook out (G11) because the core had no logging API to bind.

## Decisions (2026-10-01, Markus)

- **The product decides where logs go; Kupferdraht writes nowhere itself.**
  Kupferdraht offers a listener; without one it stays silent, as today. No
  console output of Kupferdraht's own — an app that wants the console passes a
  listener that writes there (three lines, shown in the guide).
- **Every target:** the browser and the UniFFI bindings (Kotlin, Swift).
  The CLI keeps its own subscriber (`RUST_LOG`).
- **In kupferdraht-tabs the listener runs in the leader's worker;** nothing is
  forwarded to the page.

## Design

- **Core, `logging` module** (matrix-rust-sdk's `LogLevel` vocabulary):
  - `LogLevel` — `Error`, `Warn`, `Info`, `Debug`, `Trace`;
  - `LogRecord { level, target, message }` — `message` is the event's
    message with its other fields appended as ` key=value`;
  - `LogListener` — one method, `on_log(record)`;
  - `init_logging(level, listener)` — installs Kupferdraht's `tracing`
    subscriber on the first call and hands every event of Kupferdraht's own
    targets (`kupferdraht::…`) at `level` or above to `listener`. Events of
    dependencies (hyper, rustls, openmls, …) are not forwarded. A later
    call replaces level and listener (a debug switch at runtime, a second
    `Kupferdraht.open` in the same worker). `InvalidInput` when another
    `tracing` subscriber is installed already (an app that brings its own
    keeps it).
  - One implementation for every target: the listener is `Send + Sync`
    and held in the subscriber's shared state; in the browser the JS
    function is wrapped in `SendWrapper` (already a dependency of the wasm
    build — Kupferdraht runs on one thread there). Dependency: `tracing-subscriber`
    (registry only, no default features; already in the lockfile).
  - Secrets: `Debug` of secret-carrying types is redacted since Step 101;
    log messages carry no tokens or keys.
- **Browser:** `Kupferdraht.open({ logLevel?, onLog? })` — with `onLog`, logging
  is set up with `logLevel` (default `"warn"`); `onLog({ level, target,
  message })` runs in Kupferdraht's worker. A throwing `onLog` is ignored (the
  app's logger must not break Kupferdraht). `logLevel` without `onLog` changes
  nothing. kupferdraht-tabs: `serveTab({ logLevel, onLog })` passes both on.
- **UniFFI:** `init_logging(level, listener)` and the callback interface
  `LogListener` (Kotlin: forward to logcat; Swift: to `os_log`).
- **Rust API (native embedders):** `kupferdraht::logging::init_logging` with an
  `Arc<dyn LogListener>`.

## Tests (red first)

- Natively (`logging` unit tests, a scoped subscriber per test): a
  `warn!` reaches the listener with level, target and message; an event
  below the level does not; the other fields follow the message as
  ` key=value`; an event of another crate's target is not forwarded; the
  level can be raised and lowered at runtime.
- An integration test (its own process): `init_logging` installs the
  subscriber; a second call replaces the listener; with another
  subscriber installed first it is `InvalidInput`.
- Pins: the wasm binding reads `onLog` / `logLevel`; the UniFFI export and
  the callback interface; the tabs `.d.ts` (`serveTab` options); the
  embedder guide.
- Smoke: the browser smoke page passes an `onLog` that forwards to the
  page; a storage outage shows Kupferdraht's `warn` there.

## As built (2026-10-01)

- The core owns the listener and its value types, so `LogLevel`,
  `LogRecord`, `LogListener` and `init_logging` carry their `UniFFI`
  annotations in `logging.rs` — as `SecureKeyStore` does — instead of an
  `ffi/` twin; the pin follows. The browser's adapter is `wasm/logging.rs`
  (`wasm/manager.rs` is large already); the pin follows that too.
- `tracing-subscriber` (registry only) carries one layer: it asks Kupferdraht's
  callsites every time (`Interest::sometimes`), so a level change applies
  at once, and never asks a dependency's. The listener is called outside
  the sink's lock — a listener that logs again cannot deadlock.
- A second `init_logging` replaces level and listener; only the first
  installs the subscriber, under a mutex, so two first calls cannot race.
- Smoke 2026-10-01 (test stage, release wasm): `log=debug` forwards
  Kupferdraht's debug events (`kupferdraht::wire::http`), no dependency's target
  arrived, `log=none` (no `onLog`) gave no event at all, and a warning
  (`kupferdraht::sync::removal_flush`) reached the page. The first `log=debug`
  run showed only warnings: the smoke page did not pass `&log=` on to the
  worker's URL — the harness, not the product (a browser probe test passed
  debug, info and warn).
- Found by that warning: a send in the browser's DM failed with
  `GroupStateError(PendingProposal)` — a removal flush that failed is lost
  when the client is dropped (the flush queue is in memory), so the stored
  proposal blocks every send until another removal arrives. Not this step's;
  `open.md` §73.
- No CLI command: the CLI keeps its own subscriber (`RUST_LOG`).
