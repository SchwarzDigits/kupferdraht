# ADR-0019: Auto-negotiate the push transport across /v14/events and the legacy /await socket

**Status:** Accepted (decision items 2–3 — drain-then-stream, cursor per frame — superseded by ADR-0030; the reconnect backoff by ADR-0039)
**Date:** 2026-08-21
**Deciders:** Volker Zöpfel

## Context

ADR-0017 reinstated push receive as `SyncTransport::WebSocket` over
`GET /v14/events` (ConsumableNotifications), opt-in, polling default.
Since wire-server chart 5.30, `consumableNotifications` is hard-coded
`false` in the upstream chart templates (PR #5116 hard-codes the
flag; no public timeline for re-enabling it) — on every current deployment that socket
connects, marks and acks but receives no events (observations §66/68).
gundeck then routes every push through the legacy path: the cannon
`/await` WebSocket plus the Cassandra-backed `/v14/notifications`
store.

The 2026-08-18 spike against the own instance (observations §68)
verified the legacy socket end-to-end: `wss://<nginz-ssl>/await`
delivers JSON in binary frames 0.8 s after send, the frame envelope
is exactly a `/v14/notifications` item (`{id, payload:[event],
transient}`), the `id` lives in the REST cursor space, transient
typing arrives live, Bearer-header auth works, `client=` is
optional. The socket is passive: no acks, no sync markers.

`Client::server_supports_push_events()` (Step 62c) already reports
which world a deployment lives in.

Constraints from prior decisions: ADR-0017 (one API family for push,
polling default, ack-after-persist and id-dedup rules), ADR-0008
(v14 only — the version-less `/await` route is a recorded
deviation), ADR-0009 (layering).

## Options

### Option A — third explicit variant (`SyncTransport::LegacyWebSocket`)

- Pro: no hidden behavior; each socket individually addressable.
- Con: every consumer must replicate the feature-flag check that
  Step 62c already encapsulates; misconfiguration yields a silent
  dead socket (the Step-62 experience, promoted to API design).

### Option B — `WebSocket` means "push", concrete socket auto-negotiated (chosen)

`SyncTransport::WebSocket` checks `server_supports_push_events()`
once per connection: `true` → `/v14/events` (ADR-0017 semantics
unchanged), `false` → legacy `/await`. Polling stays the default.

- Pro: consumers stay configuration-free (kupferdraht-tui asks for
  push, gets whatever the server can do); if a deployment enables
  ConsumableNotifications, clients flip back without a release.
- Con: which socket served a session is diagnosable only via logs
  (mitigated: the CLI prints the negotiated transport).

### Option C — push by default everywhere

- Pro: lowest latency out of the box.
- Con: reverses ADR-0017's polling default; wasm has no socket;
  surprises consumers that sized their runtime for polling.

## Decision

Option B. `SyncTransport` keeps exactly two variants; `WebSocket`
becomes "push, auto-negotiated". The legacy transport's mechanics:

1. **Connect:** `wss://<ws-base>/await?client=<device_id>`,
   `Authorization: Bearer` header (no token in the URL). The
   version-less path is Wire's deliberate legacy route — recorded
   as an accepted ADR-0008 deviation in `api-notes.md`.
2. **Drain-then-stream:** on every (re)connect, run one REST
   `since=<cursor>` drain first (closes any gap — the same
   Cassandra store feeds both), then dispatch socket frames.
3. **Frames:** parse as the existing `QueuedNotification` DTO —
   one frame = one notification; dispatch through the shared
   pipeline; **advance the persisted cursor with the frame `id`**
   (it is in the REST cursor space); dedup by `id` against the
   cursor (ADR-0017 rule carried over). No acks exist; transient
   frames (`transient: true`) dispatch without a cursor advance.
4. **Keepalive/reconnect:** client-side pings against cannon's
   idle timeout (interval is implementation detail, verified in
   the step's live smoke); reconnect with capped exponential
   backoff (`poll_interval` base) re-runs the drain-then-stream
   cycle. Reuse Step 62's loop; wasm keeps returning an error
   (ADR-0017).

## Consequences

### Positive

- Real live push (incl. transient typing) on every current
  wire-server — the Step-62 investment carries over (loop,
  backoff, dedup, dispatch pipeline are shared).
- Self-healing forward path: ConsumableNotifications comes back →
  clients flip to `/v14/events` automatically, no release needed.

### Negative

- The legacy route is exactly that — a future wire-server may retire it
  once ConsumableNotifications is enabled again; the negotiation makes that a
  non-event, but two socket dialects must be maintained until then.
- No server acks on the legacy socket: exactly-once relies on the
  client-side cursor dedup plus the reconnect drain (the same
  guarantees the polling path already provides — not weaker, but
  not stronger).

### Neutral

- `server_supports_push_events()` gains a second caller; the CLI
  prints which socket a `sync-stream` session negotiated.

## References

- Builds on: [ADR-0017](../../rules/0017-reinstate-websocket-push-transport.md) (rule only — its ADR file was never committed; this Context carries the rationale)
  (push family; its ack/dedup/reconnect rules carry over) and
  Step 62c's `server_supports_push_events()`.
- Constrained by: [ADR-0008](0008-target-wire-api-v14.md) (v14 —
  with the recorded version-less `/await` deviation),
  [ADR-0009](0009-matrix-rust-sdk-as-api-model.md).
- Evidence: `docs/observations.md` §66/§68; wire-server PR #5116
  (WPB-23942).

## Revision History

- 2026-08-21: Created
- 2026-09-28: Decision items 2 and 3 superseded by ADR-0030 "Treat the legacy /await push socket as a doorbell for the REST drain" (connect first, then drain; only the REST drain advances the cursor). Items 1 and 4 and the auto-negotiation stand.

- 2026-09-29: Partially superseded by ADR-0033 — push is the default receive transport wherever a target has a push transport; the browser keeps polling.
- 2026-10-02: The capped exponential reconnect backoff superseded by
  ADR-0039 "Drive sync by triggers, with timers only as safety nets" (at
  most one connect per 10 s pause, a drain without a socket). The
  auto-negotiation and items 1 and 4 stand.
- 2026-10-08: Wording — the flag's status is stated from the public
  record (PR #5116 hard-codes it to `false`, so the legacy socket
  carries push), and a re-enabling is phrased as a deployment's
  choice. Decisions unchanged.
