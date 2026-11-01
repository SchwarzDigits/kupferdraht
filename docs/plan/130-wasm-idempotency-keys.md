# Step 130 — Idempotency keys for wasm mutations (a retry across a leader change)

**Status: commissioned 2026-11-01** (transferred from open.md's former
"Proposed steps", proposed 2026-09-25 in the Step 94 retrospective; ADR-0026
option L3-C, deferred there to "a later step").

## Problem

ADR-0026 (Step 94) chose L3-A for the thin-client tabs: a read in flight
when the leader tab changes is re-issued to the new leader; a mutation in
flight (`sendText`, `sendReply`, `sendFile`, `toggleReaction`, `editMessage`,
`redact`, `markAsRead`, `createRoom`, `createDm`, `invite`, `kick`, `leave`,
…) rejects with a typed `LeaderChanged` and is never retried by
`kupferdraht-tabs`, because the module cannot know whether the old leader's
request reached the backend. Honest, and the right default without more
state — but every embedder has to write the retry, and a blind retry can
send a message twice.

Native consumers have Step 79's answer: a failed send is a `Failed` row,
`Room::failed_sends` lists them, `Timeline::resend` / `resend_failed`
re-drive them under the same message id. The wasm surface has none of it
(`wasm/manager.rs` exposes neither `failed_sends` nor `resend`), so the
browser embedder has neither a safe retry nor a visible failed row to offer
the user.

## Scope

Phase 1 picks the route, with the embedder of the browser surface in the
loop:

- **(a) L3-C, idempotency keys:** a client-chosen key per mutating call (an
  optional last argument or an options object on the wasm surface), stored
  with the call's outcome in the leader's account store (a new table, keyed
  per account; retention to decide — a sweep after a bounded age). The same
  key answers the stored handle without a second upload, a new key re-runs.
  `kupferdraht-tabs` then retries a keyed mutation on the new leader instead
  of rejecting: `protocol.js`'s classification gains a third class, the
  parity test of Step 94 pins it, `index.d.ts` carries the argument.
- **(b) The Step-79 resend path on the wasm surface:** `failedSends` and
  `resend` / `resendFailed` on the wasm `Room` / `Timeline`, their tab
  mirrors (`METHODS` rows, proxy methods, `index.d.ts`), the doc homes. The
  embedder shows the failed row and re-drives it; no automatic retry.
- **(c) both:** (b) first as the visible fallback, (a) for the silent case.

The recommendation going into phase 1 is (c): (b) is parity with native and
small, (a) is what makes a leader change invisible. Whether the key enters
the core API (native and FFI parity, so a Kotlin / Swift consumer retries
after a network error the same way) or stays a wasm-only argument is the
phase-1 question with the widest reach; ADR-0026 gets a revision entry
either way.

## Out of scope

- An offline queue (ADR-0026 L3-A: no call waits for a leader beyond the
  bounded first announcement).
- Login, device registration and logout: a `PendingLogin` cannot move
  between leaders (ADR-0026) and gets no key.

## TDD sketch

- core: the dedup table's migration; a mutation with a key stores its
  outcome; the same key returns the stored handle and the backend sees one
  upload (httpmock request count); different keys send twice; a key of
  another account is not visible — red.
- wasm surface: the parity pins of `step_94_tab_proxy_parity.rs` extended
  for the new argument or methods; the browser tests (`scripts/wasm-test.sh`,
  run in CI) cover a keyed send answered from the store — red.
- tabs: a keyed mutation in flight during a leader change resolves on the
  new leader with the stored handle, an unkeyed one still rejects
  `LeaderChanged` — a browser test in CI, or failing that a smoke item as in
  Step 125.

Smoke (two tabs on the test stage): a send in flight while the leader is
evicted (the Step-100 takeover) arrives exactly once.
