# Step 98 — Asset transfers are not cut off by the request timeout

**Status: MERGED 2026-09-29** — commissioned 2026-09-28 (Markus — finding
H1 of the production-readiness review, `open.md` §71). `d665db4`. Merged
to `main` as PR #67 (`7ca50ab`); CI green. Smoke 2026-09-28 against
schwarz-dev-wire (release CLI, a 20 MB file up and down). Scope shipped:
asset transfers use a client without a total deadline; a download fails
after 30 s without a byte, an upload has no bound of its own; API calls
keep 10 s / 30 s. Retrospective: `docs/LEARNINGS.md` §5.41. Numbered 98
in parallel with the wasm surface parity step (PR #76,
`98-wasm-surface-parity.md`); both keep their number, the file names
tell them apart. Archived with this header.

## Problem (verified in code 2026-09-28)

`HttpClient` (`wire/http/mod.rs`) builds one `reqwest` client with a
10 s connect timeout and a **30 s total timeout per request** — meant to
surface a stalled API call. Asset up- and downloads go through the same
client, so a file whose transfer takes longer than 30 s fails even while
its bytes keep flowing: a 20 MB file needs ~30 s at 5 Mbit/s. The
browser build has no timeouts (`fetch`), so this is native only.

## Decisions (2026-09-28)

- **D1 — API calls keep their bounds** (10 s connect, 30 s total).
- **D2 — Downloads fail on silence, not on length.** No total bound; a
  download fails when no byte arrives for 30 s — while waiting for the
  response and between two chunks of the body.
- **D3 — Uploads get no bound of their own** (Markus, 2026-09-28; a
  size-based bound of 30 s + size / 8 KiB/s was proposed and dropped).
  A dead path fails when the OS gives up the connection (unacknowledged
  data), an unreachable server at the 10 s connect timeout, and the app
  can cancel the send (Step 97a keeps it resendable). Left open on
  purpose: a connection that stays alive while the server stops reading
  hangs until the app cancels. Silence cannot mark an upload as stalled
  anyway — the kernel accepts megabytes into its socket buffers before
  the server reads them.
- **D4 — Browser unchanged** (`fetch` offers no timeout knobs).
- Transfers use a second `reqwest` client without a total timeout (same
  connect timeout); the bounds are applied per transfer.

## TDD (sketch)

Unit tests in `wire/http/assets.rs` against a hand-rolled loopback HTTP
server (httpmock can only delay a whole response), with the timeouts
shrunk through a crate-internal constructor: a download that keeps
flowing past the request timeout completes; a download that stalls fails
after the idle timeout, well before the request timeout; an upload that
the server reads slower than the request timeout completes.

**As built (2026-09-28).**
- `wire/http/mod.rs`: `HttpTimeouts` (`PRODUCTION`: connect 10 s,
  request 30 s, transfer idle 30 s) and `HttpClient::with_timeouts`
  (tests shrink them); a second `reqwest` client `transfer` without a
  total deadline; `checked_base_url` / `build_reqwest` split out of
  `new`. wasm unchanged.
- `wire/http/assets.rs`: `upload_asset` and `download_asset` use
  `transfer`; the download waits at most `transfer_idle` for the
  response and for every chunk (`stalled_download`).
- Tests: `wire/http/transfer_timeout_tests.rs` — a loopback HTTP server
  that streams, stalls or reads slowly; 3 red → green (the proposed
  upload-bound pin was dropped with D3). The first red run proved the
  point: a download whose bytes kept flowing was cut off mid-body by the
  request timeout. Gate 2026-09-28: 964 passed.
- Smoke 2026-09-28 (release CLI, schwarz-dev-wire): a 20 MB file up
  (3.9 s) and down (1.9 s), byte-identical. The link is too fast to pass
  30 s below the server's asset size limit — a 200 MB upload was refused
  with `413 Payload Too Large`, surfaced cleanly — so the > 30 s case
  rests on the loopback tests.
