# ADR-0039: Drive sync by triggers, with timers only as safety nets

**Status:** Accepted
**Date:** 2026-10-02
**Deciders:** Markus Troßbach

## Context

This ADR supersedes the sentence "No periodic drain on the keepalive
tick" of ADR-0030 "Treat the legacy /await push socket as a doorbell for
the REST drain", and the reconnect backoff of ADR-0017 and ADR-0019
(capped exponential, `poll_interval` base; Step 103 set the base to 5 s
and the cap to 30 s), and Step 103's 5 s polling pause. ADR-0030's other
decisions stand, as do ADR-0017's and ADR-0019's. The rationale for changing course is the request
load and a receive gap found in Step 118.

Every sync pass fetched the whole room list (`list-ids`, `list`,
`list-users`) before its drain — four requests for a pass that found
nothing. The browser's live sync ran such a pass every 2.5 s even with
its push socket live, about 5,800 requests an hour per idle tab, so the
socket only shortened the latency, it did not replace the timer. The
room list only changes through events the drain sees, and the drain
handed those events over without using them.

The push socket is not reliable on its own. A browser cannot ping, and a
native ping finds a dead connection only when its write fails: a socket
that died silently (a sleeping laptop, a dropped NAT mapping) or one a
proxy holds frames back on looks alive to both. And natively a socket
that could not be opened at all (a network that blocks WebSockets)
stopped receive entirely — the loop only retried the connect. ADR-0033
makes push the native default, so that gap would reach every native
consumer that does not choose.

## Options Considered

### Option A: Keep the timers (status quo)

The browser runs a full pass every 2.5 s beside its socket; every pass
fetches the room list; native push retries a failed connect with a
doubling backoff and does not drain without a socket.

**Pros:**
- No change; the timer hides every socket and trigger failure within
  seconds.

**Cons:**
- Four requests per pass while idle — about 5,800 an hour per browser
  tab; the socket saves nothing.
- Natively, no receive at all while the socket cannot open.

### Option B: Triggers only, no timers while a socket lives

Drain when the socket rings or closes; fetch the room list when an event
changes it; no timer beside a live socket.

**Pros:**
- Near-zero idle load.

**Cons:**
- A socket that is dead or holds frames back, but looks open, stops
  receive until the page reloads or the app restarts.
- A room-list trigger that misses an event type leaves the list stale
  until the next restart.

### Option C: Triggers, with timers only as safety nets

Triggers drive the work (B); a few long timers bound how long a missed
trigger can hide: the socket is renewed after a while without a frame,
the room list after a while without a refresh, and without a socket the
loop drains on a slow timer.

**Pros:**
- Idle load drops by more than an order of magnitude; latency stays
  sub-second while the socket works.
- Every failure is bounded: a dead or frame-holding socket by the
  renewal, a missed room-list trigger by the list's age, a socket that
  cannot open by the no-socket drain.

**Cons:**
- More moving parts than A; the bounds are latencies, not zero.

## Decision

**Option C**, for the browser's live sync and the native push loop
alike, and for the room list on every transport.

1. **The room list is fetched when it may be out of date:** at start;
   after an event Kupferdraht does not process itself, or a Welcome; when
   this client cannot have seen every event (another client or process
   drained the queue, or the backend lost the cursor) — then in the same
   pass, after the drain; and when it is older than **5 minutes** (the
   safety net). A reconnect alone fetches nothing: the connect drain
   (ADR-0030) brings the gap, and its room events trigger the fetch.
2. **No timer beside a live socket.** The loop drains when the socket
   rings or closes (a close reconnects at once, then drains).
3. **A socket without a frame for 1 minute is renewed** — reconnect,
   then drain (ADR-0030's order). The browser checks it on its wait,
   natively on the keepalive tick and only while no drain runs.
4. **Without a socket the loop drains anyway**, and connects at most
   once per pause, so a long-lived connection reconnects at once, a
   flapping or unreachable one every pause with a drain each time.
5. **One pause without a socket, on every target: 10 s** — the polling
   loop's (explicit `Polling`, FFI `start_sync`) and a push loop's while
   its socket is down; natively `poll_interval` overrides it.

The renewal started at 5 minutes and was shortened to 1 minute so that a
proxy holding frames back costs at most a minute of latency; the
room-list refresh on every reconnect was weighed and rejected (the
connect drain covers the gap, and in the browser every renewal is a
reconnect); for the pause without a socket 10 s was chosen over Step
103's 5 s — it is only a fallback once push is the default — and made
the one value for polling and push on every target.

## Consequences

### Positive

- An idle browser tab makes about 160 requests an hour instead of about
  5,800; an idle native pass is one request instead of four, and an idle
  native push loop makes about as many as the browser.
- Sub-second receive while the socket works; a new or renamed room shows
  up in the same pass as its event.
- Receive never depends on the socket opening — which makes push safe
  as the native default (ADR-0033).

### Negative

- A socket that is dead or holds frames back delays receive by up to a
  minute (before: 2.5 s in the browser).
- A room-list change no trigger caught is stale for up to 5 minutes.
- Kupferdraht now relies on the room events of backend-spec §12.3
  (`conversation.create`, `rename`, `member-join`/`leave`,
  `user.update`), which the contract lists as SHOULD; a backend that
  omits them shows such changes only after the 5 minutes.
- One reconnect and one drain per idle minute and socket.

### Neutral

- Messages take the same path as before — the drain, the cursor, the
  dedup (ADR-0030–0032); only *when* it runs changed.
- `sync_once` stays transport-agnostic.

## Implementation Notes

- Core: `client/room_freshness.rs` (the stale mark, the drain cursor
  comparison, the 5-minute age); the refresh runs before the drain in
  `sync_once` and in every dispatch cycle. The rooms' device baselines
  stay checked every pass (Step 95d's safety net).
- Shared pace: `sync/pace.rs` (`PUSH_RENEWAL`, `NO_SOCKET_POLL`,
  `socket_silent`); browser `wasm/sync_pace.rs`; native
  `SyncSettings::push_pace`, `run_push` → `run_cycle` /
  `drain_without_socket`, the legacy renewal on the ping tick.
- `/v14/events` (disabled on every current server) shares the pace and
  the no-socket drain, not the renewal — it acks and redelivers.

## References

- Supersedes, in part: ADR-0030 ("No periodic drain on the keepalive
  tick"); ADR-0017 and ADR-0019 (the capped exponential reconnect
  backoff).
- Builds on: ADR-0030 (connect, then drain; only the drain advances the
  cursor); ADR-0033 (push as the native default — implemented in the
  same step).
- Related: ADR-0031, ADR-0032 (the drain's guarantees); ADR-0023,
  ADR-0026 (one instance per account, the leader tab runs the loop).
- `docs/plan/118-sync-on-demand.md`; `docs/backend-spec.md` §12.3.

## Revision History

- 2026-10-01: Created (Proposed — Markus)
- 2026-10-02: Accepted (Markus) — one ADR for the room list and the push
  pace; implemented in Step 118.
