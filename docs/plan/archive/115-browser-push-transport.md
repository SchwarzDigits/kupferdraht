# Step 115 — Browser push: the doorbell rings the browser's sync loop

**Status: MERGED 2026-09-30** — proposed 2026-09-30 by Sven Braune
(natrium-web SDK) as "Step 112"; renumbered 115 in the archive, since
Step 112 is the recovery data (ADR-0035). Branches
`step/112-browser-push-transport` and `step/112b-browser-transient-typing`
(115b: typing through the socket). Merged to `main` as PR #93
(`5c1c803`), after main and the browser-storage chain were merged into it
(`wasm/live_sync.rs` keeps both the doorbell and Step 113's storage-state
watch); CI green. Its browser test, `step_115_browser_push_doorbell`, now
runs in CI (wasm clippy and `scripts/wasm-test.sh`). Retrospective:
`docs/LEARNINGS.md` §5.57. Archived with this header.

## Goal

Give the browser build a push transport, so `wasm/live_sync.rs` drains on a
doorbell instead of a fixed 2.5 s sleep. Sub-second receive in the browser,
with polling as the fallback it already is.

## Why this is possible, against ADR-0033

ADR-0033 recorded that a browser push transport "is not planned in this
repository today", and `wire/websocket.rs` gives the reason: "the browser
cannot set upgrade headers". Both are accurate. The obstacle is not fatal:

- `await_url` appends only `client`, and the token rides an `Authorization`
  header through `connect_with_bearer` — which is indeed impossible from
  `new WebSocket()`.
- But Wire accepts the token as a **query pair** on `/await`, exactly as
  `events_url` already builds for `/v14/events` in the same file.
- Proven in production, not in theory: the natrium web SDK has been running a
  TypeScript doorbell socket against a live wire-server for days
  (`sdk/src/internal/push.ts`), measured 0.9–1.4 s against this loop's 2.5 s
  floor. This step moves that into Kupferdraht so every wasm consumer inherits it
  and the SDK can delete its own.

ADR-0033's decision needs no reversal — it made `SyncTransport::default()`
target-dependent precisely so "a browser push transport, once it lands,
inherits the default without a new decision". This is that landing.

## Scope

Smaller than porting the native push loop, because the browser already has a
loop of its own:

1. **`await_url` gains the token as a query pair** (browser cannot use a
   header). Native keeps using the header; nothing changes for it.
2. **A browser socket** — `web_sys::WebSocket` behind an async read. No
   `tokio-tungstenite`: that dependency stays out of the wasm graph, as
   `wire/websocket.rs` requires.
3. **`wasm/live_sync.rs` waits on a doorbell** instead of sleeping a fixed
   `IDLE_BACKOFF`: whichever comes first, a frame or the timeout. A frame
   never dispatches its own payload — ADR-0030 stands, only the drain
   advances the cursor.
4. **No keepalive at all** — measured, not assumed. See below.
5. **Reconnect with backoff**, and the timed poll continues throughout — a
   dead socket degrades to today's behaviour, never below it.

### Measured 2026-09-30: the browser needs no keepalive

`legacy.rs` sends an RFC-6455 `Message::Ping` every 25 s because "cannon's idle
timeout closes a silent connection otherwise". A browser cannot do that — the
WebSocket API exposes no `ping()`, by design, since the user agent owns the
connection — so the obvious move was an application text frame instead. That
would have been wrong on both counts.

`observations.md` §68 records the premise as never checked: "not probed: server
ping/idle-timeout behaviour on the legacy route". So we probed it. Two sockets
from a headless browser against the dev backend, `/await` with a query token,
one silent and one sending `"ping"` every 25 s:

| | sent | received | after 392 s |
|---|---|---|---|
| silent | 0 | 0 | **still open** |
| text frame | 15 | 15 | still open |

- **A silent socket survives.** 392 s with zero bytes from the client, nearly 4x
  the 100 s idle window the Step-73 smoke used. Whether cannon pings us (the
  browser answers Pong invisibly, JS cannot observe control frames) or has no
  idle timeout on this route, the browser needs to send nothing. And if cannon
  ever does close one, `onclose` → reconnect → drain-on-open already covers it.
- **Cannon echoes an application text frame back, 1:1.** Which makes a text-frame
  keepalive actively harmful in a doorbell design: the natrium web SDK rings
  before parsing, so its own keepalive triggered a REST drain every 25 s per tab.
  Removed there in the same session; push latency unchanged at 1.78 s.

Caveats: the probe omitted `client=`, because the backend terminates the previous
socket of the same client id (§11.2) and that would have killed the app's own
socket and the measurement. 392 s is a floor, not a proof of no timeout.

### Out of scope

- `core::sync_stream` on wasm stays stubbed. It needs `tokio::spawn`; this step
  does not need it, and conflating the two is what made this look large.
- `/v14/events` in the browser. Current wire-server ships
  ConsumableNotifications hard-disabled (CLAUDE.md, observations §68), so it
  cannot even be smoked. The legacy `/await` doorbell is what the server
  actually pushes through.

## Test strategy

The doorbell's logic is separable from the socket, so most of it needs no
network:

- **Unit, wasm** — a rung bell wakes the loop well inside the idle backoff; an
  unrung bell falls back to it. Drives the real loop, no socket.
- **Unit, both targets** — the `/await` URL carries `client` and
  `access_token`, and rejects a non-`ws` scheme.
- **Smoke (phase 9)** — live against Wire from a browser: a message from
  another device arrives in about a second. The idle-survival half is already
  measured above; the smoke re-checks it with `client=` set, which the probe
  could not.

No CLI command (phase 6): the transport is browser-only and the CLI is native.
The browser suite is its first consumer, as in Step 104.

## Collision watch

`step/109-page-commits` also edits `sync/mod.rs`. This step's change there is
one line at most; resolve in favour of 109 and re-add the gate.

## Retrospective

**Gate** — fmt clean, 0 build warnings, clippy clean natively *and* for
wasm32, `cargo deny` clean, 1039 native tests in 104 suites green, 7 browser
tests green. No smoke yet: that is phase 9 and needs a second device.

**Two limitations this step ships with, both deliberate and neither hidden.**

1. ~~**Transient frames are not dispatched.**~~ **Closed by Step 115b** (same
   branch stack). `parse_legacy_frame` turned out to be pure `serde_json` over
   an already-portable DTO — native-only by file placement, not by dependency —
   so it moved to `wire/push_frame.rs` and both sockets share it. Transients are
   queued for the loop (capped at the native 64, oldest dropped) and dispatched
   through `SyncService::dispatch_transient_notification`, which the native path
   now calls too instead of its own copy. A transient wake deliberately does
   **not** drain: nothing was stored for a drain to find.
2. **A consumer with its own `/await` socket must drop it when adopting this.**
   The backend terminates the previous socket of the same client id (§11.2), so
   two sockets on one device fight, each reconnect killing the other. This is
   not hypothetical: the natrium web SDK runs exactly such a socket today, and
   it is also where typing currently comes from — so for that consumer, (1) and
   (2) combine into "adopt this and lose typing until the follow-up".

   With 115b done this is now a clean swap: the SDK deletes its socket and gets
   both durable push and typing from Kupferdraht.

**What went differently than planned.** The red tests were written as in-module
`#[cfg(test)]` units and could never have run: this crate's lib test build does
not compile for wasm at all — unrelated modules' test blocks pull in `httpmock`
and `tempfile`. Browser tests have to be integration tests, which forced the
seam to be `pub`. Worth knowing before the next browser step writes unit tests
it cannot execute; and `--all-targets` clippy cannot be used for wasm for the
same reason.

The URL builder also moved: it started in `wire/websocket_url.rs` as
"portable", but its only caller is the browser, so on native it was dead code
that `#![deny(warnings)]` rejected — and `wire` is `pub(crate)`, so it could
not be made publicly reachable either. It lives in `wasm/push_socket.rs` now,
which also left `wire/mod.rs` untouched and removed the collision with
`step/109`.

**What surprised us.** That the keepalive was both unnecessary and harmful —
see `LEARNINGS.md` §5.45. The step is smaller than the plan assumed precisely
because we measured instead of porting the native behaviour.
