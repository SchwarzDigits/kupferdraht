# Step 94 — Leader tab + thin-client tabs (many tabs, one instance)

**Status: MERGED 2026-09-25** — `4c1dfc1` (ADR-0026 + rule), `f83ad8b`
(the step), `93050fa` (merge of `main` after Step 93 landed in parallel:
four additive conflicts resolved, Step 93's new wasm getters `homeId` /
`cookieLabel` mirrored on the proxy in the same commit — the parity
rule's first case); fast-forwarded to `main` by Volker, CI green;
archived with this header. Scope shipped: `bindings/web/kupferdraht-tabs/`
(host, candidate worker with the self-steal hand-off, leader dispatcher,
proxies, `index.d.ts`), the optional `onSync` on the wasm `startSync`,
the parity + preservation pins, seven browser scenarios, the tabs smoke
page; gap-B.29 narrowed, gap-B.35 opened, idempotency keys proposed.
Commissioned 2026-09-25 (Volker); **built 2026-09-25** on
`step/94-leader-tab-thin-clients` (ADR-0026 accepted the same day — L1 the
module ships as plain ES modules + JSDoc + `index.d.ts`, L2 request
channel + per-tab response channel, L3 as planned, L4 the self-steal
hand-off `Kupferdraht.open({ takeOver: true })` inside the grant callback, L5
`message` / `typing` / `expired` / `sync` — the optional `onSync` on
`startSync` is the one Rust change). **Smoked 2026-09-25:** `TABS-OK` in
headless Chrome 153 and Firefox (`tabs.html?scenario=tabs`), Step 92's
`TWOTAB-OK` unchanged (preservation), and `LIVE-TABS-OK` — two real
Firefox tabs against the own Wire instance through a same-origin dev
proxy: tab B logged in, registered, synced, sent and subscribed through
tab A's leader (`onSync` arrived as `sync`), closing A promoted B, which
resumed the account from the OPFS store A had written, synced and read
the message back (observations §87). Driver: ADR-0023 gives
the browser exactly one active Kupferdraht instance per origin and a
takeover dialog for every further tab. A web app whose users open
several tabs wants all of them to *work*, not to fight. The
`SharedWorker` route is closed (ADR-0023 Option C: no
`FileSystemSyncAccessHandle` outside dedicated workers, no
`SharedWorker` on Chrome for Android). What remains — and what this
step builds — is the **leader-tab pattern**: the tab whose dedicated
worker holds the `kupferdraht:instance` lease *is* Kupferdraht (the leader);
every other tab of the origin is a thin client that talks to the
leader over an origin-scoped channel and renders what the leader
broadcasts. When the leader tab dies, the browser releases the lease,
the next waiting tab's worker acquires it and becomes the leader —
the Web Locks queue is the election. ADR-0023 stays true ("one active
instance"); it gains "served to many tabs".

**Sequencing:** after Step 92 (landed). Independent of Step 93 (keystore
ids) — may run in parallel; no shared files beyond `docs/bindings.md`.

## Decision first (phase 1/7, with Volker) — ADR-0026 via `new-adr`

The ADR extends ADR-0023 (it does not supersede it) and decides:

- **L1 — who ships the thin-client layer.** (a) Kupferdraht ships a
  reference TypeScript module in `bindings/web/` (`kupferdraht-tabs`:
  leader election, RPC proxy, event fan-out), tested by the browser
  test script and the consumer smoke page; embedders import it.
  (b) Kupferdraht documents the protocol only; every embedder builds it.
  Default: (a) — Step 79's lesson (kupferdraht-tui rebuilt resend because
  the core did not offer it) applies one level up: a protocol without
  a shipped client is a protocol every consumer re-implements
  differently. The wasm-bindgen surface (ADR-0015 Part 3) stays the
  contract the module is built on; nothing moves into Rust that JS
  can do.
- **L2 — transport.** `BroadcastChannel` (origin-scoped, no port
  handoff, structured-clone payloads incl. `Blob`; Chrome 54 /
  Firefox 38 / Safari 15.4 — under the OPFS floor) with
  request/response envelopes `{ tabId, requestId, method, args }` /
  `{ requestId, ok, value | error }`, versus `MessagePort` pairs
  brokered through the lock holder. Default: `BroadcastChannel`;
  every tab filters by `tabId`, the leader answers only requests it
  has not answered (dedup by `requestId`).
- **L3 — failover semantics for in-flight calls.** Reads are
  re-issued transparently after `leader-changed`; **mutating calls
  in flight fail** with a typed `LeaderChanged` error and are *not*
  auto-retried (a send that did reach the backend must not be sent
  twice — the leader's own resend API of Step 79 covers the honest
  case: the row shows `Failed` or it does not). Default: exactly that,
  documented; a later step may add idempotency keys.
- **L4 — election.** Every tab's worker keeps one pending
  `navigator.locks.request("kupferdraht:instance", …)` (no `ifAvailable`);
  the grant makes it the leader (`Kupferdraht.open` runs there);
  `takeOver` of ADR-0023 stays the manual override for a hung leader.
  Liveness: the leader announces itself on acquire and on a heartbeat;
  thin tabs that miss N heartbeats treat the leader as gone *only* as
  a UI hint — the lock, not the heartbeat, is the source of truth.
- **L5 — what the leader broadcasts.** Sync summaries and timeline
  diffs (the payloads `start_sync`'s callback already produces), room
  list changes, eviction/expiry events. Thin tabs render from the
  broadcast and *pull* (`rooms`, `messages`) on demand through the
  proxy; no local store in a thin tab (the OPFS pool is the leader's).

## Scope

1. **`bindings/web/kupferdraht-tabs/`** (TypeScript, no build magic beyond
   what `opfs-smoke` already uses): `createTabHost()` — runs in every
   tab: election + leader bootstrap (`Kupferdraht.open` in the tab's
   dedicated worker on grant, `onEvicted` wiring) + RPC dispatcher;
   `KupferdrahtProxy` — the thin-client face with the **same method
   names as the wasm `Kupferdraht`/`Client` classes** (login, resume,
   accounts, rooms, messages, sendText, sendReply, toggleReaction,
   editMessage, redact, markAsRead, createRoom, createDm, members,
   invite, kick, leave, logout, sendFile, downloadAsset, startTyping,
   stopTyping, syncOnce, startSync as a subscription); events via
   `addEventListener("sync" | "timeline" | "leader-changed" |
   "evicted")`.
2. **Rust side**: only what the fan-out needs and JS cannot do —
   e.g. a stable `tabId`-free event payload from `start_sync`
   (already JSON-projected), a `Client.accountId` getter if missing,
   and any `LeaderChanged`-shaped error mapping. No new storage, no
   lease-semantics change (ADR-0023 mechanics untouched).
3. **Tests (`scripts/wasm-test.sh`, headless browser)**: one page,
   two contexts — a dedicated worker as leader and the main thread as
   thin client: RPC round-trip (rooms/messages against a fixture
   store), event fan-out (a sync summary reaches the thin client),
   election (terminate the leader worker → the second worker acquires
   the lease, `leader-changed` fires, a read re-issues, an in-flight
   send fails `LeaderChanged`). `BroadcastChannel` is available to
   workers and windows alike, so the harness needs no second tab.
4. **Consumer smoke**: `bindings/web/opfs-smoke/` grows a second real
   tab — Step 92's `TWOTAB` becomes "two tabs, both usable": tab B
   reads and sends through tab A's leader, closing A promotes B
   (`THREETAB-OK`-style marker for the scripted run).
5. **Docs**: `docs/bindings.md` "WASM / web surface" (the multi-tab
   section replaces "No read-only second tab in V1"), ADR-0023 gets a
   Revision-History line pointing at ADR-0026, `README` "Platforms"
   bullet, `docs/ARCHITECTURE.md` browser data-flow.

## Out of scope

- `SharedWorker` hosting (ADR-0023 Option C — unavailable on Android,
  no sync access handles).
- Cross-origin or cross-profile sharing; offline queueing of thin-tab
  mutations while no leader exists (they fail closed with a typed
  error; the embedder shows "connecting").
- Native targets: unaffected — Step 85's per-account lock is the
  native counterpart and needs no leader.

## TDD sketch (red first)

- Proxy parity: every public method of the wasm `Kupferdraht`/`Client`
  classes has a proxy counterpart (a test enumerates the wasm-bindgen
  exports and fails on a missing name — the JS-side mirror of the FFI
  parity rule).
- RPC: request/response round-trip with `tabId` filtering; a foreign
  tab's response is ignored; a duplicate request is answered once.
- Election + failover as in Scope 3; the in-flight mutation fails
  typed, the read re-issues.
- Preservation: the single-tab path is byte-for-byte the Step-92
  path (`Kupferdraht.open` directly, no proxy) — the module is opt-in.
