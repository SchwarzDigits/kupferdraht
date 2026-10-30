# ADR-0026: Serve every tab of an origin from the one active instance — a leader tab elected by the lease queue, thin-client tabs over BroadcastChannel

**Status:** Accepted
**Date:** 2026-09-25
**Deciders:** Volker Zöpfel

## Context

This ADR **extends** ADR-0023 "Enforce one active Kupferdraht instance per
browser origin with a Web Locks lease and explicit takeover"; it
supersedes nothing. ADR-0023 gives a browser origin exactly one active
Kupferdraht instance, held by the dedicated worker that owns the Web Locks
lease `kupferdraht:instance`; every further tab is refused ("already open in
another tab") and can only *take over*. Its stated negative consequence
— "no concurrent read-only tab in V1; a user who wants Kupferdraht in two
tabs gets a takeover dialog, not two views" — is the problem now: a web
app whose users open several tabs wants all of them to work, and a
takeover dialog per tab is not a product. The rationale for deciding
now: the wasm/web consumer starts on the Step-92 surface, and Step 79's
lesson applies one level up — a coordination protocol Kupferdraht only
documents is a protocol every embedder re-implements differently.

The facts the decision rests on, verified against the platform on
2026-09-25 (W3C Web Locks API, MDN compat data, wasm-bindgen 0.2.128
sources): a Web Locks request without `ifAvailable` is **queued and
granted in FIFO order** when the holder releases; the browser releases
a holder's locks and aborts its queued requests **when its agent
terminates** (tab closed, `worker.terminate()`), and every dedicated
worker is its own agent — the queue is therefore a crash-safe election
with no heartbeat needed for correctness. `steal` releases the held
lock (the holder's request promise rejects with `AbortError`, the
eviction signal Step 92 already relies on) and **prepends** the
stealing request, so a steal is granted before anything queued.
`ifAvailable` is never queued. A lock is **not reentrant** within one
agent (observations §81): a worker whose JavaScript holds
`kupferdraht:instance` cannot pass `Kupferdraht.open`'s own `ifAvailable` request
— the plan's "queue in JS, then open on grant" needs an explicit
hand-off. `BroadcastChannel` is origin-scoped, exposed in windows and
dedicated workers alike, structured-clones its payloads (no transfer
list; `Blob`, `ArrayBuffer`, typed arrays are cloned) and never
delivers a message to the object that sent it; compat Chrome 54 /
Firefox 38 / Safari 15.4. The effective browser floor is already Chrome
102 / Firefox 111 / **Safari 15.4** — the Web Locks lease of ADR-0023
lifted Safari from ADR-0022's 15.2 — so nothing here raises it.
`SharedWorker` hosting stays closed (ADR-0023 Option C: no
`FileSystemSyncAccessHandle` outside dedicated workers, no
`SharedWorker` on Chrome for Android). The wasm surface's `startSync`
delivers per-message, typing and session-expiry callbacks; it has no
per-pass summary and no room-list or timeline-diff events (those exist
on native only), and the registry's `accountId` *is* the client's
`userId`. A wasm-bindgen local snippet (`module = "/…"`) is copied next
to the generated glue with its directory structure preserved, so the
browser test harness (`scripts/wasm-test.sh`) can load a JS module that
lives outside the crate without a bundler.

Constraints in effect: ADR-0015 Part 3 (the hand-written wasm-bindgen
surface is the contract — nothing moves into Rust that JS can do),
ADR-0022 decision 3 (dedicated worker, fail closed), ADR-0023 (the lease
mechanics stay as they are: `ifAvailable` open, `steal` takeover,
`onEvicted`, the embedder terminates the evicted worker), ADR-0011
(Step 79's failed-send recovery is the honest resend path — no send may
reach the backend twice), permissive-only dependencies (no bundler, no
runtime dependency for the module), and the Step-94 guard rail that the
single-tab path (`Kupferdraht.open` directly, no proxy) stays byte-identical
— the module is opt-in.

## Options Considered

### L1 — who ships the thin-client layer

#### Option L1-A: Kupferdraht ships a reference module `bindings/web/kupferdraht-tabs/`

Leader election, leader bootstrap, the RPC dispatcher, the thin-client
proxy and the event fan-out as one ES-module package next to
`opfs-smoke`, tested by the browser test script and proven by the
consumer smoke page; embedders import it.

**Pros:**
- One implementation of the protocol, tested where the wasm surface is
  tested; the parity test (below) keeps it in step with the surface.
- The Step-79 lesson: what the core does not ship, every consumer
  rebuilds differently (kupferdraht-tui rebuilt resend).
- The module is opt-in; `Kupferdraht.open` stays the single-tab path.

**Cons:**
- A JavaScript deliverable in a Rust repository: its own conventions,
  its own test harness inside `scripts/wasm-test.sh`, its own
  freshness duty in `docs/bindings.md`.
- Shipping **TypeScript source** would need a transpile step nothing
  in the repository has (`opfs-smoke` is build-free); the choice below
  is plain ES modules typed by JSDoc plus a hand-written `index.d.ts`.

#### Option L1-B: Kupferdraht documents the protocol only

**Pros:**
- No JavaScript to maintain in this repository.

**Cons:**
- Every embedder implements election, hand-off, dedup and failover
  themselves; the honest failover semantics (L3) are exactly the part
  that gets it wrong.

### L2 — transport between thin tabs and the leader

#### Option L2-A: `BroadcastChannel` — one request channel, one response channel per tab

Requests go on the origin-wide channel `kupferdraht:tabs` as
`{ kind: "request", tabId, requestId, target, method, args }`; the
leader answers on `kupferdraht:tabs/<tabId>` with `{ kind: "response",
requestId, ok, value | error }`; events and the leader announcement go
on `kupferdraht:tabs` to everyone. The leader answers each `(tabId,
requestId)` once (dedup); a tab accepts only responses on its own
channel.

**Pros:**
- No port hand-off, no rendezvous: a channel is opened by name in any
  same-origin context, including the leader's worker.
- Structured clone carries every payload the wasm surface returns
  (plain objects, `Uint8Array` asset bytes).
- The per-tab response channel avoids the N-fold clone a single shared
  channel would cost (every tab cloning every other tab's responses —
  a `downloadAsset` of 10 MB across five tabs would be 50 MB of copies).

**Cons:**
- Still a copy per hop (no transferables); large assets are copied
  once into the leader and once to the requesting tab.
- Ordering across the two channels is not guaranteed by the platform;
  the protocol must not depend on event-before-response ordering.

#### Option L2-B: `MessagePort` pairs brokered through the leader

**Pros:**
- Point-to-point, transferable `ArrayBuffer`s, no per-tab filtering.

**Cons:**
- The port hand-off itself needs a rendezvous over `BroadcastChannel`
  (the tabs cannot reach the leader's worker otherwise), so it is a
  second protocol on top of the first; every leader change re-brokers
  every port.

### L3 — failover semantics for calls in flight

#### Option L3-A: reads re-issue, mutations fail typed, nothing is auto-resent

A static classification in the protocol module: **idempotent** calls
(`accounts`, `rooms`, `messages`, `members`, `downloadAsset`,
`storageEngine`, `resume`, `syncOnce`, `initiateSso`,
`initiateSsoByEmail`) are re-issued transparently to the new leader
after a leader change; **mutations** (`login`, `finalizeSso`,
`registerDevice`, `sendText`, `sendReply`, `sendFile`, `toggleReaction`,
`editMessage`, `redact`, `markAsRead`, `createRoom`, `createDm`,
`invite`, `kick`, `leave`, `logout`, `startTyping`, `stopTyping`) in
flight reject with a typed `LeaderChanged` error and are never retried
by the module. No offline queue: a call while no leader is known waits
a bounded time for the first announcement, then rejects typed
(`NoLeader`).

**Pros:**
- Honest: a send that reached the backend before the leader died is
  not sent twice; the row shows `Failed` or it does not (Step 79's
  contract), and the leader's own resend path is the documented way.
- The classification is data, pinned by the parity test.

**Cons:**
- The embedder must handle `LeaderChanged` on every mutation (retry is
  a user decision, exactly as after a network error).

#### Option L3-B: retry every in-flight call on the new leader

**Pros:**
- Nothing for the embedder to handle.

**Cons:**
- Duplicates every mutation whose request reached the old leader before
  it died — a message sent twice, a device registered twice.

#### Option L3-C: idempotency keys per mutation

**Pros:**
- Safe automatic retry.

**Cons:**
- Needs a Rust-side dedup store keyed by a client-chosen key across
  every mutating call — a later step (noted in `open.md`), not this one.

### L4 — election and leader bootstrap

#### Option L4-A: a queued JS request per tab, hand-off to Rust by self-steal

Every tab's host keeps one dedicated *candidate* worker with a single
pending `navigator.locks.request("kupferdraht:instance", { mode:
"exclusive" }, …)` (no `ifAvailable`). The grant makes the worker the
leader: inside the grant callback — while its JavaScript still holds
the lease, so nothing queued can slip in — it loads the wasm module and
calls `Kupferdraht.open({ takeOver: true, … }, onEvicted)`. That steal moves
the lease from the candidate's JavaScript to the Rust instance without
a gap (steal preempts the queue; the candidate's own request rejects
with the expected `AbortError`, which the host ignores for its own
hand-off), and the takeover's bounded retry (five seconds) also covers
a just-terminated leader whose OPFS handles linger. `Kupferdraht.open`
failing releases the lease (the callback settles) and the next
candidate is granted. ADR-0023's manual takeover stays the override for
a hung leader: the host aborts its own queued request (`signal`) and
opens with `takeOver` directly. An evicted leader's host terminates the
worker and re-enters the queue with a fresh candidate.

**Pros:**
- The lock — released by the browser on tab close, crash or
  termination — is the single source of truth; no heartbeat decides
  anything.
- No change to the lease mechanics of ADR-0023: the queue and the
  hand-off are JavaScript around the existing `takeOver` open.
- A thin tab instantiates no wasm until it wins (a 6–7 MB module per
  tab otherwise).

**Cons:**
- The hand-off rides on the steal semantics — a steal by a *foreign*
  tab in the microseconds between grant and self-steal is stolen back;
  the last stealer wins and the other side is evicted (the same race
  two "use here" clicks have today). Documented, not prevented.
- `takeOver: true` is the bootstrap's normal path, so "takeover" no
  longer implies "a human chose to evict someone".

#### Option L4-B: a queued acquisition mode in Rust (`Kupferdraht.open({ waitForLease })`)

**Pros:**
- One lock request, held by Rust from the start; no hand-off.

**Cons:**
- Every tab instantiates the wasm module just to wait; the lease
  mechanics of ADR-0023 gain a third mode (a change the Step-94 guard
  rail excludes).

#### Option L4-C: heartbeat-based election over `BroadcastChannel`, no lock

**Pros:**
- No Web Locks at all.

**Cons:**
- Split brain under a stalled tab: two leaders opening OPFS, the
  failure ADR-0023 was written to end; the lease exists and is
  crash-safe.

### L5 — what the leader broadcasts

#### Option L5-A: the wasm surface's callbacks, plus one per-pass summary

`message` (the `onMessage` payload — the inbound decrypted item, which
is the timeline diff the wasm surface has), `typing`, `expired`, and
`sync` = `{ roomsUpdated, eventsDispatched }` after every pass of the
leader's sync loop — the one Rust addition, an optional fourth
callback `onSync` on `startSync`. Module-level events: `leader-changed`
`{ leaderTabId, epoch, isSelf }`, `evicted` (this tab's leader was
taken over elsewhere). Thin tabs render from the broadcast and **pull**
(`rooms`, `messages`, `members`) through the proxy; there is no local
store in a thin tab (the OPFS pool is the leader's).

**Pros:**
- Every payload is the one `docs/bindings.md` already documents; the
  summary is the honest "something changed, re-pull" signal for room
  lists without inventing a diff the surface does not have.
- The leader's sync loop runs once per instance; a tab's `startSync`
  is a subscription (ref-counted; the loop stops when the last
  subscriber stops).

**Cons:**
- A room rename or invite without a message reaches a thin tab only
  via the summary's `roomsUpdated` — it re-pulls the list; no per-room
  granularity.
- One wasm surface change (`startSync`'s optional `onSync`), mirrored
  in `docs/bindings.md` and the parity test.

#### Option L5-B: no summary — thin tabs re-pull rooms on every `message`

**Pros:**
- No Rust change.

**Cons:**
- Room-list changes without a message (rename, invite, leave) are
  invisible to thin tabs until they poll.

## Decision

**L1-A, L2-A, L3-A, L4-A, L5-A.** Kupferdraht ships
`bindings/web/kupferdraht-tabs/` — plain ES modules typed by JSDoc with a
hand-written `index.d.ts`, no bundler, no runtime dependency — with:

1. **`createTabHost({ worker, … })`** in every tab: spawns the tab's
   candidate worker, tracks the leader announcement and heartbeat
   (a UI hint only), exposes `state` (`electing` | `leader` | `thin`),
   `takeOver()` (the ADR-0023 override), `close()`, and the events
   `leader-changed`, `leader-unresponsive`, `evicted`, `error`. The
   candidate worker (`serveTab({ load, permitHttp })`, imported by the
   embedder's worker script that knows where `pkg/kupferdraht.js` lives)
   queues for `kupferdraht:instance`, bootstraps by the self-steal hand-off
   on grant, dispatches requests and broadcasts events while it leads.
2. **`KupferdrahtProxy`** (from `host.kupferdraht`): the thin-client face with
   the **same method names as the wasm `Kupferdraht` / `PendingLogin` /
   `Client` / `SyncHandle` classes** — every public method except the
   leader-only `Kupferdraht.open` — returning proxies for the wrapper
   objects the surface returns (getters snapshotted at creation so
   `userId`, `displayName`, `email`, `evicted` stay synchronous).
   A native test enumerates the `#[wasm_bindgen]` method names in
   `kupferdraht-core/src/wasm/` and fails on any name the proxy or the
   `.d.ts` does not mirror — the JS twin of the FFI-parity rule.
3. **Transport** as L2-A; **failover** as L3-A; **election** as L4-A;
   **broadcast set** as L5-A.
4. **Rust changes are limited to** the optional `onSync` callback on
   `startSync`. Lease mechanics, `Kupferdraht.open`'s shape and the
   single-tab path are untouched; a preservation test pins that.

The rationale that carries it: the platform already has a crash-safe
election (the lock queue) and an origin-wide bus (`BroadcastChannel`);
the module adds only the hand-off, the envelopes and the honest failover
rule, and shipping it once is cheaper than every embedder getting the
failover rule wrong.

## Consequences

### Positive

- Every tab of the origin works; closing the leader promotes the next
  tab without user action; the takeover dialog becomes the override
  for a hung leader, not the norm.
- ADR-0023 stays true ("one active instance") and gains "served to
  many tabs"; native targets are untouched (Step 85's per-account lock
  is their counterpart and needs no leader).
- No new dependency, no bundler; the module runs from a
  `<script type="module">` and inside a worker as is.
- The failover contract is explicit: reads survive a leader change,
  mutations fail typed, nothing is sent twice.

### Negative

- Mutations in flight during a leader change fail; the embedder has to
  offer "try again" (or the Step-79 resend path once the surface
  exposes it on wasm). Idempotency keys are a later step.
- Payloads are cloned per hop; a large asset download costs two copies
  and lands in one tab only.
- A thin tab holds a live candidate worker at all times (small, no
  wasm) — one worker per tab is the price of the crash-safe queue.
- Two `takeOver` steals racing (a foreign "use here" between grant and
  hand-off) end with the last stealer as leader and the other evicted;
  the same race ADR-0023 has, now with one more participant.
- A JavaScript module in the repository: JSDoc types instead of
  TypeScript source (no transpile step), a hand-written `.d.ts` kept
  honest by the parity test, its own section in `docs/bindings.md`.
- The `sync` summary is coarse: a thin tab re-pulls the room list when
  `roomsUpdated > 0`.

### Neutral

- Cross-origin, cross-profile and private-window sharing are out
  (`BroadcastChannel` and Web Locks are origin- and partition-scoped).
- A `SharedWorker` host remains closed (ADR-0023 Option C).
- Thin-tab mutations while no leader exists are not queued offline;
  they reject typed after a bounded wait — the embedder shows
  "connecting".
- The browser floor is unchanged (Safari 15.4 effective since
  ADR-0023).

## Implementation Notes

For Step 94 (`docs/plan/archive/94-leader-tab-thin-clients.md`, merged 2026-09-25):

- `bindings/web/kupferdraht-tabs/`: `protocol.js` (channel names, envelope
  shapes, the method table with the idempotent/mutation classification,
  the error classes `LeaderChangedError` / `NoLeaderError`), `host.js`
  (`createTabHost`), `worker.js` (`serveTab`: queue → hand-off →
  dispatcher), `proxy.js` (`KupferdrahtProxy` and the handle proxies),
  `index.js` (re-exports), `index.d.ts`, `README.md`.
- Handles: the leader keeps `handleId → object` for `PendingLogin`,
  `Client`, `SyncHandle`; a response carrying a wrapper returns
  `{ $handle, $class, …getters }`; handles die with their leader — a
  `Client` handle re-resumes by `userId` on the new leader for
  idempotent calls, a `PendingLogin` handle cannot and fails
  `LeaderChanged`.
- Leader announcement: `{ kind: "leader", tabId, epoch }` on acquire
  and every 2 s; `epoch` increases per acquisition; three missed
  beats → `leader-unresponsive` (hint). A different `tabId`/`epoch`
  than the one a request was sent to is the failover trigger.
- The one Rust change: `wasm::Client::start_sync(on_message, on_typing?,
  on_expired?, on_sync?)` with `on_sync({ roomsUpdated,
  eventsDispatched })` per pass (`live_sync.rs`); `docs/bindings.md`
  and the api-mapping row follow.
- Tests: `kupferdraht-core/tests/step_94_leader_tab.rs` (browser, main
  thread as the thin client; the module loaded as wasm-bindgen local
  snippets; candidate workers with a stub `Kupferdraht` whose `open`
  performs the same `steal` the Rust open does — RPC round trip with
  `tabId` filtering and dedup, event fan-out, election on
  `worker.terminate()`, read re-issue, in-flight mutation failing
  `LeaderChanged`, the self-steal hand-off against the real Web Locks
  API); `kupferdraht-core/tests/step_94_tab_proxy_parity.rs` (native:
  wasm-bindgen export enumeration vs proxy + `.d.ts`; preservation:
  `Kupferdraht.open`'s shape and the smoke worker's direct open unchanged;
  nothing in `kupferdraht-core/src/` references the module).
- Consumer proof: `bindings/web/opfs-smoke/` gains the tabs-hosted
  page — two `createTabHost()` instances on one page play two tabs
  (Web Locks and `BroadcastChannel` are shared across same-origin
  contexts), tab B reads through tab A's leader, terminating A
  promotes B and the read re-issues — `TABS-OK` in headless Chrome and
  Firefox; the login vertical through the proxy against a Wire
  instance is the manual/live part.
- Docs: `docs/bindings.md` (the multi-tab section replaces "No
  read-only second tab in V1"), ADR-0023 Revision History line
  pointing here, `README.md` Platforms bullet, `docs/ARCHITECTURE.md`
  browser data flow, `rules/0026`, CHANGELOG `[Unreleased]`.

## References

- Extends (builds on): ADR-0023 "Enforce one active Kupferdraht instance per
  browser origin with a Web Locks lease and explicit takeover" — the
  lease is the election and the hand-off target; its negative
  consequence "no read-only second tab" is what this ADR resolves.
  Nothing in ADR-0023 is superseded.
- Builds on: ADR-0022 "Persist browser data in SQLite3MC over OPFS"
  (dedicated worker, one OPFS pool — why only one instance can exist).
- Constrained by: ADR-0015 Part 3 (hand-written wasm-bindgen surface
  is the contract; the proxy mirrors it, Rust gains only `onSync`),
  ADR-0011 / Step 79 (no duplicate sends — the failover rule),
  ADR-0010 / ADR-0016 (no new dependency).
- Related: ADR-0024 (native per-account lock — the native counterpart,
  no leader needed), ADR-0019 (push transport is native-only; the
  leader's loop is REST polling, `open.md` NG-14).
- External: W3C Web Locks API (request algorithm: `steal` releases held
  locks with `AbortError` and prepends the request; `ifAvailable` is not
  queued; agent termination releases held locks and aborts queued
  requests; FIFO grant); MDN `BroadcastChannel` (available in workers,
  same-origin, sender excluded; Chrome 54 / Firefox 38 / Safari 15.4);
  MDN `LockManager.request` (release on callback settle or tab/worker
  close); wasm-bindgen 0.2.128 `cli-support` (local snippets written
  under `snippets/<crate>-<hash>/<path>`); observations §81.

## Revision History

- 2026-09-25: Created (Step 94 orientation, Proposed)
- 2026-09-25: Accepted (Volker) — the three open spots decided as
  recommended: the inbound-message event is named `message` (not
  `timeline` — the payload is the `onMessage` item, not a diff), the
  module ships as plain ES modules + JSDoc + `index.d.ts` (no transpile
  step), and `startSync` gains the optional `onSync` callback (the one
  Rust change)
- 2026-09-25: Step 94 merged (`f83ad8b`, with the ADR commit `4c1dfc1` and
  the merge `93050fa` that mirrored Step 93's `homeId` / `cookieLabel` on
  the proxy) and archived; the plan path in Implementation Notes
  repointed — no content change
- 2026-09-26: Step 95d added a fifth optional `startSync` callback, `onDevicesChanged` (a room's MLS device set changed); the leader broadcasts it as `devices` and the proxy delivers it like `sync`. Same mechanism, current truth — not a new decision.
