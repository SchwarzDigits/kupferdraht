# Step 103 — Pace the sync loops: default pause, `429`, reconnect gap

**Status: MERGED 2026-09-29** — commissioned 2026-09-28 (Markus — finding
H8 of the production-readiness review, `open.md` §71). `e0bf9a4`; fix
`8fc3b24`. Merged to `main` as PR #73 (`8151252`); CI green. Smoke
2026-09-28 against schwarz-dev-wire (release CLI). Scope shipped: 5 s
default poll interval, `KupferdrahtError::RateLimited` with `Retry-After`
waited out by every sync loop, ≥ 1 s between push connections.
Retrospective: `docs/LEARNINGS.md` §5.43. Follow-up ADR-0033 (push as
the default) accepted and merged as PR #72. Archived with this header.

## Problem (verified in code 2026-09-28)

- `SyncSettings::poll_interval` defaults to `None`, and `None` means "no
  pause": `Client::sync`, `sync_with_callback` and `sync_stream` start the
  next iteration (conversation list, notifications drain, key-package
  count) the moment an empty one ends; FFI `start_sync` pauses only when
  the value is set. Every consumer that does not set it polls the backend
  in a tight loop.
- The backend rate-limits (`429 too-many-requests`, observations.md).
  Kupferdraht folds a `429` into `BackendRejected`: `Client::sync` and the push
  loop abort on it, FFI `start_sync` retries after 2 s whatever
  `Retry-After` says.
- Push: a connection that processed frames and then closes reconnects at
  once; with `poll_interval: None` failed connections reconnect at once too.
- `sync_with_callback` and `sync_stream` pause with `tokio::time::sleep`,
  which has no timer in the browser; `crate::time::sleep` is the
  target-agnostic one `Client::sync` already uses.

## Decisions (2026-09-28, Markus)

- **D1 — 5 s default pause.** `poll_interval: None` means 5 s;
  `Some(d)` is `d`, `Some(Duration::ZERO)` explicitly no pause (tests).
  Where each loop pauses is unchanged (the core loops after an empty
  drain, FFI `start_sync` after every iteration). The push reconnect
  backoff uses the same effective value as its base.
- **D2 — `KupferdrahtError::RateLimited { retry_after_secs }`** for every `429`
  on the shared rejection path (`Retry-After` in delta-seconds; an
  HTTP-date or none → `None`). The loop variants and the push loop wait —
  `Retry-After`, else the poll interval, bounded to 1–60 s — and carry
  on; `sync_once` and every other call return it. FFI `start_sync` waits
  the same instead of its error backoff; the listener still sees the
  error through `on_error`. A new error variant — **BREAKING** for
  exhaustive matches in Kotlin / Swift (accepted by Markus).
- **D3 — at least 1 s between two push connections.** After a connection
  that processed frames closes, reconnect after min(1 s, poll interval);
  after failed connections the capped exponential as before.
- **D4 — target-agnostic sleep** in every polling loop.

Out of scope: which transport is the default — a separate ADR proposal
(push on native targets) for the owner.

## TDD

- `wire/http/notifications.rs` (the shared rejection path): a `429` with
  `Retry-After: 7` is `RateLimited { Some(7) }`; without the header, and
  with an HTTP-date, `RateLimited { None }`.
- `ffi/sync.rs` (paused clock): an Ok iteration pauses 5 s by default; a
  rate-limited iteration waits its `Retry-After`; a huge `Retry-After` is
  capped at 60 s.
- `sync/push/mod.rs`: failed reconnects back off from 5 s when the
  interval is unset; after a productive connection the gap is
  min(1 s, interval).
- `tests/step_103_polling_load.rs`: `Client::sync` with the interval unset
  drains once in 1.5 s; a `429` on the drain and a `429` on the push
  loop's metadata refresh are waited out (`Retry-After: 1`) and the loop
  keeps running.
- The wasm timer (D4) has no test: the browser has no loop test harness;
  the change is the call site only.

**As built (2026-09-28).**
- `sync/settings.rs`: `DEFAULT_POLL_INTERVAL` (5 s),
  `effective_poll_interval`, `SyncSettings::idle_pause` and
  `rate_limit_pause` (`Retry-After` else the idle pause, clamped 1–60 s).
- `wire/http/mod.rs`: `reject_from_response` maps `429` to
  `RateLimited { retry_after_secs }` (`retry_after_secs`: delta-seconds
  only). `/access` — the token refresh, which runs inside the loops'
  authenticated calls — classifies its own non-2xx and got the same `429`
  arm in the review (2026-09-29, `rebind_rejection`); the login is not
  called from the loops and stays as it is.
- `client/sync.rs`: `sync` / `sync_with_callback` go through
  `sync_once_waiting_out_rate_limits`; `sync_stream` waits a `429` out
  without yielding it; all three pause `idle_pause()` via
  `crate::time::sleep` (D4).
- `ffi/sync.rs`: Ok → `idle_pause()`; Err → `error_pause` (rate-limit
  wait for `RateLimited`, else the unchanged 2 s / `poll_interval` error
  backoff); the listener still gets `on_error` for a `429`.
- `sync/push/mod.rs`: `run_connection` (metadata refresh + negotiation
  + cycle) extracted; `run_push` waits a `RateLimited` out (the wait
  replaces the round's reconnect backoff); `reconnect_delay` bases on the
  effective interval; `reconnect_gap` = min(1 s, interval) after a
  productive connection.
- `wasm/live_sync.rs`: a `429` waits the rate-limit pause instead of the
  fixed 2.5 s (untested, like D4).
- Tests: 11 red → green.
- Smoke 2026-09-28 (schwarz-dev-wire, release CLI): fresh `login`;
  `sync-stream --transport websocket` 15 s (legacy `/await`, one connect
  drain, no warning or error); `sync-stream --transport polling` 9 s (a
  drain every 2 s — the CLI's own `--idle-backoff 2`); `logout
  --purge-device`. A live `429` cannot be provoked responsibly; its
  shape is recorded in observations.md.
