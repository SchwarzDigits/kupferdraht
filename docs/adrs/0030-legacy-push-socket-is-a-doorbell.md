# ADR-0030: Treat the legacy /await push socket as a doorbell for the REST drain

**Status:** Accepted ("No periodic drain on the keepalive tick" superseded by ADR-0039)
**Date:** 2026-09-28
**Deciders:** Markus Troßbach

## Context

This ADR supersedes decision items 2 and 3 of ADR-0019 "Auto-negotiate
the push transport across /v14/events and the legacy /await socket"
(drain-then-stream; a socket frame advances the persisted cursor). Items
1 and 4 of ADR-0019 and the auto-negotiation stand. The rationale for
changing course is a message loss the production-readiness review of
2026-09-25 found (open.md §71 B6).

On the legacy `/await` socket, Kupferdraht drains `GET /notifications?since=`
first and connects afterwards; every durable frame is dispatched and
advances the persisted cursor. An event stored after the drain but
before the socket is registered is never pushed to it — and the next
durable frame moves the cursor past that event, so no later drain
(`since` is exclusive) returns it. The same happens to any push the
socket does not deliver while connected: the socket has no redelivery
and no ack, yet its frames move the durable cursor. The window reopens
on every reconnect. ADR-0019 assumed "drain first closes any gap"; it
closes the gap before the drain, not the one after it.

The legacy socket matters in practice: current wire-server ships
`/v14/events` disabled, so auto-negotiation lands on `/await`.
`/v14/events` is not affected — its queue redelivers until an event is
acked, and Kupferdraht acks only after persisting the cursor (ADR-0017). The
REST drain returns the complete, ordered notification stream of the
client.

## Options Considered

### Option A: Keep drain-then-stream

**Pros:**
- No change; lowest latency per event (the frame is dispatched as is).

**Cons:**
- Loses events stored in the drain→connect window and pushes the
  socket does not deliver — permanently, because a later frame moves
  the cursor past them.

### Option B: Connect first, buffer frames during the drain

Connect, drain while buffering socket frames, then dispatch the
buffered frames the drain did not already return; frames keep
advancing the cursor.

**Pros:**
- Closes the connect window; one REST drain per connect only.

**Cons:**
- Pushes the socket does not deliver while connected are still lost.
- Needs a set of drained ids per connection (a drained id can still
  arrive as a frame) or the cursor moves backwards; more ordering logic.

### Option C: The socket is a doorbell

A durable frame only triggers a REST drain from the cursor; only the
drain dispatches and advances the cursor. Transient frames dispatch
directly.

**Pros:**
- Closes the connect window and undelivered pushes alike: the cursor
  only moves through the complete, ordered REST stream.
- Simpler than B: no id set, no ordering between frames and drain.

**Cons:**
- One REST round trip per burst of events (latency and backend load).
- A push the socket does not deliver, with no event after it, waits for
  the next event or the next reconnect (latency, not loss).

## Decision

**Option C.** On the legacy `/await` socket:

1. **Connect, then drain.** On every (re)connect the socket is opened
   first and one REST `since=<cursor>` drain follows — everything stored
   before the socket was registered comes from that drain, everything
   after it rings the doorbell.
2. **A durable frame is a doorbell.** It triggers a REST drain from the
   cursor; frames that arrive while a drain runs coalesce into one
   follow-up drain. The frame's own payload is not dispatched.
3. **Only the REST drain advances the cursor.** Never a socket frame.
4. **Transient frames** (`transient: true`, e.g. typing) are dispatched
   directly, without a cursor advance — they are never stored, the drain
   cannot return them.

No periodic drain on the keepalive tick: with a live socket it would be
load without a reason — an undelivered push is picked up by the next
event's drain or the next reconnect's.

The socket keeps being read while a drain runs (pings, frames), and a
running drain is never cancelled mid-way.

## Consequences

### Positive

- No event is lost to the connect window or to an undelivered push; the
  cursor only moves through the REST stream, monotonically.
- Less logic than buffering: no id set, no frame/drain ordering.

### Negative

- One `GET /notifications` per burst of events — more backend requests
  than dispatching frames directly.
- Slightly higher latency per event (one REST round trip).
- An undelivered push with no event after it waits for the next event
  or reconnect.

### Neutral

- `/v14/events` (ADR-0017) and polling are unchanged.
- Frames are still parsed (to tell durable from transient).

## Implementation Notes

- `sync/push/legacy.rs`: connect → drain → loop over socket frames, the
  ping tick and a pending drain; the drain is not cancel-safe, so it
  runs to completion while the socket is read alongside.
- observations §68 and `docs/wire/api-notes.md` repeat "drain first
  closes any gap" and are corrected with this step (97b).

## References

- Supersedes ADR-0019 decision items 2 and 3 (partial).
- Builds on ADR-0017 — `/v14/events` keeps its ack-based queue.
- Related: ADR-0008 (v14 only; `/await` is its recorded deviation).
- `docs/plan/97-durable-send-and-receive.md` (D4); open.md §71 B6.

## Revision History

- 2026-09-28: Created (Accepted — Markus; no keepalive drain)
- 2026-10-02: "No periodic drain on the keepalive tick" superseded by
  ADR-0039 "Drive sync by triggers, with timers only as safety nets" (a
  socket without a frame for a minute is renewed and drained once; without
  a socket the loop drains every 10 s). Connect-then-drain, the doorbell
  and the drain-only cursor stand.
