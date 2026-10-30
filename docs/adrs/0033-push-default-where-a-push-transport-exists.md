# ADR-0033: Make push the default receive transport wherever a target has one

**Status:** Accepted
**Date:** 2026-09-28
**Deciders:** Volker Zöpfel (proposed by Markus Troßbach)

## Context

Kupferdraht receives through one of two transports (`SyncTransport`):
`Polling` drains `GET /v14/notifications` in a paced loop; `WebSocket`
holds a push socket, negotiated per connection between `/v14/events`
and the legacy `/await` socket (ADR-0019). ADR-0017 introduced push as
opt-in with polling the default; ADR-0019 kept that and rejected "push
by default everywhere" because it reversed ADR-0017, because the browser
build has no socket, and because it would surprise consumers that sized
their runtime for polling.

Two things changed since. ADR-0030 made the legacy socket a doorbell: a
frame only triggers the same REST drain the polling loop runs, so push
shares the drain's cursor, dedup and persistence guarantees (ADR-0031,
ADR-0032) and adds no receive path of its own. Step 103 gives polling a
default pause of 5 s, which makes the cost of the polling default
explicit: one drain per device every 5 s while idle, up to 5 s receive
latency. Push receives within a second and idles on one pinged socket.

Constraints: the push module compiles on native targets only; the
browser build polls (its live sync is a `sync_once` loop with a fixed
pause), and a browser push transport is not planned in this repository
today. `sync_once` ignores the transport — it is what an app runs from a
background wake-up. UniFFI records carry no defaults today, so Kotlin
and Swift callers already name the transport; the Rust `Default`
reaches Rust consumers (CLI, tests, Rust apps).

## Options Considered

### Option A — keep polling the default (status quo)

- Pro: no behaviour change; one default on every target.
- Con: a consumer that does not choose gets 5 s latency and a request
  every 5 s per device; the better transport stays opt-in knowledge.

### Option B — push wherever the target has a push transport, polling elsewhere

`SyncTransport::default()` is `WebSocket` on every target that compiles
a push transport and `Polling` on the others — today: push on native
targets, polling in the browser. A browser push transport, once it
lands, inherits the default without a new decision.

- Pro: sub-second receive and near-zero idle load out of the box; no
  browser regression; `Polling` stays one explicit setting away; the
  rule does not depend on whether or when browser push arrives.
- Con: the default differs between targets until every target has
  push, and must be documented as such; the default changes for existing
  Rust consumers; one long-lived socket per running sync loop.

### Option C — push by default everywhere, unconditionally

- Pro: one default on every target.
- Con: not possible today — the browser build has no push transport;
  it is where Option B ends up once one exists.

## Decision

**Option B.** Since ADR-0030 push and polling run the same
drain; the transport only decides *when* to drain — on a doorbell or on
a timer — and the doorbell is the better trigger wherever a socket
exists. Of ADR-0019's objections, the browser keeps polling until it
has a push transport, and consumers sized for polling set `Polling`
explicitly (CHANGELOG, BREAKING).

Resolved 2026-09-29: the `transport` field of `SyncSettings` gets no
UniFFI default — UniFFI 0.29.5 record defaults are literals, `None`,
`Some(literal)` or `[]` (`uniffi_macros` `src/default.rs`), neither an
enum variant nor the Rust `Default` impl. Kotlin and Swift callers keep
naming the transport; their docs and examples switch to `WebSocket`. An
`Option<SyncTransport>` shape ("`None` = the target default") would be a
decision of its own.

## Consequences

**Positive**

- Sub-second receive by default on every target with push.
- Idle load drops from a drain every 5 s to a socket ping.

**Negative**

- The default changes for Rust consumers that relied on it (BREAKING).
- One long-lived connection per running sync loop.
- Until the browser has push, the default differs between native and
  browser builds.

**Neutral**

- Mobile background receive is unchanged: OS push wakes the app, which
  runs `sync_once`.
- `SyncSettings` keeps its shape.

## Implementation Notes

- Replace `#[default]` on `SyncTransport` with a `Default` impl gated on
  the same `cfg` that compiles the push module (today
  `not(target_arch = "wasm32")`), so the default moves with the push
  transport.
- `kupferdraht-cli sync-stream --transport` defaults to `websocket`.
- The "polling stays the default" rules of ADR-0017 and ADR-0019 are
  superseded (2026-09-29); their rule files point here. The code change
  is a step of its own — until it lands, `SyncTransport::default()` in
  code is still `Polling`.

## References

- Supersedes, on the polling default: ADR-0019 ("Polling stays the default
  and the wasm-only path") and ADR-0017 (push "opt-in", "`Polling` stays
  the default").
- Builds on ADR-0030 — the socket only rings; the drain does the work.
- Related: ADR-0031 and ADR-0032 (the drain's serialisation and
  persistence guarantees push inherits); ADR-0015 (WASM is the browser
  path).
- `docs/plan/103-polling-load.md` (the 5 s default pause); open.md §71 H8;
  open.md NG-14 (browser delivery by polling).

## Revision History

- 2026-09-28: Created (Proposed — Markus; for Volker's decision)
- 2026-09-29: Accepted (Volker). The UniFFI question is closed (record
  defaults cannot name an enum variant in 0.29.5); implementation is a
  step of its own.
- 2026-10-01: Implemented in Step 118 — `SyncTransport::default()` is
  `WebSocket` under `not(target_arch = "wasm32")`, `sync-stream` defaults
  to `websocket`; the push loop drains without a socket, so the default
  never depends on the socket opening.
