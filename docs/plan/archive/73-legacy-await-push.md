# Step 73 — legacy `/await` push transport (auto-negotiated)

> **Status:** merged to `main` as `e61bc56` (2026-08-23; red + green
> + live smoke 2026-08-21, external review round applied 2026-08-23,
> CI green). Scope: ADR-0019 negotiation in `run_push`, the legacy
> `/await` cycle (`sync/push/legacy.rs`), Bearer-header connect,
> transient-aware cursor, keepalive pings, 62b re-pinned to the
> enabled dialect, `await_url` unit pins, doc-freshness sweep.
> Implements ADR-0019: `SyncTransport::WebSocket` auto-negotiates
> the concrete socket via `server_supports_push_events()` — `true` →
> `/v14/events` (ADR-0017 unchanged), `false` (every current
> wire-server, observations §68) → the legacy cannon `/await` socket.
> Spike-verified 2026-08-18; delivers real live push (incl. transient
> typing) for kupferdraht-tui on current Wire.
>
> **Historical names:**
> - "Step 73" — proposed 2026-08-18 (spike), cut 2026-08-21.
>
> **Branch / commit prefix:** `step/73-*`, `feat(step-73): …`.

---

## Goal

`sync*` with `SyncTransport::WebSocket` works on servers with
ConsumableNotifications disabled: connect `wss://<ws-base>/await?client=<device>`
(Bearer header, never a URL token), drain the REST backlog first,
then dispatch socket frames (each one a `QueuedNotification`: JSON in
a binary frame) through the shared pipeline, advancing the persisted
cursor with the frame `id`; `transient: true` frames dispatch without
a cursor advance. Client pings hold cannon's idle timeout; reconnect
= backoff + drain-then-stream (ADR-0019 rules).

## TDD list (red)

In-process tokio-tungstenite server (the Step-62b harness, extended
to capture the `Authorization` header):

1. `negotiation_connects_await_when_consumable_disabled` — feature
   flag absent/disabled → the loop must connect `/await` with a
   non-empty `client=` param, NO `access_token` in the URI, and a
   `Bearer` header (today it dials `/v14/events` → red).
2. `legacy_frames_dispatch_and_advance_the_cursor` — a durable
   binary `QueuedNotification` frame dispatches (typing observable)
   and advances `sync_state.last_event_id` to the frame id; a
   `transient: true` frame dispatches but leaves the cursor.
3. `connect_drains_rest_backlog_before_streaming` — a queued REST
   notification is dispatched BEFORE the first socket frame
   (recorded handler order).
4. `negotiation_keeps_events_when_consumable_enabled` — day-0 green
   guard: with the feature enabled the loop still dials
   `/v14/events` (ADR-0017 path untouched).

Keepalive ping cadence is deliberately untested here (no wall-clock
tests) — the live smoke holds a socket through an idle window.

## Live smoke

PASSED 2026-08-21, all stages (`scripts/smoke-step73.sh`, local):
fresh room, negotiated-`/await` banner asserted, live receive,
transient typing via REST POST, **100 s idle window on a single
connection** (client keepalive holds cannon's idle timeout),
exactly-once across a stream restart. Deviation from the original
plan ("re-run `smoke-step62` stages 4–7"): the old shared smoke
rooms are wedged by a Remove-proposal backlog from the 2026-08-21
device cleanup (observations §69, gap-B.25) — the smoke uses a
fresh room instead, which exercises the same stages.

## Out of scope

- Browser/wasm socket (ADR-0017/0019 — follow-up step).
- Retiring the `/v14/events` dialect (stays for the auto-flip).
