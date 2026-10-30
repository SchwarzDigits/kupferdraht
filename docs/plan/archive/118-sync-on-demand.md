# Step 118 — Sync only what changed: the room list on demand, no polling beside a live socket

**Status: MERGED 2026-10-02** — `26ef3d4`, `ff83c62`, `f79b5ab`,
`769f1a9`, `db31241`, `d0ffe70` (ADR-0039). Merged to `main` as PR #105
(`f8abebd`); CI green. Smoke 2026-10-01 against the test stage in two
rounds (browser; native push, polling and without a socket). Scope
shipped: the room list on demand with a 5-minute safety net; no timer
beside a live socket, a silent socket renewed after a minute, a drain
without a socket, one 10 s pause without a socket on every target; push
as the native default (ADR-0033). Retrospective: `docs/LEARNINGS.md`
§5.61. Archived with this header.

Commissioned 2026-10-01 (Markus — "why do we poll at all when
we have a socket, and why four requests each time?"; design and the
5-minute renewal approved 2026-10-01; the 5-minute room-list safety net
added at his request the same day; after PR #105's first review the
renewal shortened to 1 minute, the native push loop made to behave like
the browser's, and push made the native default — D3–D5, Markus,
2026-10-01; on 2026-10-02 one pause of 10 s for every loop without a
socket, native polling included). Recorded as ADR-0039 (accepted by
Markus 2026-10-02). Branch `step/118-sync-on-demand`, off `main`.

## Problem (verified in code 2026-10-01)

- **Every pass fetches the whole room list.** `Client::sync_once` runs
  `sync_metadata` (`POST /conversations/list-ids`, `POST
  /conversations/list`, `POST /list-users`) and then the drain (`GET
  /notifications`) — four requests, also when nothing changed. The room
  list only changes through events the drain sees (a room created,
  renamed, joined, left; a member's profile), and the drain hands those
  events over today without using them (`skipping non-MLS event`).
- **The browser polls beside a live socket.** `wasm/live_sync.rs` waits
  for the doorbell *or* 2.5 s, whichever comes first, so an idle tab with
  a live socket still runs a full pass every 2.5 s: about 5,800 requests
  an hour per account.
- **A closed browser socket is noticed late.** Its `onclose` only logs;
  the loop finds the closed socket at its next timed pass.
- **Native polling** (the default transport) pays the same four requests
  every 5 s. **Native push** refreshes the room list once per connection
  and never in between: a room created during a long connection stays
  unseen until the next reconnect.

## Design

- **D1 — the room list is refreshed when it is stale, not every pass.**
  The client keeps one flag, `rooms_stale`, set at construction. The
  drain sets it when it hands over an event that may change the room
  list: an event it does not process itself (today's skip arm — room
  created, renamed, member joined or left, a profile changed, …) and a
  Welcome (a new room). It also sets it when this client cannot have seen
  every event: the drain does not start from the cursor this client's
  last drain ended at (another client or process drained the queue,
  ADR-0031 — an app and its notification extension), or the backend lost
  the cursor (the marker-less refetch skips the gap). A refresh clears
  it; a failed one leaves it set. **Safety net** (Markus, 2026-10-01): a
  list older than 5 minutes is refreshed by the next pass too, so a change
  no trigger caught is stale for at most that long.
  `sync_once` refreshes the stale list before the drain (rooms before the
  drain, as today; the device baselines are still checked every pass) and
  the dispatch cycle refreshes it after a drain or push frame that marked
  it, so a new room shows up in the same pass. An idle pass is one
  request.
- **D2 — native push** refreshes the stale list per connection instead of
  unconditionally, and mid-connection through D1: a reconnect without room
  events fetches no room list, and a room created during a long
  connection is seen at once.
- **D3 — the browser waits for the bell while the socket lives.**
  - A closing socket rings the bell: the loop wakes at once, reopens the
    socket and drains (connect, then drain — ADR-0030).
  - With a live socket the loop waits up to `PUSH_RENEWAL` (1 min; 5 min
    in the first review round). A browser cannot ping, so a socket that
    died silently (a sleeping laptop, a dropped NAT mapping) or one a
    proxy holds frames on is invisible; when the bell stayed silent that
    long, the loop renews the socket and drains once. Cost while idle:
    one connect and one drain per minute.
  - Without a socket — push unavailable — it polls every 10 s
    (`NO_SOCKET_POLL`; Markus, 2026-10-01: 10 s rather than native's 5 s
    for a fallback; 2026-10-02: the same 10 s for every loop without a
    socket on every target, native polling included), retrying the socket
    each pass.
  - At most one connect per 10 s: a server that accepts and closes at
    once cannot spin the loop.
  - `IDLE_BACKOFF` (2.5 s) goes away.
- **D4 — the native push loop behaves like the browser's** (both
  dialects share the outer loop):
  - a failed connect drains anyway — before, a socket that could not be
    opened (a network that blocks WebSockets) stopped receive entirely;
  - at most one connect per pause — `poll_interval`, 10 s when unset
    (`NO_SOCKET_POLL`, shared with the browser and the polling loop) —
    replacing Step 103's
    exponential backoff (5 s doubling to 30 s) and its 1 s gap after a
    productive connection: a long-lived connection reconnects at once, a
    flapping or unreachable one every 10 s, with a drain each time;
  - the legacy socket is renewed after `PUSH_RENEWAL` (1 min) without a
    frame — checked on the 25 s keepalive tick. The pings detect a dead
    TCP connection only when a write fails; a proxy that holds frames
    while it answers pings is caught only by the renewal. The
    `/v14/events` dialect (disabled on every current server) acks and
    redelivers; it gets no renewal.
- **D5 — push is the native default** (ADR-0033, accepted 2026-09-29,
  implemented here): `SyncTransport::default()` is `WebSocket` where the
  push module compiles (`not(target_arch = "wasm32")`), `Polling` in the
  browser build (whose live sync has its own socket and ignores the
  field). `kupferdraht-cli sync-stream --transport` defaults to `websocket`.
  Kotlin and Swift name the transport anyway (UniFFI records carry no
  enum defaults); their docs and examples switch to `WebSocket`.
  `Polling` is the explicit choice, and it pauses the same 10 s as a push
  loop without a socket: one value without a socket on every target
  (`poll_interval` unset; Step 103's 5 s is gone). BREAKING for Rust
  consumers relying on the default.
- **Unchanged:** the public API's shape and `SyncSummary`
  (`rooms_updated` stays the size of the room list); `sync_once` stays
  transport-agnostic; `sync_metadata` stays public and refreshes
  unconditionally.

## Tests (red first)

- **Native, `tests/step_118_sync_on_demand.rs`** (httpmock; the Step-97b
  loopback socket for push):
  - two idle `sync_once` passes fetch the room list once;
  - a drained room event (`conversation.rename`) refreshes the room list
    in the same pass, and the new name is in `rooms()`;
  - a Welcome refreshes the room list (a garbage one too — the refresh
    does not depend on the absorb);
  - a lost cursor (`404` on `since`) refreshes the room list;
  - a drain by another client on the account refreshes this client's
    room list at its next pass;
  - a failed refresh is retried by the next pass;
  - push: a reconnect without room events fetches the room list once in
    total; a room event drained on a live connection refreshes it without
    a reconnect.
- **Unit, `client/room_freshness.rs`:** a new client is due; a fresh list
  only when marked, and taking clears the mark; a list older than 5 minutes
  is due; a drain from where the last one ended keeps it fresh, one from
  elsewhere marks it.
- **Browser, `tests/step_118_browser_sync_pace.rs`** (wasm, pure seams in
  `wasm::sync_pace`): with a live socket the wait is `PUSH_RENEWAL`
  (1 min), without one `NO_SOCKET_POLL` (10 s); a connect is due at once
  after a long-lived socket and not again within 10 s.
- **Native push, `tests/step_118_native_push.rs`** (the Step-97b loopback
  harness): push is the native default; without a socket (nothing
  listening) a message still arrives through the drain; without a socket
  the loop retries at the push pace, not with a doubling backoff; a
  connection the server closes at once is not retried before the pace.
- **Unit, `sync/pace.rs` and `sync/settings.rs`:** `PUSH_RENEWAL` is
  1 minute, `NO_SOCKET_POLL` 10 s; a socket is silent after
  `PUSH_RENEWAL` without a frame; the push pace is `poll_interval`, 10 s
  when unset.
- **Smoke** (test stage, release wasm): an idle tab for 10 minutes with
  `log=debug` — no room-list request after the first pass, one drain and
  one connect per renewal; a message from another device arrives within
  about a second; a room renamed from another device shows up at once.
- **Smoke, native** (test stage, release CLI): `sync-stream` with the
  default transport (push) and with `--transport polling`, each with a
  room created, renamed and a message from another device; push idle for
  more than a minute shows one renewal; with the WebSocket host
  unreachable, push still receives through the drain.

## As built (2026-10-01)

- `client/room_freshness.rs`: `RoomFreshness` (the stale flag, set at
  construction, and where this client's last drain left the cursor) and
  `Client::refresh_rooms_if_stale` — clears the flag before the fetch, so
  an event marking it during the fetch is not lost, and sets it again when
  the fetch fails. `sync_metadata` itself is unchanged and stays public.
- The safety net is the list's age in `RoomFreshness` (`MAX_AGE_MS`,
  5 min, set after a successful refresh): `take_due(now)` is the mark or
  the age. In the idle browser it coincides with the socket renewal; native
  polling refreshes every 5 minutes; native push at the next frame or
  reconnect after 5 minutes.
- Before its first drain a client has no cursor of its own to compare;
  the first version compared against "none" and refreshed the list a
  second time right after start. Found by the smoke, fixed with a unit
  test (`the_first_drain_marks_nothing`).
- The device baselines (Step 95d) first moved with the refresh — "they
  only change when the room list does". Wrong: the per-pass check is also
  the safety net for a room without a baseline (joined by a pre-95d build,
  a missed call site), and the Step-95d test
  `a_room_without_a_baseline_gets_one_before_the_next_drain` failed in the
  gate. They stay per pass in `sync_once` — a local read per room, no
  request.
- Triggers: the skip arm of the event dispatch and the Welcome arm (before
  the absorb, so a Welcome that fails to absorb still refreshes); in the
  drain, a start cursor this client did not leave, a cursor another drain
  moved mid-drain (ADR-0031's `CursorMoved`), and the lost-cursor refetch.
- The post-drain refresh lives in `end_dispatch_cycle`, so the REST drain,
  the push frames and the transient path share it. A failure there is
  reported through `on_sync_error` and stays marked; the next `sync_once`
  retries it with its error surfacing.
- Native push: `run_connection` refreshes only a stale list.
- Browser: the loop moved out of `start` into `run_sync_loop` (`start` was
  over the function-length limit before); `drain_once` and
  `reconnect_if_due` carry the pass and the connect. The pacing is in
  `wasm/sync_pace.rs` (`PUSH_RENEWAL`, `idle_wait`, `connect_due`), public
  only for the browser suite, like `push_socket`. A dead socket is dropped
  before its successor connects, so its close cannot ring for the new one.
- Found on the way: `push_socket::connect` attached its close and message
  callbacks before the upgrade and dropped them when the upgrade failed — the
  browser's close event then reached a freed closure. The socket is now owned
  before the upgrade, so a failed one is dropped with its callbacks detached.
- D3–D5 (second round): `sync/pace.rs` holds what both push loops share —
  `PUSH_RENEWAL` (1 min), `NO_SOCKET_POLL` (10 s), `socket_silent`;
  `wasm::sync_pace` aliases the constants (a `pub(crate)` item cannot be
  re-exported publicly). Natively `run_push` runs `run_cycle` — a
  connection, or `drain_without_socket` when the connect failed — and
  then sleeps the rest of `SyncSettings::idle_pause` — the polling loop's
  pause, one value (10 s unset) for both. `Ended { NoSocket,
  Closed }` replaces the frame counts, which only fed Step 103's backoff;
  `reconnect_delay`, `reconnect_gap` and their unit tests are gone. The
  legacy renewal is checked on the 25 s keepalive tick and only while no
  drain runs (a running drain is never cancelled, ADR-0030), so a silent
  socket is renewed after 60–85 s. The default is two `cfg`-gated
  `Default` impls. CLI: `sync-stream --transport` defaults to
  `websocket`, `--idle-backoff` is optional (unset: the library's
  defaults).
- Tests changed with D5: the Step-14b loop tests name `Polling`; Step
  62b's `default_sync_transport_is_polling` pin moved to
  `step_118_native_push.rs` as the opposite.
- No CLI command: `sync` and `sync-stream` exercise the change.
- The smoke page can now exercise push: `/smoke-login.json` may carry
  `ws`, which every login of the page passes as the websocket base (before,
  `''` — the browser never tried a socket and the page smoked the polling
  fallback).

## Smoke (2026-10-01, test stage, release wasm, `?scenario=idle`)

A leader tab (account 2, storage server) resumed with the push socket
relayed by the dev proxy; account 2's CLI device acted from another
device:

| time | what | browser |
|---|---|---|
| 19:04:41 | start | socket open, one room-list fetch, one drain |
| 19:05:11 | CLI creates a room | ring, drain, room-list fetch — the room is listed the same second |
| 19:05:37 | CLI renames it | ring, drain, room-list fetch — new name the same second |
| 19:05:54 | CLI joins the smoke room (external commit) | ring, drain; the tab's group was a commit behind (stale state from earlier runs, `open.md` §73) and healed by rejoining |
| 19:06:57 | CLI sends a message | ring, drain — delivered the same second, no room-list fetch |
| 19:06:57–19:11:58 | idle | no request at all |
| 19:11:58 | 5 minutes without a ring | socket renewed, one room-list fetch (the 5-minute age), one drain |

Requests over the 12 minutes: 5 room-list fetches, 8 drains (before:
a full pass of four requests every 2.5 s — about 1,150 in 12 minutes).

## Smoke, second round (2026-10-01, test stage — D3 at 1 minute, D4, D5)

Release CLI and release wasm. Account 2's CLI device (device 2) and a
third device of the account, logged in with a websocket base nothing
listens on (device 3), acting for each other; the browser tab as in the
first round.

- **Native, no socket** (device 3, `sync-stream` with the defaults): the
  connect failed every 10 s and each time the loop drained
  (20:47:40, :50, 48:00 UTC); a message device 2 sent arrived with the
  next drain, 8 s later. One room-list fetch at start.
- **Native push** (device 2, `sync-stream` with the defaults): device 3
  created a room, renamed it and sent a message — each rang, drained and
  arrived the same second; the room events fetched the room list, the
  message did not. Silent afterwards: renewed after 78 s and then every
  ~75 s (the 25 s ping tick), one drain each; at 5 minutes the renewal
  fetched the room list too.
- **Native polling** (device 2, `--transport polling`; still Step 103's
  5 s then — 10 s since the unification of 2026-10-02): a drain every
  5 s, the room list once; device 3's message arrived 4 s after it was
  sent.
- **Browser** (1-minute renewal): the three messages sent while it was
  closed arrived at start; device 3's new room, rename and message each
  the same second; renewed at 22:57:03, 22:58:03, 22:59:04, 23:00:05 and
  23:01:05 local, the last one with the 5-minute room-list fetch.

Device 3 was deleted after the run.

