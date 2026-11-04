# Leader tab + thin-client tabs: many tabs, one instance (extends ADR-0023)

Condensed from `docs/adrs/0026-leader-tab-serves-thin-client-tabs.md`.

Accepted 2026-09-25 (Step 94 implements it); the failover classes
amended 2026-11-04 (Step 130a: the `keyed` class). Extends
`rules/0023-single-active-kupferdraht-instance-per-origin.md` — every rule
there stays in force; this file adds the multi-tab layer on top.

- **Kupferdraht ships the thin-client layer** as `bindings/web/kupferdraht-tabs/`
  (plain ES modules + JSDoc + hand-written `index.d.ts`, no bundler, no
  runtime dependency). **Don't** document the protocol only; **don't**
  add a transpile step.
- **The lock queue is the election.** Every tab's host keeps one
  candidate dedicated worker with one queued
  `navigator.locks.request("kupferdraht:instance")` (no `ifAvailable`); the
  grant bootstraps the leader **inside the grant callback** via
  `Kupferdraht.open({ takeOver: true }, onEvicted)` — the self-steal hand-off.
  **Don't** release the JS lock before opening (a queued tab would slip
  in); **don't** add a queued mode to the Rust lease; **don't** let a
  heartbeat decide leadership (hint only).
- **Transport is `BroadcastChannel`:** requests + events + leader
  announcements on `kupferdraht:tabs`, responses on `kupferdraht:tabs/<tabId>`;
  the leader answers each `(tabId, requestId)` once. **Don't** broker
  `MessagePort`s; **don't** rely on ordering across the two channels.
- **Failover is honest — by class** (`protocol.js`'s `METHODS`; the
  table in `bindings/web/kupferdraht-tabs/README.md`, pinned against
  it): `idempotent` calls (reads, and mutations a repeat converges)
  re-issue to the new leader; `keyed` calls (`sendText`, `sendReply`,
  `sendKnock`, `sendLocation`, `sendFile`, `toggleReaction`) re-issue
  with their idempotency key — the proxy adds one when the caller gave
  none — and the leader's `send_keyed` answers a key already used from
  the store; **every other mutation in flight rejects typed
  `LeaderChanged` and is never auto-resent**; no leader known → bounded
  wait, then typed `NoLeader`. **Don't** retry a `mutation` for the
  embedder; **don't** re-issue a keyed call without its key; **don't**
  class a call `idempotent` unless a repeat after a call that landed is
  known to answer like the first (`invite`, `kick`, `leave`, `deleteRoom`,
  `revokeJoinLink`, `setHandle` stay `mutation`); **don't** queue
  mutations offline.
- **The proxy mirrors the wasm surface name for name** (every public
  method of `Kupferdraht` / `PendingLogin` / `Client` / `SyncHandle` except the
  leader-only `Kupferdraht.open`); the native parity test fails on a missing
  name. A wasm-surface change lands with its proxy + `.d.ts` mirror in
  the same commit.
- **The leader broadcasts** `message`, `typing`, `expired`, `sync`
  (per-pass summary — the one Rust addition: `startSync`'s optional
  `onSync`), `devices` (Step 95d: `startSync`'s optional
  `onDevicesChanged`), `storage` (Step 113: `onStorage`), `rooms` (Step
  138: `onRoomsChanged`, a room's read state after a local change),
  `leader-changed`, `evicted`. Thin tabs **pull** the rest; **no local
  store in a thin tab**.
- **The single-tab path stays byte-identical:** `Kupferdraht.open` directly,
  no proxy — the module is opt-in. **Don't** make `kupferdraht-core` depend
  on the module; **don't** host in a `SharedWorker` (ADR-0023 Option C).
